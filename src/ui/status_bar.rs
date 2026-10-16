//! The bar along the bottom: the last error, how many assets there are, what's under way
//! (deleting, scanning, making previews, an update), and the sound playing when it isn't the one
//! shown.

use std::time::{Duration, Instant};

use eframe::egui::{self, Align, Frame, Layout, Margin, RichText, Ui};
use egui_phosphor::regular as icon;

use crate::library::Library;
use crate::paths;
use crate::playback::Playback;
use crate::rescan::ScanTotals;
use crate::thumbs::ThumbProgress;
use crate::updates::Updates;

use super::format::grouped;
use super::organize::plural;
use super::updates::{self, UpdateAction};
use super::{sound, theme, widgets};

pub struct StatusArgs<'a> {
    /// The last error, kept until another one.
    pub error: Option<String>,
    /// `None` when there are no folders.
    pub library: Option<&'a Library>,
    pub deleting: bool,
    /// When the scan under way started, if one is.
    pub scanning_since: Option<Instant>,
    pub last_scan: Option<&'a ScanTotals>,
    /// What's known of the folder index, for the last scan's tooltip.
    pub index_note: Option<&'a str>,
    pub previews: ThumbProgress,
    pub updates: &'a Updates,
    /// The asset in the pane: the sound playing shows here only when it's another.
    pub focus: Option<u32>,
    pub playback: &'a mut Playback,
}

/// What the user asked for from the status bar.
pub enum StatusCommand {
    Update(UpdateAction),
    /// Show the sound playing.
    ShowPlaying(u32),
}

pub fn show(ui: &mut Ui, mut args: StatusArgs<'_>) -> Vec<StatusCommand> {
    let frame = Frame::new().fill(theme::PANEL).inner_margin(Margin::symmetric(12, 4));
    let mut commands = Vec::new();
    egui::Panel::bottom("status").frame(frame).show(ui, |ui| {
        ui.horizontal(|ui| {
            under_way(ui, &args);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                commands.extend(updates::status(ui, args.updates).map(StatusCommand::Update));
                widgets::divider(ui);
                previews(ui, args.previews);
                commands.extend(playing_elsewhere(ui, &mut args).map(StatusCommand::ShowPlaying));
            });
        });
    });
    commands
}

/// Left to right: the last error, the library's size, a deletion under way, and the scan.
fn under_way(ui: &mut Ui, args: &StatusArgs<'_>) {
    if let Some(error) = &args.error {
        ui.label(RichText::new(format!("{}  {error}", icon::WARNING_CIRCLE)).size(11.5).color(theme::ERROR))
            .on_hover_text(format!("Logged to {}", paths::log_file().display()));
        widgets::divider(ui);
    }
    if let Some(library) = args.library {
        ui.label(small(format!("{} assets", grouped(library.entries().len()))));
        widgets::divider(ui);
    }
    if args.deleting {
        ui.add(egui::Spinner::new().size(12.0));
        ui.label(small("Deleting…".to_owned()));
        widgets::divider(ui);
    }
    scan(ui, args);
}

/// The scan under way, or how long the last one took.
fn scan(ui: &mut Ui, args: &StatusArgs<'_>) {
    if let Some(started) = args.scanning_since {
        ui.add(egui::Spinner::new().size(12.0));
        ui.label(small(format!("Scanning… {:.1} s", started.elapsed().as_secs_f32())));
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    } else if let Some(scan) = args.last_scan {
        ui.label(small(format!(
            "Scanned {} in {:.2} s",
            plural(scan.dirs_visited, "folder"),
            scan.elapsed.as_secs_f32()
        )))
        .on_hover_text(format!(
            "{} assets; {} other files skipped. {}",
            grouped(scan.assets),
            grouped(scan.skipped),
            args.index_note.unwrap_or("")
        ));
    }
}

/// How many previews there are, or how many are still to make; how they were made on hover.
fn previews(ui: &mut Ui, progress: ThumbProgress) {
    let average = if progress.made > 0 { progress.make_micros as f64 / progress.made as f64 / 1000.0 } else { 0.0 };
    let details = format!(
        "{} made ({average:.1} ms average on {} threads), {} from the cache, {} couldn't be read",
        grouped(progress.made as usize),
        progress.threads,
        grouped(progress.from_cache as usize),
        grouped(progress.failed as usize),
    );
    let pending = progress.queued + progress.busy;
    if pending > 0 {
        ui.label(small(format!("Making previews… {}", grouped(pending)))).on_hover_text(details);
        ui.add(egui::Spinner::new().size(12.0));
    } else {
        ui.label(small(format!("{} previews", grouped((progress.made + progress.from_cache) as usize))))
            .on_hover_text(details);
    }
}

/// The sound playing, when it isn't the one shown; the entry clicked, to show it.
fn playing_elsewhere(ui: &mut Ui, args: &mut StatusArgs<'_>) -> Option<u32> {
    let library = args.library?;
    if !args.playback.playing.is_some_and(|(index, _)| Some(index) != args.focus) {
        return None;
    }
    widgets::divider(ui);
    sound::now_playing_chip(ui, args.playback, library)
}

fn small(text: String) -> RichText {
    RichText::new(text).size(11.5).color(theme::TEXT_FAINT)
}
