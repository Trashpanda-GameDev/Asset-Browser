//! The updater in the status bar: this copy's version, what the updater is doing, and "Restart to
//! update" once a newer release has downloaded.

use std::time::{Duration, UNIX_EPOCH};

use eframe::egui::{self, Color32, RichText, Sense, Ui};
use egui_phosphor::regular as icon;

use super::format::format_age;
use super::theme;
use crate::updates::{Status, Updates};

/// How long the answer to a check the user asked for stays up.
const ANSWER_TIME: Duration = Duration::from_secs(4);

/// What's asked of the updater, from the status bar or the settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateAction {
    Check,
    /// Restart into the update that has downloaded.
    Restart,
}

/// Draws it, for the right-to-left end of the status bar. Returns what was clicked.
#[expect(clippy::too_many_lines, reason = "refactor: split into named steps")]
pub fn status(ui: &mut Ui, updates: &Updates) -> Option<UpdateAction> {
    let small = |text: String, color: Color32| RichText::new(text).size(11.5).color(color);
    let version = updates.version();
    let about = format!("Asset Browser {version}, {}", if updates.is_portable() { "portable" } else { "installed" });
    let answering = updates.answered().map(|at| at.elapsed()).filter(|shown| *shown < ANSWER_TIME);
    if let Some(shown) = answering {
        ui.ctx().request_repaint_after(ANSWER_TIME - shown);
    }
    // The version, which checks again when clicked.
    let version_label = |ui: &mut Ui, hover: String| {
        let label = egui::Label::new(small(format!("v{version}"), theme::TEXT_FAINT)).sense(Sense::click());
        ui.add(label).on_hover_text(hover).clicked().then_some(UpdateAction::Check)
    };
    match updates.status() {
        Status::NotInstalled => {
            ui.label(small(format!("v{version}"), theme::TEXT_FAINT)).on_hover_text(format!(
                "Asset Browser {version}, run from a build, so it doesn't update itself. Installed and portable \
                 copies do."
            ));
            None
        }
        Status::Checking if updates.asked() => {
            ui.label(small("Checking for updates…".to_owned(), theme::TEXT_FAINT));
            ui.add(egui::Spinner::new().size(12.0));
            None
        }
        Status::Checking => {
            version_label(ui, format!("{about}. Checking for updates…"));
            None
        }
        Status::UpToDate if answering.is_some() => {
            ui.label(small(format!("{}  Up to date", icon::CHECK), theme::TEXT_WEAK));
            None
        }
        Status::UpToDate => {
            let when = updates
                .checked()
                .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
                .map_or(String::new(), |at| format!(" (checked {})", format_age(at.as_nanos() as i64)));
            version_label(ui, format!("{about}. Up to date{when}.\nClick to check again."))
        }
        Status::Failed { action, error } if answering.is_some() => {
            ui.label(small(format!("{}  Couldn't {action}", icon::WARNING_CIRCLE), theme::ERROR))
                .on_hover_text(error.as_str());
            None
        }
        Status::Failed { action, error } => {
            version_label(ui, format!("{about}. Couldn't {action}: {error}\nClick to try again."))
        }
        Status::Downloading { version: new, percent } => {
            ui.label(small(format!("Downloading {new}… {percent}%"), theme::TEXT_FAINT))
                .on_hover_text(format!("{about}. Downloading Asset Browser {new}."));
            ui.add(egui::Spinner::new().size(12.0));
            None
        }
        Status::Ready { version: new } => {
            let text = small(format!("{}  Restart to update", icon::ARROW_CLOCKWISE), theme::ACCENT);
            let button = egui::Button::new(text).fill(theme::ACCENT_SOFT).small();
            let hover = format!(
                "Asset Browser {new} is downloaded. Restart to use it now, or it's installed when you close the app."
            );
            ui.add(button).on_hover_text(hover).clicked().then_some(UpdateAction::Restart)
        }
    }
}
