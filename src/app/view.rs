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

impl BrowserApp {
    pub(super) fn mode_view(&self) -> &ModeView {
        &self.modes[self.mode.index()]
    }

    pub(super) fn mode_view_mut(&mut self) -> &mut ModeView {
        &mut self.modes[self.mode.index()]
    }

    /// Re-finds the groups' members in the library after either changed.
    pub(super) fn refresh_membership(&mut self) {
        if !self.membership_dirty {
            return;
        }
        self.membership_dirty = false;
        self.membership = match &self.library {
            Some(library) => Membership::build(library, &self.groups),
            None => Membership::default(),
        };
        if self.group.is_some_and(|id| id != UNCATEGORIZED && self.groups.get(id).is_none()) {
            self.group = None;
        }
        // Group views and searches (which match category names) both depend on it.
        self.view_dirty = true;
    }

    /// Whether the browsed assets are in a group's own order (the order they were added) or
    /// ordered like a folder's. Uncategorized was never added to, so it's ordered like a folder.
    pub(super) fn in_group_order(&self) -> bool {
        self.group.is_some_and(|id| id != UNCATEGORIZED)
    }

    pub(super) fn rebuild_view(&mut self) {
        self.view_dirty = false;
        let follow_focus = std::mem::take(&mut self.follow_focus);
        let Some(library) = &self.library else {
            self.view.clear();
            return;
        };
        if self.selected_dir as usize >= library.dirs.len() {
            self.selected_dir = library.home();
        }
        let in_group_order = self.in_group_order();
        let query = self.query.to_lowercase();
        let words: Vec<&str> = query.split_whitespace().collect();
        let (groups, membership) = (&self.groups, &self.membership);
        let by_category = |index: u32, word: &str| membership.category_matches(groups, index, word);
        let kinds = self.mode.kinds();
        let mode_view = &mut self.modes[self.mode.index()];
        let (sort, descending) = mode_view.order(in_group_order);
        let mut view = match self.group {
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
            None => library.filter_with(self.selected_dir, kinds, &query, by_category),
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
                    let at = self.view.iter().position(|&index| index == focus)?;
                    self.view[at + 1..].iter().chain(self.view[..at].iter().rev()).copied().find(|i| shown.contains(i))
                });
            mode_view.selection.retain(&shown);
            if let Some(next) = next
                && mode_view.selection.is_empty()
            {
                mode_view.selection.only(next);
                focus_moved = Some(next);
            }
        }
        self.view = view;
        if let Some(index) = focus_moved {
            self.focused(index);
            self.scroll_to_focus = true;
        }
    }

    /// Browse a group instead of a folder, or go back to the folder.
    pub(super) fn show_group(&mut self, group: Option<i64>) {
        if self.group != group {
            self.group = group;
            self.view_dirty = true;
            self.scroll_to_focus = true;
        }
    }

    pub(super) fn set_mode(&mut self, mode: Mode) {
        if mode == self.mode {
            return;
        }
        self.mode = mode;
        self.view_dirty = true;
        self.scroll_to_focus = true;
        self.save_settings();
    }

    pub(super) fn select_dir(&mut self, dir: u32) {
        self.selected_dir = dir;
        self.group = None;
        self.view_dirty = true;
        if let Some(library) = &self.library {
            self.folders.reveal(library, dir);
        }
    }

    /// Counts of each kind in what's browsed: the folder or the group.
    pub(super) fn source_totals(&self) -> [u32; 3] {
        match (self.group, &self.library) {
            (Some(id), _) => self.membership.totals(id),
            (None, Some(library)) => library.dirs.get(self.selected_dir as usize).map_or([0; 3], |dir| dir.totals),
            (None, None) => [0; 3],
        }
    }

    pub(super) fn paths_of(&self, indices: &[u32]) -> Vec<PathBuf> {
        let Some(library) = &self.library else { return Vec::new() };
        indices.iter().map(|&index| library.path_of(&library.entries[index as usize])).collect()
    }

    pub(super) fn resolve(&self, targets: Targets) -> Vec<PathBuf> {
        match targets {
            Targets::Selection => self.paths_of(&self.selected_in_order()),
            Targets::Paths(paths) => paths,
        }
    }

    /// The focus moved to `index` (by click or key): a sound starts when auto-play is on, and the
    /// sounds after it are read into memory so moving on to them starts without a disk wait.
    pub(super) fn focused(&mut self, index: u32) {
        let Some(library) = &self.library else { return };
        if library.entries[index as usize].kind == AssetKind::Audio && self.sound.autoplay {
            self.sound.play(library, index);
        }
        self.sound.read_ahead_around(library, &self.view, index);
    }

    pub(super) fn apply_browse(&mut self, output: BrowseOutput) {
        if output.columns > 0 {
            self.columns = output.columns;
            self.page_rows = output.page_rows;
        }
        let view = &self.view;
        let selection = &mut self.modes[self.mode.index()].selection;
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
        let selection = &mut self.modes[self.mode.index()].selection;
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
            let in_group = self.in_group_order();
            let view = self.mode_view_mut();
            let (current, descending) = view.order(in_group);
            if current == sort {
                view.set_order(in_group, sort, !descending);
            } else {
                view.set_order(in_group, sort, sort.starts_descending());
            }
            self.view_dirty = true;
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

    /// Shows and selects `index`, switching mode, folder and search as needed.
    pub(super) fn reveal(&mut self, index: u32) {
        let Some(library) = &self.library else { return };
        let entry = &library.entries[index as usize];
        let (kind, dir) = (entry.kind, entry.dir);
        let in_folder = library.dir_chain(dir).contains(&self.selected_dir);
        let query = self.query.to_lowercase();
        let matches = query.split_whitespace().all(|word| entry.lower.contains(word));
        if !self.mode.kinds()[kind.index()] {
            self.set_mode(Mode::ALL.into_iter().find(|mode| mode.kind() == Some(kind)).unwrap_or(Mode::All));
        }
        if !matches {
            self.query.clear();
        }
        if !in_folder {
            self.select_dir(dir);
        }
        self.view_dirty = true;
        self.mode_view_mut().selection.only(index);
        self.scroll_to_focus = true;
    }
}
