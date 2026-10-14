//! What's new in each version: `CHANGELOG.md`, built into the app, so a version's notes are there
//! offline and every earlier version's too. The release workflow publishes the same notes with
//! the release (`.github/workflows/release.yml`).
//!
//! A version's section starts `## 0.2.0 – 13 Oct 2026`; `## Unreleased` gathers what's coming.
//! Inside one: `- ` items (continued on lines indented by two spaces), `### ` headings and plain
//! paragraphs, with `**bold**`.

use std::sync::OnceLock;

use semver::Version;

const TEXT: &str = include_str!("../CHANGELOG.md");

/// One version's notes.
#[derive(Debug, PartialEq)]
pub struct Release {
    /// `None` for the unreleased section.
    pub version: Option<Version>,
    /// "13 Oct 2026", when the heading has one.
    pub date: Option<String>,
    pub blocks: Vec<Block>,
}

#[derive(Debug, PartialEq)]
pub enum Block {
    Heading(String),
    Item(String),
    Text(String),
}

/// Every version in the built-in changelog, newest first, as written.
pub fn releases() -> &'static [Release] {
    static RELEASES: OnceLock<Vec<Release>> = OnceLock::new();
    RELEASES.get_or_init(|| parse(TEXT))
}

pub fn parse(text: &str) -> Vec<Release> {
    let mut releases: Vec<Release> = Vec::new();
    // A block being gathered over several lines: an item's continuation or a paragraph.
    let mut open: Option<Block> = None;
    let close = |releases: &mut Vec<Release>, open: &mut Option<Block>| {
        if let (Some(release), Some(block)) = (releases.last_mut(), open.take()) {
            release.blocks.push(block);
        }
    };
    for line in text.lines() {
        let line = line.trim_end();
        if let Some(heading) = line.strip_prefix("## ") {
            close(&mut releases, &mut open);
            let (name, date) = match heading.split_once(['–', '—']).or_else(|| heading.split_once(" - ")) {
                Some((name, date)) => (name.trim(), Some(date.trim().to_owned()).filter(|date| !date.is_empty())),
                None => (heading.trim(), None),
            };
            releases.push(Release { version: Version::parse(name).ok(), date, blocks: Vec::new() });
            continue;
        }
        if releases.is_empty() {
            continue; // the introduction
        }
        if let Some(heading) = line.strip_prefix("### ") {
            close(&mut releases, &mut open);
            releases.last_mut().unwrap().blocks.push(Block::Heading(heading.trim().to_owned()));
        } else if let Some(item) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            close(&mut releases, &mut open);
            open = Some(Block::Item(item.trim().to_owned()));
        } else if line.trim().is_empty() {
            close(&mut releases, &mut open);
        } else {
            match &mut open {
                Some(Block::Item(text) | Block::Text(text)) => {
                    text.push(' ');
                    text.push_str(line.trim());
                }
                _ => open = Some(Block::Text(line.trim().to_owned())),
            }
        }
    }
    close(&mut releases, &mut open);
    releases
}

/// The releases the user hasn't seen the notes of: those after `seen`, up to and including
/// `current`, newest first.
pub fn since<'a>(releases: &'a [Release], seen: &Version, current: &Version) -> Vec<&'a Release> {
    releases.iter().filter(|release| release.version.as_ref().is_some_and(|v| v > seen && v <= current)).collect()
}

/// What to do about "What's new" when the app starts. `seen` is the last version whose notes were
/// shown (or would have been); `used_before` says the app has run before (any saved settings), to
/// tell an update from a first install when nothing was recorded, as before notes existed.
#[derive(Debug, PartialEq)]
pub enum AtStart {
    /// A first install, the same version, or an older one: nothing to show.
    Nothing,
    /// Show the notes of the versions after this one.
    Since(Version),
}

pub fn at_start(releases: &[Release], seen: Option<&str>, used_before: bool, current: &Version) -> AtStart {
    let seen = match seen.and_then(|seen| Version::parse(seen).ok()) {
        Some(seen) => seen,
        // Updated from a version that didn't record what it showed: just this version's notes.
        None if used_before => match releases
            .iter()
            .filter_map(|release| release.version.as_ref())
            .filter(|version| *version < current)
            .max()
        {
            Some(previous) => previous.clone(),
            None => return AtStart::Nothing,
        },
        None => return AtStart::Nothing,
    };
    if since(releases, &seen, current).is_empty() { AtStart::Nothing } else { AtStart::Since(seen) }
}

