//! What the screenshots tool (`src/bin/shots`) drives: the settings, "What's new" and a view set up
//! for each picture.

use crate::groups::{self, UNCATEGORIZED};
use crate::library::{Library, Order, SortBy};
use crate::ui::mode::{Layout as ViewLayout, Mode};
use crate::ui::whats_new::WhatsNew;

use super::BrowserApp;

/// What `BrowserApp::show_view` shows, for the screenshots tool (`src/bin/shots`).
#[derive(Clone, Copy)]
pub struct ScreenshotView {
    pub mode: Mode,
    pub layout: ViewLayout,
    /// The order, or else the mode's own.
    pub sort: Option<SortBy>,
    /// The tiles' size, or else the mode's own.
    pub tile: Option<f32>,
    /// A collection or category by name, or else the whole library.
    pub group: Option<&'static str>,
    /// A folder of the library's first, by its path in it ("Items"), instead of a group.
    pub folder: Option<&'static str>,
    /// What's in the search box.
    pub query: &'static str,
    /// The asset whose path ends with this, selected.
    pub file: Option<&'static str>,
}

impl ScreenshotView {
    pub const fn new(mode: Mode, layout: ViewLayout) -> Self {
        Self { mode, layout, sort: None, tile: None, group: None, folder: None, query: "", file: None }
    }
}

impl BrowserApp {
    /// Opens the settings at `page`; `None` closes them. (Also for `src/bin/shots.rs`.)
    pub fn show_settings(&mut self, page: Option<crate::ui::settings::Page>) {
        match page {
            Some(page) => self.preferences.settings.open_at(page),
            None => self.preferences.settings.close(),
        }
    }

    /// Shows "What's new": every version's notes, or as after an update, the running version's.
    /// (Also for `src/bin/shots.rs`.)
    pub fn show_whats_new(&mut self, every_version: bool) {
        let Ok(current) = semver::Version::parse(&self.updates.version()) else { return };
        self.preferences.whats_new = Some(if every_version {
            WhatsNew::every_version(current)
        } else {
            WhatsNew::since(semver::Version::new(0, 0, 0), current)
        });
    }

    pub fn hide_whats_new(&mut self) {
        self.preferences.whats_new = None;
    }

    /// Shows what `view` says, with its asset selected but not played. For `src/bin/shots`.
    pub fn show_view(&mut self, view: ScreenshotView) {
        self.set_mode(view.mode);
        self.browsing.mode_view_mut().layout = view.layout;
        if let Some(tile) = view.tile {
            self.browsing.mode_view_mut().tile = tile.clamp(64.0, 256.0);
        }
        let group = view.group.and_then(|name| match name {
            groups::UNCATEGORIZED_NAME => Some(UNCATEGORIZED),
            name => self.organizing.groups.all().iter().find(|group| group.name == name).map(|group| group.id),
        });
        let folder = (self.library.as_ref().zip(view.folder)).and_then(|(library, folder)| {
            let lower = folder.to_lowercase();
            library.dirs().iter().position(|dir| dir.root == Some(0) && dir.lower == lower)
        });
        match (group, folder) {
            (Some(id), _) => self.browsing.show_group(Some(id)),
            (None, Some(dir)) => self.select_dir(dir as u32),
            (None, None) => self.select_dir(self.library.as_ref().map_or(0, Library::home)),
        }
        if self.browsing.query != view.query {
            view.query.clone_into(&mut self.browsing.query);
        }
        if let Some(sort) = view.sort {
            let in_group = self.browsing.in_group_order();
            self.browsing.mode_view_mut().set_order(in_group, Order { by: sort, descending: false });
        }
        let found = (self.library.as_ref().zip(view.file))
            .and_then(|(library, file)| library.entries().iter().position(|entry| entry.rel.ends_with(file)));
        let selection = &mut self.browsing.mode_view_mut().selection;
        match found {
            Some(index) => selection.only(index as u32),
            None => selection.clear(),
        }
        self.browsing.view_dirty = true;
        self.browsing.scroll_to_focus = true;
    }
}
