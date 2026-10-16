//! The Storage page: how much space the cache and your data take, part by part, clearing parts
//! of them, and the logs.

use std::path::Path;
use std::time::Duration;

use eframe::egui::{self, Align, FontId, Frame, Layout, Margin, RichText, Ui, vec2};
use egui_phosphor::regular as icon;

use crate::storage::{Part, Usage};
use crate::ui::format::{format_size, grouped, plural};
use crate::ui::{theme, widgets};

use super::{Command, Settings, SettingsArgs, note, tick};

impl Settings {
    pub(super) fn storage_page(&mut self, ui: &mut Ui, args: &SettingsArgs<'_>, commands: &mut Vec<Command>) {
        // Measured when first shown, and again after clearing.
        if args.usage.is_none() && !args.measuring && !args.clearing {
            commands.push(Command::Measure);
        }
        let usage = match args.usage {
            Some(Ok(usage)) => Some(usage.clone()),
            Some(Err(error)) => {
                ui.colored_label(theme::ERROR, format!("Couldn't measure the files: {error}"));
                None
            }
            None => None,
        };
        let measuring = args.measuring || args.clearing;
        note(ui, "The cache is made again as it's needed; your data can't be. Tick what to clear.");
        ui.add_space(6.0);
        file_row(ui, "Cache", args.cache_path, usage.as_ref().map(|usage| usage.cache_file), commands);
        for part in Part::CACHE {
            self.part_row(ui, part, usage.as_ref(), measuring);
        }
        ui.add_space(10.0);
        file_row(ui, "Your data", args.user_path, usage.as_ref().map(|usage| usage.user_file), commands);
        for part in Part::USER {
            self.part_row(ui, part, usage.as_ref(), measuring);
        }
        ui.add_space(10.0);
        self.clear_controls(ui, args.clearing, usage.as_ref(), commands);
        ui.add_space(14.0);
        log_rows(ui, usage.as_ref(), args.log_path, commands);
        if measuring {
            ui.ctx().request_repaint_after(Duration::from_millis(200));
        }
    }