/// The version to remember as seen after starting `current`: never an older one than before, so
/// running an older copy now and then doesn't bring the newer one's notes back.
pub fn remembered(seen: Option<&str>, current: &Version) -> Version {
    match seen.and_then(|seen| Version::parse(seen).ok()) {
        Some(seen) if seen > *current => seen,
        _ => current.clone(),
    }
}

/// `**bold**` parts of a line, as (text, bold) runs; backticks are dropped.
pub fn runs(text: &str) -> Vec<(String, bool)> {
    let text = text.replace('`', "");
    text.split("**")
        .enumerate()
        .filter(|(_, part)| !part.is_empty())
        .map(|(index, part)| (part.to_owned(), index % 2 == 1))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# Changelog\n\nIntro text.\n\n## Unreleased\n\n- **Settings**, from the gear\n  and Ctrl+,.\n\
                          - Another\n\n## 0.2.0 – 13 Oct 2026\n\n### New\n\nA paragraph\nover two lines.\n\n- Item\n\n\
                          ## 0.1.0 - 12 Oct 2026\n\n- First\n";

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn sections_items_and_paragraphs() {
        let releases = parse(SAMPLE);
        assert_eq!(releases.len(), 3);
        assert_eq!(releases[0].version, None);
        assert_eq!(
            releases[0].blocks,
            [Block::Item("**Settings**, from the gear and Ctrl+,.".into()), Block::Item("Another".into())]
        );
        assert_eq!(releases[1].version, Some(version("0.2.0")));
        assert_eq!(releases[1].date.as_deref(), Some("13 Oct 2026"));
        assert_eq!(
            releases[1].blocks,
            [
                Block::Heading("New".into()),
                Block::Text("A paragraph over two lines.".into()),
                Block::Item("Item".into())
            ]
        );
        assert_eq!(releases[2].date.as_deref(), Some("12 Oct 2026"), "a plain hyphen works too");
    }

    #[test]
    fn what_to_show_at_start() {
        let releases = parse(SAMPLE);
        let current = version("0.2.0");
        assert_eq!(at_start(&releases, Some("0.1.0"), true, &current), AtStart::Since(version("0.1.0")));
        assert_eq!(at_start(&releases, Some("0.2.0"), true, &current), AtStart::Nothing, "seen already");
        assert_eq!(at_start(&releases, Some("0.3.0"), true, &current), AtStart::Nothing, "went back a version");
        assert_eq!(at_start(&releases, None, false, &current), AtStart::Nothing, "a first install");
        // Updated from a version that recorded nothing: this version's notes only.
        assert_eq!(at_start(&releases, None, true, &current), AtStart::Since(version("0.1.0")));
        let shown: Vec<_> = since(&releases, &version("0.1.0"), &current).iter().map(|r| r.version.clone()).collect();
        assert_eq!(shown, [Some(version("0.2.0"))]);
        // A build whose version has no notes (still Unreleased) shows nothing.
        assert_eq!(at_start(&releases, Some("0.2.0"), true, &version("0.2.1")), AtStart::Nothing);
    }

    #[test]
    fn an_older_copy_keeps_the_newer_notes_seen() {
        assert_eq!(remembered(Some("0.2.0"), &version("0.1.0")), version("0.2.0"));
        assert_eq!(remembered(Some("0.1.0"), &version("0.2.0")), version("0.2.0"));
        assert_eq!(remembered(None, &version("0.2.0")), version("0.2.0"));
        assert_eq!(remembered(Some("garbage"), &version("0.2.0")), version("0.2.0"));
    }

    #[test]
    fn bold_runs() {
        assert_eq!(runs("**Settings**, from `Ctrl+,`"), [("Settings".into(), true), (", from Ctrl+,".into(), false)]);
    }

    #[test]
    fn the_built_in_changelog_reads() {
        let releases = releases();
        assert!(releases.iter().any(|release| release.version == Some(version("0.1.0"))));
        assert!(releases.iter().all(|release| !release.blocks.is_empty()), "no empty sections");
    }
}
