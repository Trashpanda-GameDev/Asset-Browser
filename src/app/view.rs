//! What's browsed: the mode, the folder or group, the search and the order give the view (the
//! assets shown, in display order), and the selection within it.

use std::collections::HashSet;
use std::path::PathBuf;

use crate::asset::AssetKind;
use crate::groups::{Membership, UNCATEGORIZED};
use crate::library::SortBy;
use crate::ui::browse::BrowseOutput;
use crate::ui::mode::{Mode, ModeView};
use crate::ui::organize::Targets;

use super::BrowserApp;

/// What's browsed: the mode, the folder or group, the search, and how it's shown.
pub(super) struct Browsing {
    pub(super) mode: Mode,
    pub(super) modes: [ModeView; 4],
    /// Indices into `library.entries()` shown for the mode, folder and search, in display order.
    pub(super) view: Vec<u32>,
    pub(super) view_dirty: bool,
    pub(super) selected_dir: u32,
    /// Browsing one of the groups instead of the folder.
    pub(super) group: Option<i64>,
    pub(super) query: String,
    /// The membership changed by the user's hand: an asset that leaves the group being browsed
    /// hands the focus on to the next one.
    pub(super) follow_focus: bool,
    /// Assets per row and rows per screen in the current view, for the keyboard.
    pub(super) columns: usize,
    pub(super) page_rows: usize,
    /// The current view's scroll offset and height, last frame.
    pub(super) scroll: (f32, f32),
    pub(super) scroll_to_focus: bool,
}

impl Default for Browsing {
    fn default() -> Self {
        Self {
            mode: Mode::All,
            modes: Mode::ALL.map(ModeView::new),
            view: Vec::new(),
            view_dirty: false,
            selected_dir: 0,
            group: None,
            query: String::new(),
            follow_focus: false,
            columns: 1,
            page_rows: 1,
            scroll: (0.0, 0.0),
            scroll_to_focus: false,
        }
    }
}

impl Browsing {
    pub(super) fn mode_view(&self) -> &ModeView {
        &self.modes[self.mode.index()]
    }

    pub(super) fn mode_view_mut(&mut self) -> &mut ModeView {
        &mut self.modes[self.mode.index()]
    }

    /// Whether the browsed assets are in a group's own order (the order they were added) or
    /// ordered like a folder's. Uncategorized was never added to, so it's ordered like a folder.
    pub(super) fn in_group_order(&self) -> bool {
        self.group.is_some_and(|id| id != UNCATEGORIZED)
    }

    /// Browse a group instead of a folder, or go back to the folder.
    pub(super) fn show_group(&mut self, group: Option<i64>) {
        if self.group != group {
            self.group = group;
            self.view_dirty = true;
            self.scroll_to_focus = true;
        }
    }

    /// The selection in display order, focus first.
    pub(super) fn selected_in_order(&self) -> Vec<u32> {
        let selection = &self.mode_view().selection;
        let mut items: Vec<u32> = self.view.iter().copied().filter(|&index| selection.contains(index)).collect();
        if let Some(focus) = selection.focus {
            items.retain(|&index| index != focus);
            items.insert(0, focus);
        }
        items
    }

    /// Switches to `mode`; whether that changed anything.
    pub(super) fn set_mode(&mut self, mode: Mode) -> bool {
        if mode == self.mode {
            return false;
        }
        self.mode = mode;
        self.view_dirty = true;
        self.scroll_to_focus = true;
        true
    }
}

impl BrowserApp {
    pub(super) fn set_mode(&mut self, mode: Mode) {
        if self.browsing.set_mode(mode) {
            self.save_settings();
        }
    }

    /// Re-finds the groups' members in the library after either changed.
    pub(super) fn refresh_membership(&mut self) {
        if !self.organizing.membership_dirty {
            return;
        }
        self.organizing.membership_dirty = false;
        self.organizing.membership = match &self.library {
            Some(library) => Membership::build(library, &self.organizing.groups),
            None => Membership::default(),
        };
        if self.browsing.group.is_some_and(|id| id != UNCATEGORIZED && self.organizing.groups.get(id).is_none()) {
            self.browsing.group = None;
        }
        // Group views and searches (which match category names) both depend on it.
        self.browsing.view_dirty = true;
    }

