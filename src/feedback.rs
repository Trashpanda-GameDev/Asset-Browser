//! Feedback from inside the app: a bug report, an idea or a remark, sent as an issue in the app's
//! GitHub repository, on a thread of its own. Release builds carry a key for that, put in when
//! they're built (`build.rs`); a build without one can't send, and opens GitHub's new-issue page
//! with the report filled in instead, for the person to send it there.

use std::time::Duration;

use crossbeam_channel::Receiver;

use crate::work::{self, Wake};

/// Where reports go.
pub const REPOSITORY: &str = "Trashpanda-GameDev/Asset-Browser";

/// How much of a report goes in a new-issue page's address: GitHub turns down longer ones.
const URL_BODY_MAX: usize = 6000;

include!(concat!(env!("OUT_DIR"), "/feedback_key.rs"));

/// What a report is about.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Bug,
    Idea,
    Feedback,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Bug, Kind::Idea, Kind::Feedback];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Bug => "Bug",
            Kind::Idea => "Idea",
            Kind::Feedback => "Feedback",
        }
    }

    /// GitHub's own label for it, where it has one.
    fn github_label(self) -> Option<&'static str> {
        match self {
            Kind::Bug => Some("bug"),
            Kind::Idea => Some("enhancement"),
            Kind::Feedback => None,
        }
    }
}

/// What the person wrote, and what the app adds about itself (its version, Windows, the graphics).
pub struct Report {
    pub kind: Kind,
    pub title: String,
    pub description: String,
    pub details: Vec<String>,
}

/// A report as a GitHub issue.
#[derive(Debug, PartialEq, Eq)]
pub struct Issue {
    pub title: String,
    pub body: String,
    pub labels: Vec<&'static str>,
}

/// The issue a report became.
#[derive(Debug, Clone)]
pub struct Sent {
    /// The issue's number, when GitHub's answer has it.
    pub number: Option<u64>,
}

