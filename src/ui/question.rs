//! A question in a window over the app, such as "Delete?": the answers are buttons, Enter can
//! answer yes, and Esc or a click outside the window cancels.

use eframe::egui::{self, Align, Key, Layout, Modifiers, RichText};

use super::{theme, widgets};

pub struct Question<'a> {
    /// Tells the windows apart.
    pub id: &'a str,
    pub width: f32,
    pub title: &'a str,
    pub details: &'a str,
    /// A line under the details, in a colour of its own: a warning, say.
    pub note: Option<RichText>,
    /// The button that does it.
    pub confirm: Answer<'a>,
    /// Another way out, beside Cancel.
    pub other: Option<Answer<'a>>,
    /// Enter answers with `confirm`.
    pub enter_confirms: bool,
}

/// A button to answer with.
#[derive(Clone, Copy)]
pub struct Answer<'a> {
    pub glyph: &'a str,
    pub label: &'a str,
    pub tooltip: Option<&'a str>,
}

/// What the question was answered with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answered {
    Confirm,
    Other,
    Cancel,
}

/// Shows the question; the answer, once there is one.
pub fn ask(ctx: &egui::Context, question: &Question<'_>) -> Option<Answered> {
    let mut answered = None;
    let modal = egui::Modal::new(egui::Id::new(question.id)).show(ctx, |ui| {
        ui.set_width(question.width);
        ui.label(RichText::new(question.title).font(egui::FontId::new(theme::size::TITLE, theme::semibold())));
        ui.add_space(4.0);
        ui.add(egui::Label::new(RichText::new(question.details).color(theme::TEXT_WEAK)).wrap());
        if let Some(note) = &question.note {
            ui.add_space(6.0);
            ui.add(egui::Label::new(note.clone()).wrap());
        }
        ui.add_space(12.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if answer_button(ui, question.confirm) {
                answered = Some(Answered::Confirm);
            }
            if widgets::button(ui, "", "Cancel").clicked() {
                answered = Some(Answered::Cancel);
            }
            if let Some(other) = question.other
                && answer_button(ui, other)
            {
                answered = Some(Answered::Other);
            }
        });
    });
    let enter = || ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Enter));
    if answered.is_none() && question.enter_confirms && enter() {
        answered = Some(Answered::Confirm);
    }
    if answered.is_none() && modal.should_close() {
        answered = Some(Answered::Cancel);
    }
    answered
}

/// Whether it was clicked.
fn answer_button(ui: &mut egui::Ui, answer: Answer<'_>) -> bool {
    let response = widgets::button(ui, answer.glyph, answer.label);
    match answer.tooltip {
        Some(tooltip) => response.on_hover_text(tooltip).clicked(),
        None => response.clicked(),
    }
}
