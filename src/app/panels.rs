//! The rest of the window: the browsed assets in the middle, the preview pane on the right, the
//! player at the bottom when the pane is hidden, and the status bar.

use eframe::egui::{self, Frame, Margin, RichText};

use crate::asset::AssetKind;
use crate::diag;
use crate::ui::browse::{self, Browse, BrowseOutput};
use crate::ui::empty::{self, EmptyArgs};
use crate::ui::folders::FolderAction;
use crate::ui::inspector::InspectorArgs;
use crate::ui::mode::Layout as ViewLayout;
use crate::ui::sound;
use crate::ui::status_bar::{self, StatusArgs, StatusCommand};
use crate::ui::theme;

use super::BrowserApp;

impl BrowserApp {
    pub(super) fn status_bar(&mut self, ui: &mut egui::Ui) {
        let args = StatusArgs {
            error: diag::last(),
            library: self.library.as_ref(),
            deleting: self.deletion.deleting.is_some(),
            scanning_since: (self.folders.scans > 0).then_some(self.folders.scan_started),
            last_scan: self.folders.last_scan.as_ref(),
            index_note: self.folders.index_note.as_deref(),
            previews: self.thumbs.progress(),
            updates: &self.updates,
            focus: self.browsing.mode_view().selection.focus,
            playback: &self.playback,
        };
        for command in status_bar::show(ui, &args) {
            match command {
                StatusCommand::Update(action) => self.run_update_action(action),
                StatusCommand::Sound(command) => self.run_sound_command(command),
                StatusCommand::ShowPlaying(index) => self.reveal(index),
            }
        }
    }

    pub(super) fn inspector_panel(&mut self, ui: &mut egui::Ui) {
        let frame = Frame::new().fill(theme::PANEL).inner_margin(Margin::same(12));
        let totals = self.source_totals();
        let mut output = None;
        egui::Panel::right("inspector")
            .resizable(true)
            .default_size(360.0)
            .size_range(260.0..=720.0)
            .frame(frame)
            .show_collapsible(ui, &mut self.chrome.show_inspector, |ui| {
                let Some(library) = &self.library else {
                    ui.label(RichText::new("Open a folder to see its assets here.").color(theme::TEXT_FAINT));
                    return;
                };
                let selection = &self.browsing.modes[self.browsing.mode.index()].selection;
                let args = InspectorArgs {
                    library,
                    selection,
                    previews: &mut self.previews,
                    playback: &self.playback,
                    render_state: self.render_state.as_ref(),
                    totals,
                    kinds: self.browsing.mode.kinds(),
                    groups: &self.organizing.groups,
                    membership: &self.organizing.membership,
                    sheet_choices: self.organizing.sheets.of_focus(library, selection.focus),
                };
                output = Some(self.inspector.show(ui, args));
            });
        let Some(output) = output else { return };
        if let Some(index) = output.load_detail
            && let Some(library) = &self.library
        {
            let entry = library.entry(index);
            self.detail.request(entry.key, library.path_of(entry), entry.kind);
        }
        if let Some((key, choices)) = output.sheet_choices {
            self.organizing.sheets.save(key, choices);
        }
        if let Some(command) = output.command {
            self.run_group_command(command);
        }
        if let Some(command) = output.sound {
            self.run_sound_command(command);
        }
        if let Some(index) = output.focus {
            let selection = &mut self.browsing.mode_view_mut().selection;
            selection.focus = Some(index);
            selection.anchor = Some(index);
            self.focused(index);
            self.browsing.scroll_to_focus = true;
        }
        if let Some(action) = output.action {
            self.run_action(action);
        }
    }

    /// Hands the pane the large previews that finished loading.
    pub(super) fn poll_detail(&mut self, ctx: &egui::Context) {
        while let Some(loaded) = self.detail.try_recv() {
            self.inspector.take_detail(loaded, ctx);
        }
    }

