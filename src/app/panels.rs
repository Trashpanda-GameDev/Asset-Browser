//! The rest of the window: the browsed assets in the middle, the preview pane on the right, the
//! player at the bottom when the pane is hidden, and the status bar.

use std::time::Duration;

use eframe::egui::{self, Align, Frame, Layout, Margin, RichText};
use egui_phosphor::regular as icon;

use crate::asset::AssetKind;
use crate::groups::{GroupKind, UNCATEGORIZED};
use crate::ui::browse::{self, Browse};
use crate::ui::folders::FolderAction;
use crate::ui::format::grouped;
use crate::ui::inspector::InspectorArgs;
use crate::ui::mode::Layout as ViewLayout;
use crate::ui::organize::plural;
use crate::ui::sound;
use crate::ui::updates::UpdateAction;
use crate::ui::{theme, widgets};
use crate::{diag, paths};

use super::BrowserApp;

impl BrowserApp {
    #[expect(clippy::too_many_lines, reason = "refactor: split into named steps")]
    pub(super) fn status_bar(&mut self, ui: &mut egui::Ui) {
        let frame = Frame::new().fill(theme::PANEL).inner_margin(Margin::symmetric(12, 4));
        let mut show = None;
        let mut update = None;
        egui::Panel::bottom("status").frame(frame).show(ui, |ui| {
            ui.horizontal(|ui| {
                let small = |text: String| RichText::new(text).size(11.5).color(theme::TEXT_FAINT);
                if let Some(error) = diag::last() {
                    ui.label(
                        RichText::new(format!("{}  {error}", icon::WARNING_CIRCLE)).size(11.5).color(theme::ERROR),
                    )
                    .on_hover_text(format!("Logged to {}", paths::log_file().display()));
                    widgets::divider(ui);
                }
                if let Some(library) = &self.library {
                    ui.label(small(format!("{} assets", grouped(library.entries().len()))));
                    widgets::divider(ui);
                }
                if self.deleting.is_some() {
                    ui.add(egui::Spinner::new().size(12.0));
                    ui.label(small("Deleting…".to_owned()));
                    widgets::divider(ui);
                }
                if self.scans > 0 {
                    ui.add(egui::Spinner::new().size(12.0));
                    ui.label(small(format!("Scanning… {:.1} s", self.scan_started.elapsed().as_secs_f32())));
                    ui.ctx().request_repaint_after(Duration::from_millis(100));
                } else if let Some(scan) = &self.last_scan {
                    ui.label(small(format!(
                        "Scanned {} in {:.2} s",
                        plural(scan.dirs_visited, "folder"),
                        scan.elapsed.as_secs_f32()
                    )))
                    .on_hover_text(format!(
                        "{} assets; {} other files skipped. {}",
                        grouped(scan.assets),
                        grouped(scan.skipped),
                        self.index_note.as_deref().unwrap_or("")
                    ));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    update = crate::ui::updates::status(ui, &self.updates);
                    widgets::divider(ui);
                    let stats = &self.thumbs.stats;
                    let made = stats.made.load(std::sync::atomic::Ordering::Relaxed);
                    let micros = stats.make_micros.load(std::sync::atomic::Ordering::Relaxed);
                    let from_cache = stats.from_cache.load(std::sync::atomic::Ordering::Relaxed);
                    let failed = stats.failed.load(std::sync::atomic::Ordering::Relaxed);
                    let (queued, busy) = self.thumbs.pending();
                    let average = if made > 0 { micros as f64 / made as f64 / 1000.0 } else { 0.0 };
                    let details = format!(
                        "{} made ({average:.1} ms average on {} threads), {} from the cache, {} couldn't be read",
                        grouped(made as usize),
                        self.thumbs.thread_count(),
                        grouped(from_cache as usize),
                        grouped(failed as usize),
                    );
                    if queued + busy > 0 {
                        ui.label(small(format!("Making previews… {}", grouped(queued + busy)))).on_hover_text(details);
                        ui.add(egui::Spinner::new().size(12.0));
                    } else {
                        ui.label(small(format!("{} previews", grouped((made + from_cache) as usize))))
                            .on_hover_text(details);
                    }
                    // The playing sound, when it isn't the one shown.
                    let focus = self.modes[self.mode.index()].selection.focus;
                    let playing_elsewhere = self.sound.playing.is_some_and(|(index, _)| Some(index) != focus);
                    if playing_elsewhere && let Some(library) = &self.library {
                        widgets::divider(ui);
                        show = sound::now_playing_chip(ui, &mut self.sound, library);
                    }
                });
            });
        });
        if let Some(index) = show {
            self.reveal(index);
        }
        match update {
            Some(UpdateAction::Check) => self.updates.check_now(),
            Some(UpdateAction::Restart) => self.restart_to_update(),
            None => {}
        }
    }

    pub(super) fn inspector_panel(&mut self, ui: &mut egui::Ui) {
        let frame = Frame::new().fill(theme::PANEL).inner_margin(Margin::same(12));
        let mut output = None;
        egui::Panel::right("inspector")
            .resizable(true)
            .default_size(360.0)
            .size_range(260.0..=720.0)
            .frame(frame)
            .show_collapsible(ui, &mut self.show_inspector, |ui| {
                let Some(library) = &self.library else {
                    ui.label(RichText::new("Open a folder to see its assets here.").color(theme::TEXT_FAINT));
                    return;
                };
                let totals = match self.group {
                    Some(id) => self.membership.totals(id),
                    None => library.dirs().get(self.selected_dir as usize).map_or([0; 3], |dir| dir.totals),
                };
                let args = InspectorArgs {
                    library,
                    selection: &self.modes[self.mode.index()].selection,
                    previews: &mut self.previews,
                    sound: &mut self.sound,
                    render_state: self.render_state.as_ref(),
                    totals,
                    kinds: self.mode.kinds(),
                    groups: &self.groups,
                    membership: &self.membership,
                };
                output = Some(self.inspector.show(ui, args));
            });
        let Some(output) = output else { return };
        if let Some(command) = output.command {
            self.run_group_command(command);
        }
        if let Some(index) = output.focus {
            let selection = &mut self.mode_view_mut().selection;
            selection.focus = Some(index);
            selection.anchor = Some(index);
            self.focused(index);
            self.scroll_to_focus = true;
        }
        if let Some(action) = output.action {
            self.run_action(action);
        }
    }

    /// The sound controls at the bottom, for when the preview pane is hidden.
    pub(super) fn player_bar(&mut self, ui: &mut egui::Ui) {
        let Some(library) = &self.library else { return };
        let focus = self.mode_view().selection.focus.filter(|&index| library.entry(index).kind == AssetKind::Audio);
        let Some(index) = focus.or(self.sound.playing.map(|(index, _)| index)) else { return };
        if index as usize >= library.entries().len() {
            return;
        }
        let frame = Frame::new().fill(theme::PANEL).inner_margin(Margin::symmetric(12, 8));
        egui::Panel::bottom("player").frame(frame).show(ui, |ui| {
            let entry = library.entry(index);
            let waveform = self.previews.waveforms.get(&entry.key).map(|waveform| waveform.as_ref());
            ui.label(RichText::new(entry.name()).font(egui::FontId::new(13.0, theme::semibold())));
            sound::scrubber(
                ui,
                56.0,
                &mut self.sound,
                library,
                index,
                waveform,
                self.previews.failed.contains_key(&entry.key),
            );
            let duration = waveform.map(|waveform| waveform.duration as f64);
            sound::transport(ui, &mut self.sound, library, index, duration);
        });
    }

    #[expect(clippy::too_many_lines, reason = "refactor: split into named steps")]
    pub(super) fn central(&mut self, ui: &mut egui::Ui) {
        let frame = Frame::new().fill(theme::BG).inner_margin(Margin { left: 12, right: 6, top: 6, bottom: 0 });
        let mut output = None;
        let mut add_folder = false;
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            if self.library.is_none() {
                // No folders yet.
                ui.vertical_centered(|ui| {
                    ui.add_space((ui.available_height() / 2.0 - 80.0).max(0.0));
                    ui.label(RichText::new(icon::FOLDERS).size(44.0).color(theme::TEXT_FAINT));
                    ui.add_space(10.0);
                    ui.label(RichText::new("Add a folder to browse its assets").size(16.0).color(theme::TEXT_WEAK));
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("Or drop one on the window. Add as many as you like: they're browsed together.")
                            .size(12.5)
                            .color(theme::TEXT_FAINT),
                    );
                    ui.add_space(14.0);
                    add_folder = widgets::button(ui, icon::FOLDER_PLUS, "Add a folder…").clicked();
                });
                return;
            }
            self.toolbar(ui);
            let Some(library) = self.library.as_ref() else { return };
            if self.view.is_empty() {
                // A folder that's empty only because its first scan hasn't finished.
                let scanning = self.scans > 0
                    && self.group.is_none()
                    && library.dirs().get(self.selected_dir as usize).is_some_and(|dir| dir.totals == [0; 3]);
                ui.centered_and_justified(|ui| {
                    let what = self.mode.kind().map_or("assets", |kind| match kind {
                        AssetKind::Image => "images",
                        AssetKind::Audio => "sounds",
                        AssetKind::Model => "models",
                    });
                    let place = match self.group {
                        Some(UNCATEGORIZED) => "without a category".to_owned(),
                        Some(id) => match self.groups.get(id) {
                            Some(group) if group.kind == GroupKind::Favorites => {
                                "in Favorites yet. Star assets with F or the star on their tile".to_owned()
                            }
                            Some(group) => {
                                format!("in \u{201c}{}\u{201d} yet. Drag assets onto it in the sidebar", group.name)
                            }
                            None => "here".to_owned(),
                        },
                        None => "in this folder".to_owned(),
                    };
                    let text = if scanning {
                        "Scanning…".to_owned()
                    } else if self.query.trim().is_empty() {
                        format!("No {what} {place}.")
                    } else {
                        format!("No {what} here match \u{201c}{}\u{201d}.", self.query.trim())
                    };
                    ui.add(egui::Label::new(RichText::new(text).size(14.0).color(theme::TEXT_FAINT)).wrap());
                });
                return;
            }
            let mode_view = &self.modes[self.mode.index()];
            let scroll_to = std::mem::take(&mut self.scroll_to_focus)
                .then_some(mode_view.selection.focus)
                .flatten()
                .and_then(|focus| self.view.iter().position(|&index| index == focus));
            let now = self.sound.now();
            let (sort, descending) = mode_view.order(self.in_group_order());
            let browse = Browse {
                library,
                view: &self.view,
                selection: &mode_view.selection,
                now: now.as_ref(),
                mode: self.mode,
                sort,
                descending,
                backdrop: self.inspector.image.backdrop,
                scroll_to,
                groups: &self.groups,
                membership: &self.membership,
                viewing: self.group,
            };
            output = Some(match mode_view.layout {
                ViewLayout::Grid => browse::grid(ui, &browse, &mut self.previews, mode_view.tile, &mut self.scroll),
                ViewLayout::List => browse::list(ui, &browse, &mut self.previews, &mut self.scroll),
            });
        });
        if let Some(output) = output {
            self.apply_browse(output);
        }
        if add_folder {
            self.run_folder_action(FolderAction::AddRoot);
        }
    }
}
