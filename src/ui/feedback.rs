//! The feedback window: what a report is about, a title and a description, and what the app adds
//! about itself. It hands back what was asked for; sending is the app's (`crate::feedback`).

use eframe::egui::{self, Align, FontId, Layout, RichText};
use egui_phosphor::regular as icon;

use super::{theme, widgets};
use crate::feedback::Kind;

/// What's typed in the window, kept while it's open.
#[derive(Default)]
pub struct FeedbackForm {
    pub kind: Kind,
    pub title: String,
    pub description: String,
    /// The last error the app showed goes with the report.
    pub include_error: bool,
}

pub struct FeedbackArgs<'a> {
    /// What the app adds about itself.
    pub details: &'a [String],
    /// The last error the app showed, to offer adding.
    pub last_error: Option<&'a str>,
    /// This build can send reports itself; otherwise they're sent from GitHub's page.
    pub can_send: bool,
    pub sending: bool,
    /// Why the last try failed.
    pub problem: Option<&'a str>,
}

/// What the user asked for in the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeedbackCommand {
    Send,
    /// Open GitHub's new-issue page with the report filled in.
    OpenOnGitHub,
    Close,
}

pub fn show(ctx: &egui::Context, form: &mut FeedbackForm, args: &FeedbackArgs<'_>) -> Option<FeedbackCommand> {
    let mut command = None;
    let modal = widgets::modal_window("feedback", 20).show(ctx, |ui| {
        ui.set_width(460.0);
        ui.label(RichText::new("Send feedback").font(FontId::new(theme::size::TITLE, theme::semibold())));
        ui.add_space(4.0);
        let note = "Report a bug, suggest an idea or say what you think. It becomes a public issue on GitHub, so \
                    leave out anything private.";
        ui.add(egui::Label::new(RichText::new(note).color(theme::TEXT_WEAK)).wrap());
        ui.add_space(10.0);
        kind_choice(ui, form);
        ui.add_space(10.0);
        fields(ui, form);
        ui.add_space(8.0);
        sent_with_it(ui, form, args);
        if let Some(problem) = args.problem {
            ui.add_space(6.0);
            ui.add(egui::Label::new(RichText::new(problem).color(theme::ERROR)).wrap());
        }
        ui.add_space(12.0);
        command = buttons(ui, form, args);
    });
    if command.is_none() && modal.should_close() && !args.sending {
        command = Some(FeedbackCommand::Close);
    }
    command
}

fn kind_choice(ui: &mut egui::Ui, form: &mut FeedbackForm) {
    let segments: Vec<widgets::Segment<'_>> = Kind::ALL
        .iter()
        .map(|kind| widgets::Segment {
            glyph: match kind {
                Kind::Bug => icon::BUG,
                Kind::Idea => icon::LIGHTBULB,
                Kind::Feedback => icon::CHAT_CIRCLE_TEXT,
            },
            label: kind.label(),
            note: None,
            tooltip: "",
        })
        .collect();
    let selected = Kind::ALL.iter().position(|&kind| kind == form.kind).unwrap_or(0);
    if let Some(index) = widgets::segmented(ui, ui.id().with("feedback-kind"), &segments, selected) {
        form.kind = Kind::ALL[index];
    }
}

fn fields(ui: &mut egui::Ui, form: &mut FeedbackForm) {
    let hint = match form.kind {
        Kind::Bug => "What went wrong, in a line",
        Kind::Idea => "What you'd like, in a line",
        Kind::Feedback => "What it's about, in a line",
    };
    ui.add(egui::TextEdit::singleline(&mut form.title).hint_text(hint).char_limit(120).desired_width(f32::INFINITY));
    ui.add_space(6.0);
    let hint = match form.kind {
        Kind::Bug => "What you did, what happened, and what you expected",
        Kind::Idea | Kind::Feedback => "Tell us more",
    };
    ui.add(
        egui::TextEdit::multiline(&mut form.description)
            .hint_text(hint)
            .char_limit(8000)
            .desired_rows(8)
            .desired_width(f32::INFINITY),
    );
}

/// What the app adds, shown so nothing goes without the person seeing it.
fn sent_with_it(ui: &mut egui::Ui, form: &mut FeedbackForm, args: &FeedbackArgs<'_>) {
    let faint = |text: String| RichText::new(text).size(theme::size::SMALL).color(theme::TEXT_FAINT);
    ui.label(faint(format!("Sent with it: {}", args.details.join(" · "))));
    if let Some(error) = args.last_error {
        ui.checkbox(&mut form.include_error, faint(format!("And the last error: {error}")));
    }
}

fn buttons(ui: &mut egui::Ui, form: &FeedbackForm, args: &FeedbackArgs<'_>) -> Option<FeedbackCommand> {
    let mut command = None;
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        let ready = !form.title.trim().is_empty() && !args.sending;
        let (glyph, label) = match (args.can_send, args.sending) {
            (_, true) => (icon::HOURGLASS, "Sending…"),
            (true, false) => (icon::PAPER_PLANE_RIGHT, "Send"),
            (false, false) => (icon::ARROW_SQUARE_OUT, "Open on GitHub"),
        };
        let send = ui.add_enabled_ui(ready, |ui| widgets::button(ui, glyph, label)).inner;
        if send.clicked() {
            command = Some(if args.can_send { FeedbackCommand::Send } else { FeedbackCommand::OpenOnGitHub });
        }
        if !args.can_send && ready {
            send.on_hover_text("Opens GitHub's page with the report filled in, to send it there");
        }
        if ui.add_enabled_ui(!args.sending, |ui| widgets::button(ui, "", "Cancel")).inner.clicked() {
            command = Some(FeedbackCommand::Close);
        }
        if args.can_send
            && args.problem.is_some()
            && ui
                .add_enabled_ui(ready, |ui| widgets::button(ui, icon::ARROW_SQUARE_OUT, "Open on GitHub"))
                .inner
                .clicked()
        {
            command = Some(FeedbackCommand::OpenOnGitHub);
        }
    });
    command
}