    pub(super) fn rebuild_view(&mut self) {
        self.browsing.view_dirty = false;
        let follow_focus = std::mem::take(&mut self.browsing.follow_focus);
        let Some(library) = &self.library else {
            self.browsing.view.clear();
            return;
        };
        if self.browsing.selected_dir as usize >= library.dirs().len() {
            self.browsing.selected_dir = library.home();
        }
        let in_group_order = self.browsing.in_group_order();
        let query = self.browsing.query.to_lowercase();
        let words: Vec<&str> = query.split_whitespace().collect();
        let (groups, membership) = (&self.organizing.groups, &self.organizing.membership);
        let by_category = |index: u32, word: &str| membership.category_matches(groups, index, word);
        let kinds = self.browsing.mode.kinds();
        let mode_view = &mut self.browsing.modes[self.browsing.mode.index()];
        let (sort, descending) = mode_view.order(in_group_order);
        let mut view = match self.browsing.group {
            Some(id) => {
                let members = membership.entries.get(&id).map_or(&[][..], Vec::as_slice);
                let mut view: Vec<u32> = members
                    .iter()
                    .copied()
                    .filter(|&index| library.matches(index, kinds, &words, &by_category))
                    .collect();
                // The group's own order is the order things were added.
                if in_group_order && sort == SortBy::Added && descending {
                    view.reverse();
                }
                view
            }
            None => library.filter_with(self.browsing.selected_dir, kinds, &query, by_category),
        };
        if !(in_group_order && sort == SortBy::Added) {
            library.sort(&mut view, sort, descending);
        }
        let mut focus_moved = None;
        if !mode_view.selection.is_empty() {
            let shown: HashSet<u32> = view.iter().copied().collect();
            // Put in a category from Uncategorized, say: the next asset (or else the one before)
            // takes the focus, so working through a group keeps its place.
            let next =
                mode_view.selection.focus.filter(|focus| follow_focus && !shown.contains(focus)).and_then(|focus| {
                    let at = self.browsing.view.iter().position(|&index| index == focus)?;
                    self.browsing.view[at + 1..]
                        .iter()
                        .chain(self.browsing.view[..at].iter().rev())
                        .copied()
                        .find(|i| shown.contains(i))
                });
            mode_view.selection.retain(&shown);
            if let Some(next) = next
                && mode_view.selection.is_empty()
            {
                mode_view.selection.only(next);
                focus_moved = Some(next);
            }
        }
        self.browsing.view = view;
        if let Some(index) = focus_moved {
            self.focused(index);
            self.browsing.scroll_to_focus = true;
        }
    }

    pub(super) fn select_dir(&mut self, dir: u32) {
        self.browsing.selected_dir = dir;
        self.browsing.group = None;
        self.browsing.view_dirty = true;
        if let Some(library) = &self.library {
            self.chrome.folder_tree.reveal(library, dir);
        }
    }

    /// Counts of each kind in what's browsed: the folder or the group.
    pub(super) fn source_totals(&self) -> [u32; 3] {
        match (self.browsing.group, &self.library) {
            (Some(id), _) => self.organizing.membership.totals(id),
            (None, Some(library)) => {
                library.dirs().get(self.browsing.selected_dir as usize).map_or([0; 3], |dir| dir.totals)
            }
            (None, None) => [0; 3],
        }
    }

    pub(super) fn paths_of(&self, indices: &[u32]) -> Vec<PathBuf> {
        let Some(library) = &self.library else { return Vec::new() };
        indices.iter().map(|&index| library.path_of(library.entry(index))).collect()
    }

    pub(super) fn resolve(&self, targets: Targets) -> Vec<PathBuf> {
        match targets {
            Targets::Selection => self.paths_of(&self.browsing.selected_in_order()),
            Targets::Paths(paths) => paths,
        }
    }

    /// The focus moved to `index` (by click or key): a sound starts when auto-play is on, and the
    /// sounds after it are read into memory so moving on to them starts without a disk wait.
    pub(super) fn focused(&mut self, index: u32) {
        let Some(library) = &self.library else { return };
        if library.entry(index).kind == AssetKind::Audio && self.playback.autoplay {
            self.playback.play(library, index);
        }
        self.playback.read_ahead_around(library, &self.browsing.view, index);
    }

    pub(super) fn apply_browse(&mut self, output: BrowseOutput) {
        if output.columns > 0 {
            self.browsing.columns = output.columns;
            self.browsing.page_rows = output.page_rows;
        }
        let view = &self.browsing.view;
        let selection = &mut self.browsing.modes[self.browsing.mode.index()].selection;
        if let Some((index, modifiers)) = output.clicked {
            if modifiers.command {
                selection.toggle(index);
            } else if modifiers.shift {
                selection.extend_to(view, index);
            } else {
                selection.only(index);
            }
            if selection.focus == Some(index) && !modifiers.shift {
                self.focused(index);
            }
        }
        let selection = &mut self.browsing.modes[self.browsing.mode.index()].selection;
        if let Some(index) = output.secondary_clicked
            && !selection.contains(index)
        {
            selection.only(index);
        }
        if let Some(index) = output.double_clicked {
            self.open_entries(&[index]);
        }
        if let Some(action) = output.menu {
            self.run_action(action);
        }
        for command in output.commands {
            self.run_group_command(command);
        }
        if let Some(sort) = output.sort_clicked {
            let in_group = self.browsing.in_group_order();
            let view = self.browsing.mode_view_mut();
            let (current, descending) = view.order(in_group);
            if current == sort {
                view.set_order(in_group, sort, !descending);
            } else {
                view.set_order(in_group, sort, sort.starts_descending());
            }
            self.browsing.view_dirty = true;
        }
    }

    /// Shows and selects `index`, switching mode, folder and search as needed.
    pub(super) fn reveal(&mut self, index: u32) {
        let Some(library) = &self.library else { return };
        let entry = library.entry(index);
        let (kind, dir) = (entry.kind, entry.dir);
        let in_folder = library.dir_chain(dir).contains(&self.browsing.selected_dir);
        let query = self.browsing.query.to_lowercase();
        let matches = query.split_whitespace().all(|word| entry.lower.contains(word));
        if !self.browsing.mode.kinds().contains(kind) {
            self.set_mode(Mode::ALL.into_iter().find(|mode| mode.kind() == Some(kind)).unwrap_or(Mode::All));
        }
        if !matches {
            self.browsing.query.clear();
        }
        if !in_folder {
            self.select_dir(dir);
        }
        self.browsing.view_dirty = true;
        self.browsing.mode_view_mut().selection.only(index);
        self.browsing.scroll_to_focus = true;
    }
}
