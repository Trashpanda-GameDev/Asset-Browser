//! The Updates page: this copy's version, what the updater is doing, and its buttons.

use eframe::egui::{self, FontId, RichText, Ui, vec2};
use egui_phosphor::regular as icon;

use crate::ui::updates::{self, UpdateAction};
use crate::ui::{theme, widgets};
use crate::updates::{RELEASES, Status, Updates};

use super::{Command, SettingsArgs, note};

pub(super) fn updates_page(ui: &mut Ui, args: &SettingsArgs<'_>, commands: &mut Vec<Command>) {
    let updates = args.updates;
    version_heading(ui, updates);
    ui.add_space(6.0);
    update_status(ui, updates);
    ui.add_space(6.0);
    update_buttons(ui, updates, commands);
    ui.add_space(10.0);
    note(
        ui,
        "Installed and portable copies check when they start and every 6 hours, and download an update in the background.",
    );
}

/// This copy's version, and whether it's installed, portable or run from a build.
fn version_heading(ui: &mut Ui, updates: &Updates) {
    let kind = match updates.status() {
        Status::NotInstalled => "run from a build",
        _ if updates.is_portable() => "portable",
        _ => "installed",
    };
    ui.label(
        RichText::new(format!("Asset Browser {}", updates.version()))
            .font(FontId::new(theme::size::LARGE, theme::semibold()))
            .color(theme::TEXT),
    );
    ui.label(RichText::new(kind).color(theme::TEXT_FAINT));
}

/// What the updater is doing, or last found.
fn update_status(ui: &mut Ui, updates: &Updates) {
    let checked = updates::checked_ago(updates).map(|ago| format!(", checked {ago}"));
    let (text, color) = match updates.status() {
        Status::NotInstalled => (
            "This copy doesn't update itself: installed and portable copies do, from the releases on GitHub."
                .to_owned(),
            theme::TEXT_WEAK,
        ),
        Status::Checking => ("Checking for updates…".to_owned(), theme::TEXT_WEAK),
        Status::UpToDate => (format!("Up to date{}.", checked.unwrap_or_default()), theme::TEXT_WEAK),
        Status::Downloading { version: new, percent } => (updates::downloading(new, *percent), theme::TEXT_WEAK),
        Status::Ready { version: new } => (updates::ready(new), theme::TEXT),
        Status::Failed { action, error } => (format!("Couldn't {action}: {error}"), theme::ERROR),
    };
    ui.horizontal(|ui| {
        if matches!(updates.status(), Status::Checking | Status::Downloading { .. }) {
            ui.add(egui::Spinner::new().size(theme::size::MEDIUM));
        }
        ui.add(egui::Label::new(RichText::new(text).color(color)).wrap());
    });
}

/// "Restart to update" once an update has downloaded, or else "Check for updates"; What's new,
/// and the releases.
fn update_buttons(ui: &mut Ui, updates: &Updates, commands: &mut Vec<Command>) {
    ui.horizontal(|ui| {
        match updates.status() {
            Status::Ready { .. } => {
                let restart = egui::Button::new(
                    RichText::new(format!("{}  Restart to update", icon::ARROW_CLOCKWISE)).color(theme::ON_ACCENT),
                )
                .fill(theme::ACCENT)
                .min_size(vec2(0.0, 28.0));
                if ui.add(restart).clicked() {
                    commands.push(Command::Update(UpdateAction::Restart));
                }
            }
            status => {
                let can_check = matches!(status, Status::UpToDate | Status::Failed { .. });
                let check = egui::Button::new(format!("{}  Check for updates", icon::ARROW_CLOCKWISE))
                    .min_size(vec2(0.0, 28.0));
                if ui.add_enabled(can_check, check).clicked() {
                    commands.push(Command::Update(UpdateAction::Check));
                }
            }
        }
        if widgets::button(ui, icon::SPARKLE, "What's new").on_hover_text("What changed in each version").clicked() {
            commands.push(Command::WhatsNew);
        }
        if widgets::button(ui, icon::GITHUB_LOGO, "Releases").on_hover_text(format!("{RELEASES}/releases")).clicked() {
            commands.push(Command::OpenLink(format!("{RELEASES}/releases")));
        }
    });
}