    /// "Clear selected…", the question that follows it, or how clearing is going.
    fn clear_controls(&mut self, ui: &mut Ui, clearing: bool, usage: Option<&Usage>, commands: &mut Vec<Command>) {
        if clearing {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(theme::size::MEDIUM));
                ui.label(RichText::new("Clearing…").color(theme::TEXT_WEAK));
            });
        } else if self.confirming {
            self.confirm_clear(ui, usage, commands);
        } else {
            let button = egui::Button::new(format!("{}  Clear selected…", icon::BROOM)).min_size(vec2(0.0, 28.0));
            if ui.add_enabled(!self.chosen.is_empty(), button).clicked() {
                self.confirming = true;
            }
        }
    }

    fn part_row(&mut self, ui: &mut Ui, part: Part, usage: Option<&Usage>, measuring: bool) {
        ui.horizontal(|ui| {
            let mut ticked = self.chosen.contains(&part);
            let label =
                RichText::new(part.label()).color(if part.in_user_data() { theme::TEXT } else { theme::TEXT_WEAK });
            if tick(ui, &mut ticked, label) {
                if ticked {
                    self.chosen.insert(part);
                } else {
                    self.chosen.remove(&part);
                }
                self.confirming = false;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                match usage.and_then(|usage| usage.parts.get(&part).map(|measured| (usage, measured))) {
                    Some((usage, measured)) => {
                        let count = measured.count as usize;
                        let count = match part {
                            Part::Thumbnails | Part::SheetChoices => plural(count, "picture"),
                            Part::Waveforms => plural(count, "sound"),
                            Part::ModelFacts => plural(count, "model"),
                            Part::Index => plural(count, "file"),
                            Part::Settings => format!("{} saved", grouped(count)),
                            Part::Groups => {
                                format!("{} · {}", plural(usage.groups as usize, "group"), plural(count, "asset"))
                            }
                            Part::LibraryFolders => plural(count, "folder"),
                        };
                        // Unknown when the file's statistics couldn't be read (it's in the log).
                        let size = measured.bytes.map_or_else(|| "—".to_owned(), format_size);
                        ui.label(RichText::new(size).monospace().color(theme::TEXT_WEAK));
                        ui.label(RichText::new(count).color(theme::TEXT_FAINT));
                    }
                    None if measuring => {
                        ui.add(egui::Spinner::new().size(theme::size::CAPTION));
                    }
                    None => {}
                }
            });
        })
        .response
        .on_hover_text(part.note());
    }

    fn confirm_clear(&mut self, ui: &mut Ui, usage: Option<&Usage>, commands: &mut Vec<Command>) {
        let chosen: Vec<Part> =
            Part::CACHE.into_iter().chain(Part::USER).filter(|part| self.chosen.contains(part)).collect();
        let bytes: u64 = chosen
            .iter()
            .filter_map(|part| usage.and_then(|usage| usage.parts.get(part)))
            .filter_map(|measured| measured.bytes)
            .sum();
        let yours = chosen.iter().any(|part| part.in_user_data());
        Frame::new().fill(theme::CARD).corner_radius(theme::RADIUS).inner_margin(Margin::same(12)).show(ui, |ui| {
            let names: Vec<&str> = chosen.iter().map(|part| part.label()).collect();
            ui.label(RichText::new(format!("Clear {}?", names.join(", "))).color(theme::TEXT));
            for part in &chosen {
                ui.label(
                    RichText::new(format!("{}: {}", part.label(), part.note()))
                        .size(theme::size::CAPTION)
                        .color(theme::TEXT_WEAK),
                );
            }
            if yours {
                ui.label(RichText::new("Your data can't be brought back once it's cleared.").color(theme::ERROR));
            }
            ui.horizontal(|ui| {
                let label = if bytes > 0 { format!("Clear ({})", format_size(bytes)) } else { "Clear".to_owned() };
                let fill = if yours { theme::ERROR } else { theme::ACCENT };
                let clear = egui::Button::new(RichText::new(label).color(theme::ON_ACCENT))
                    .fill(fill)
                    .min_size(vec2(0.0, 28.0));
                if ui.add(clear).clicked() {
                    commands.push(Command::Clear(chosen.clone()));
                    self.chosen.clear();
                    self.confirming = false;
                }
                if widgets::button(ui, "", "Cancel").clicked() {
                    self.confirming = false;
                }
            });
        });
    }
}

/// A file's name and size, a button to show it in its folder, and its path.
fn file_row(ui: &mut Ui, label: &str, path: &Path, size: Option<u64>, commands: &mut Vec<Command>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).font(FontId::new(theme::size::MEDIUM, theme::semibold())).color(theme::TEXT));
        if let Some(size) = size {
            ui.label(RichText::new(format_size(size)).color(theme::TEXT_WEAK));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::button(ui, icon::FOLDER_OPEN, "Show in folder").clicked() {
                commands.push(Command::Reveal(path.to_path_buf()));
            }
        });
    });
    ui.label(RichText::new(crate::platform::native(path)).size(theme::size::SMALL).color(theme::TEXT_FAINT));
}

/// The logs, each with its size and folder; the app's own before it's been measured.
fn log_rows(ui: &mut Ui, usage: Option<&Usage>, log_path: &Path, commands: &mut Vec<Command>) {
    widgets::section_label(ui, "Logs");
    match usage {
        Some(usage) => {
            for log in &usage.logs {
                file_row(ui, log.name, &log.path, log.bytes, commands);
                if log.name == "App log" {
                    note(ui, "Errors the app carried on after, and crashes. Attach it when reporting a problem.");
                }
            }
        }
        None => file_row(ui, "App log", log_path, None, commands),
    }
}