/// Why a report wasn't sent.
#[derive(Debug, thiserror::Error)]
pub enum FeedbackError {
    #[error("this copy of the app can't send reports itself")]
    NoKey,
    #[error("{0}")]
    Http(#[from] ureq::Error),
    #[error("GitHub turned it down ({status}): {message}")]
    Refused { status: u16, message: String },
    #[error("GitHub's answer couldn't be read: {0}")]
    Answer(#[from] serde_json::Error),
}

impl Report {
    pub fn issue(&self) -> Issue {
        let mut body = self.description.trim().to_owned();
        if body.is_empty() {
            body = "(No description.)".to_owned();
        }
        body.push_str("\n\n---\nSent from the app:\n");
        for detail in &self.details {
            body.push_str(&format!("- {detail}\n"));
        }
        Issue {
            title: format!("{}: {}", self.kind.label(), self.title.trim()),
            body,
            labels: self.kind.github_label().into_iter().collect(),
        }
    }
}

impl Issue {
    /// GitHub's new-issue page with this filled in, for sending it there (with a GitHub account).
    pub fn page(&self) -> String {
        let encode =
            |text: &str| percent_encoding::utf8_percent_encode(text, percent_encoding::NON_ALPHANUMERIC).to_string();
        let mut body: String = self.body.chars().take(URL_BODY_MAX).collect();
        if body.len() < self.body.len() {
            body.push('…');
        }
        let mut url =
            format!("https://github.com/{REPOSITORY}/issues/new?title={}&body={}", encode(&self.title), encode(&body));
        if !self.labels.is_empty() {
            url.push_str(&format!("&labels={}", encode(&self.labels.join(","))));
        }
        url
    }
}

/// Whether this build can send reports itself.
pub fn can_send() -> bool {
    !SCRAMBLED_KEY.is_empty()
}

/// Sends the issue on a thread of its own; `wake` runs when the answer is in.
pub fn send(issue: Issue, wake: Wake) -> Receiver<Result<Sent, FeedbackError>> {
    let (tx, rx) = crossbeam_channel::bounded(1);
    work::spawn("feedback", move || {
        // Fails only when the window has closed meanwhile.
        let _ = tx.send(post(&issue));
        wake();
    });
    rx
}

fn post(issue: &Issue) -> Result<Sent, FeedbackError> {
    let key = unscramble(SCRAMBLED_KEY, KEY_MASK).ok_or(FeedbackError::NoKey)?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .build()
        .into();
    let body = serde_json::json!({ "title": issue.title, "body": issue.body, "labels": issue.labels }).to_string();
    let mut response = agent
        .post(format!("https://api.github.com/repos/{REPOSITORY}/issues"))
        .header("Authorization", format!("Bearer {key}"))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", concat!("Asset Browser ", env!("CARGO_PKG_VERSION")))
        .content_type("application/json")
        .send(body.as_str())?;
    let status = response.status().as_u16();
    let answer = response.body_mut().read_to_string()?;
    read_answer(status, &answer)
}

/// The issue GitHub made, from its answer; or why it didn't.
fn read_answer(status: u16, answer: &str) -> Result<Sent, FeedbackError> {
    let json: serde_json::Value = serde_json::from_str(answer)?;
    if status != 201 {
        let message = json["message"].as_str().unwrap_or("no reason given").to_owned();
        return Err(FeedbackError::Refused { status, message });
    }
    Ok(Sent { number: json["number"].as_u64() })
}

/// The key `build.rs` put in, or `None` for a build without one.
fn unscramble(scrambled: &[u8], mask: &[u8]) -> Option<String> {
    let bytes: Vec<u8> = scrambled.iter().zip(mask.iter().cycle()).map(|(byte, mask)| byte ^ mask).collect();
    String::from_utf8(bytes).ok().filter(|key| !key.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(kind: Kind) -> Report {
        Report {
            kind,
            title: "  The grid jumps  ".to_owned(),
            description: "It jumps when I scroll.\nEvery time.".to_owned(),
            details: vec!["Asset Browser 0.2.2".to_owned(), "Windows 11, version 24H2 (build 26100)".to_owned()],
        }
    }

    #[test]
    fn a_report_becomes_an_issue_with_the_apps_details() {
        let issue = report(Kind::Bug).issue();
        assert_eq!(issue.title, "Bug: The grid jumps");
        assert!(issue.body.starts_with("It jumps when I scroll.\nEvery time.\n\n---\n"), "{}", issue.body);
        assert!(
            issue.body.ends_with("- Asset Browser 0.2.2\n- Windows 11, version 24H2 (build 26100)\n"),
            "{}",
            issue.body
        );
        assert_eq!(issue.labels, ["bug"]);
        assert_eq!(report(Kind::Idea).issue().labels, ["enhancement"]);
        assert!(report(Kind::Feedback).issue().labels.is_empty(), "GitHub has no label for it");
        let empty = Report { description: " ".to_owned(), ..report(Kind::Idea) };
        assert!(empty.issue().body.starts_with("(No description.)"));
    }

    #[test]
    fn the_page_has_the_report_filled_in() {
        let page = report(Kind::Bug).issue().page();
        assert!(page.starts_with("https://github.com/Trashpanda-GameDev/Asset-Browser/issues/new?title=Bug%3A%20The%20grid%20jumps&body=It%20jumps"));
        assert!(page.ends_with("&labels=bug"), "{page}");
        let long = Report { description: "é".repeat(URL_BODY_MAX * 2), ..report(Kind::Feedback) }.issue();
        let page = long.page();
        assert!(page.len() < URL_BODY_MAX * 7, "kept short enough for GitHub: {}", page.len());
        assert!(!page.contains("labels="));
    }

    #[test]
    fn the_key_comes_back_as_it_went_in() {
        let mask = [0x5a, 0x13, 0x77];
        let scrambled: Vec<u8> = "github_pat_example".bytes().zip(mask.iter().cycle()).map(|(b, m)| b ^ m).collect();
        assert!(!scrambled.windows(6).any(|w| w == b"github"), "not there as text");
        assert_eq!(unscramble(&scrambled, &mask).as_deref(), Some("github_pat_example"));
        assert_eq!(unscramble(&[], &mask), None, "a build without one");
    }

    #[test]
    fn githubs_answer_says_which_issue_or_why_not() {
        let made = read_answer(201, r#"{"number": 42, "html_url": "https://github.com/x/y/issues/42"}"#).unwrap();
        assert_eq!(made.number, Some(42));
        let refused = read_answer(401, r#"{"message": "Bad credentials"}"#).unwrap_err();
        assert_eq!(refused.to_string(), "GitHub turned it down (401): Bad credentials");
        assert_eq!(FeedbackError::NoKey.to_string(), "this copy of the app can't send reports itself");
    }
}