    /// The sound controls at the bottom, for when the preview pane is hidden.
    pub(super) fn player_bar(&mut self, ui: &mut egui::Ui) {
        let Some(library) = &self.library else { return };
        let focus =
            self.browsing.mode_view().selection.focus.filter(|&index| library.entry(index).kind == AssetKind::Audio);
        let Some(index) = focus.or(self.playback.playing.map(|(index, _)| index)) else { return };
        if index as usize >= library.entries().len() {
            return;
        }
        let frame = Frame::new().fill(theme::PANEL).inner_margin(Margin::symmetric(12, 8));
        let mut command = None;
        egui::Panel::bottom("player").frame(frame).show(ui, |ui| {
            let entry = library.entry(index);
            let waveform = self.previews.waveforms.get(&entry.key).map(|waveform| waveform.as_ref());
            let failed = self.previews.failed.contains_key(&entry.key);
            ui.label(RichText::new(entry.name()).font(egui::FontId::new(13.0, theme::semibold())));
            command = sound::scrubber(ui, 56.0, &self.playback, library, index, waveform, failed);
            let duration = waveform.map(|waveform| waveform.duration as f64);
            command = command.or(sound::transport(ui, &self.playback, library, index, duration));
        });
        if let Some(command) = command {
            self.run_sound_command(command);
        }
    }

    /// The middle: the toolbar over the browsed assets, or the welcome before any folder is added.
    pub(super) fn central(&mut self, ui: &mut egui::Ui) {
        let frame = Frame::new().fill(theme::BG).inner_margin(Margin { left: 12, right: 6, top: 6, bottom: 0 });
        let mut output = None;
        let mut add_folder = false;
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            if self.library.is_none() {
                add_folder = empty::welcome(ui);
                return;
            }
            self.toolbar(ui);
            if self.browsing.view.is_empty() {
                self.nothing_here(ui);
            } else {
                output = self.browse(ui);
            }
        });
        if let Some(output) = output {
            self.apply_browse(output);
        }
        if add_folder {
            self.run_folder_action(FolderAction::AddRoot);
        }
    }

    /// Why the view is empty.
    fn nothing_here(&self, ui: &mut egui::Ui) {
        let Some(library) = &self.library else { return };
        // A folder that's empty only because its first scan hasn't finished.
        let scanning = self.folders.scans > 0
            && self.browsing.group.is_none()
            && library.dirs().get(self.browsing.selected_dir as usize).is_some_and(|dir| dir.totals == [0; 3]);
        let args = EmptyArgs {
            kind: self.browsing.mode.kind(),
            group: self.browsing.group,
            groups: &self.organizing.groups,
            query: &self.browsing.query,
            scanning,
        };
        empty::nothing_here(ui, &args);
    }

    /// The browsed assets, as a grid or a list.
    fn browse(&mut self, ui: &mut egui::Ui) -> Option<BrowseOutput> {
        let library = self.library.as_ref()?;
        let mode_view = &self.browsing.modes[self.browsing.mode.index()];
        let scroll_to = std::mem::take(&mut self.browsing.scroll_to_focus)
            .then_some(mode_view.selection.focus)
            .flatten()
            .and_then(|focus| self.browsing.view.iter().position(|&index| index == focus));
        let now = self.playback.now();
        let (sort, descending) = mode_view.order(self.browsing.in_group_order());
        let browse = Browse {
            library,
            view: &self.browsing.view,
            selection: &mode_view.selection,
            now: now.as_ref(),
            mode: self.browsing.mode,
            sort,
            descending,
            backdrop: self.inspector.image.backdrop,
            scroll_to,
            groups: &self.organizing.groups,
            membership: &self.organizing.membership,
            viewing: self.browsing.group,
        };
        Some(match mode_view.layout {
            ViewLayout::Grid => {
                browse::grid(ui, &browse, &mut self.previews, mode_view.tile, &mut self.browsing.scroll)
            }
            ViewLayout::List => browse::list(ui, &browse, &mut self.previews, &mut self.browsing.scroll),
        })
    }
}
