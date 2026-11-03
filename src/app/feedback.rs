//! Feedback from the header's button: the window (`ui::feedback`), and the report sent as a GitHub
//! issue on its own thread (`crate::feedback`), or opened on GitHub's page by a build that can't send.

use std::sync::Arc;

use crossbeam_channel::Receiver;
use eframe::egui;

use super::BrowserApp;
use crate::feedback::{self, FeedbackError, Report, Sent};
use crate::ui::feedback::{self as view, FeedbackArgs, FeedbackCommand, FeedbackForm};
use crate::{diag, platform};

/// The feedback window while it's open, and a report on its way.
#[derive(Default)]
pub(super) struct Feedback {
    form: Option<FeedbackForm>,
    sending: Option<Receiver<Result<Sent, FeedbackError>>>,
    /// Why the last try failed, shown in the window.
    problem: Option<String>,
}

impl Feedback {
    pub(super) fn is_open(&self) -> bool {
        self.form.is_some()
    }

    pub(super) fn open(&mut self) {
        self.form.get_or_insert_with(FeedbackForm::default);
    }

    /// Shows the window; what was asked for, with the report as it stands.
    fn show(
        &mut self,
        ctx: &egui::Context,
        details: Vec<String>,
        last_error: Option<String>,
    ) -> Option<(FeedbackCommand, Report)> {
        let form = self.form.as_mut()?;
        let args = FeedbackArgs {
            details: &details,
            last_error: last_error.as_deref(),
            can_send: feedback::can_send(),
            sending: self.sending.is_some(),
            problem: self.problem.as_deref(),
        };
        let command = view::show(ctx, form, &args)?;
        let error = last_error.filter(|_| form.include_error).map(|error| format!("Last error: {error}"));
        let report = Report {
            kind: form.kind,
            title: form.title.clone(),
            description: form.description.clone(),
            details: details.into_iter().chain(error).collect(),
        };
        Some((command, report))
    }
}

impl BrowserApp {
    pub(super) fn feedback_window(&mut self, ctx: &egui::Context) {
        self.poll_feedback();
        if !self.chrome.feedback.is_open() {
            return;
        }
        let details = self.feedback_details();
        let last_error = diag::last();
        let Some((command, report)) = self.chrome.feedback.show(ctx, details, last_error) else { return };
        match command {
            FeedbackCommand::Send => {
                let ctx = ctx.clone();
                let sending = feedback::send(report.issue(), Arc::new(move || ctx.request_repaint()));
                self.chrome.feedback.problem = None;
                self.chrome.feedback.sending = Some(sending);
            }
            FeedbackCommand::OpenOnGitHub => self.open_report_on_github(&report),
            FeedbackCommand::Close => self.chrome.feedback = Feedback::default(),
        }
    }

    /// Takes in the answer to a report on its way.
    fn poll_feedback(&mut self) {
        let feedback = &mut self.chrome.feedback;
        let Some(answer) = feedback.sending.as_ref().and_then(|rx| rx.try_recv().ok()) else { return };
        feedback.sending = None;
        match answer {
            Ok(sent) => {
                *feedback = Feedback::default();
                self.notify(match sent.number {
                    Some(number) => format!("Thanks! Sent as issue #{number}"),
                    None => "Thanks! Sent".to_owned(),
                });
            }
            Err(err) => {
                diag::note("Sending feedback", &err);
                feedback.problem = Some(format!("Not sent: {err}. You can open it on GitHub and send it there."));
            }
        }
    }

    fn open_report_on_github(&mut self, report: &Report) {
        match platform::open_link(&report.issue().page()) {
            Ok(()) => {
                self.chrome.feedback = Feedback::default();
                self.notify("Opened on GitHub, to send it there".to_owned());
            }
            Err(err) => self.chrome.feedback.problem = Some(format!("GitHub's page didn't open: {err}")),
        }
    }

    /// What goes with a report about the app itself.
    fn feedback_details(&self) -> Vec<String> {
        let mut details = vec![format!("Asset Browser {}", env!("CARGO_PKG_VERSION")), platform::os_version()];
        if let Some(render) = &self.render_state {
            let adapter = render.adapter.get_info();
            details.push(format!("{} ({:?})", adapter.name, adapter.backend));
        }
        details
    }
}
