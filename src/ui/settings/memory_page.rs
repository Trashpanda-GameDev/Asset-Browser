//! The Memory page: how much memory the app may use, and how it's shared out.

use std::time::Duration;

use eframe::egui::{self, Align, Layout, RichText, Ui};
use egui_phosphor::regular as icon;

use crate::memory::{self, Shares};
use crate::ui::format::{format_size, grouped};
use crate::ui::{theme, widgets};

use super::{Command, SettingsArgs, note};

pub(super) fn memory_page(ui: &mut Ui, args: &SettingsArgs<'_>, commands: &mut Vec<Command>) {
    note(
        ui,
        "How much memory the app may use. It shares this out among what it keeps to be quick; what the window and the \
         graphics card need comes first, and opening one very large file can take more for a moment.",
    );
    ui.add_space(6.0);
    let mut gigabytes = args.memory_mb as f32 / 1024.0;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Memory limit").color(theme::TEXT));
        let range = memory::MIN_MB as f32 / 1024.0..=memory::MAX_MB as f32 / 1024.0;
        let slider = egui::Slider::new(&mut gigabytes, range)
            .step_by(0.25)
            .custom_formatter(|value, _| format!("{value:.2} GB"))
            .custom_parser(|text| text.trim().trim_end_matches("GB").trim().parse::<f64>().ok());
        // A slider's length is a spacing setting, not the room it's given.
        ui.spacing_mut().slider_width = 300.0;
        if ui.add(slider).changed() {
            commands.push(Command::SetMemory((gigabytes * 1024.0).round() as u64));
        }
        if args.memory_mb != memory::DEFAULT_MB
            && widgets::icon_button_sized(
                ui,
                icon::ARROW_COUNTER_CLOCKWISE,
                "Back to the default (1.5 GB)",
                false,
                24.0,
            )
            .clicked()
        {
            commands.push(Command::SetMemory(memory::DEFAULT_MB));
        }
    });
    if let Some(in_use) = crate::platform::memory_in_use() {
        ui.label(RichText::new(format!("In use now: {}", format_size(in_use))).color(theme::TEXT_WEAK));
        ui.ctx().request_repaint_after(Duration::from_secs(1));
    }
    ui.add_space(8.0);
    widgets::section_label(ui, "Within it");
    let shares: Shares = memory::shares(args.memory_mb);
    let thumbnails = shares.thumbnails as u64 * 192 * 192 * 4;
    share_row(ui, "Pictures being made into thumbnails, at once", format_size(shares.decoding));
    share_row(
        ui,
        "Thumbnails kept on the graphics card",
        format!("{} ({})", grouped(shares.thumbnails), format_size(thumbnails)),
    );
    share_row(ui, "Sounds read ahead of the selection", format_size(shares.read_ahead));
    share_row(ui, "Textures of 3D models", format_size(shares.model_textures));
    share_row(ui, "An animated GIF's frames", format!("{:.1} megapixels", shares.animation_pixels as f64 / 1e6));
}

/// "Name ........ value" with the value at the right.
fn share_row(ui: &mut Ui, name: &str, value: String) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(name).color(theme::TEXT_WEAK));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(value).monospace().color(theme::TEXT));
        });
    });
}
