//! The window's top (`ui::header`), and what's asked for there.

use eframe::egui;

use crate::ui::header::{self, HeaderArgs, HeaderCommand};

use super::BrowserApp;

impl BrowserApp {
    pub(super) fn header(&mut self, ui: &mut egui::Ui) {
        let args = HeaderArgs {
            roots: &self.folders.roots,
            library: self.library.as_ref(),
            totals: self.library.as_ref().map(|_| self.source_totals()),
            mode: self.browsing.mode,
            layout: self.browsing.mode_view().layout,
            settings_open: self.preferences.settings.is_open(),
            pane_shown: self.chrome.show_inspector,
            query: &mut self.browsing.query,
            focus_search: std::mem::take(&mut self.chrome.focus_search),
        };
        for command in header::show(ui, args) {
            match command {
                HeaderCommand::Folder(action) => self.run_folder_action(action),
                HeaderCommand::Mode(mode) => self.set_mode(mode),
                HeaderCommand::OpenSettings => self.preferences.settings.open(),
                HeaderCommand::TogglePane => self.chrome.show_inspector = !self.chrome.show_inspector,
                HeaderCommand::Layout(layout) => self.browsing.set_layout(layout),
                HeaderCommand::Searched => self.browsing.view_dirty = true,
            }
        }
    }
}
