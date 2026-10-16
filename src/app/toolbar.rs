//! Above the browsed assets (`ui::toolbar`), and what's asked for there.

use eframe::egui;

use crate::ui::toolbar::{self, ToolbarArgs, ToolbarCommand};

use super::BrowserApp;

impl BrowserApp {
    /// Above the assets: where you are, how many there are, and how they're shown.
    pub(super) fn toolbar(&mut self, ui: &mut egui::Ui) {
        let Some(library) = &self.library else { return };
        let args = ToolbarArgs {
            library,
            selected_dir: self.browsing.selected_dir,
            group: self.browsing.group,
            groups: &self.organizing.groups,
            membership: &self.organizing.membership,
            mode: self.browsing.mode,
            mode_view: self.browsing.mode_view(),
            in_group_order: self.browsing.in_group_order(),
            shown: self.browsing.view.len(),
            autoplay: self.playback.autoplay,
            display: self.browsing.display,
        };
        for command in toolbar::show(ui, &args) {
            match command {
                ToolbarCommand::GoTo(dir) => self.select_dir(dir),
                ToolbarCommand::Group(command) => self.run_group_command(command),
                ToolbarCommand::Order(order) => self.browsing.set_order(order),
                ToolbarCommand::Tile(size) => self.browsing.mode_view_mut().tile = size,
                ToolbarCommand::Autoplay(on) => self.playback.autoplay = on,
                ToolbarCommand::Display(display) => self.browsing.display = display,
            }
        }
    }
}
