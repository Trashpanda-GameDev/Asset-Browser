//! Modes: what kind of asset is being browsed. Each mode keeps its own layout, tile size, order and
//! selection, and brings its own view options, list columns and inspector; that's where
//! kind-specific tools go as they're added.

use std::collections::{HashMap, HashSet};

use eframe::egui::Color32;
use egui_phosphor::regular as icon;

use crate::asset::{AssetKind, KindSet};
use crate::library::SortBy;

use super::keymap::Action;
use super::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    All,
    Images,
    Sounds,
    Models,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Self::All, Self::Images, Self::Sounds, Self::Models];

    pub fn index(self) -> usize {
        match self {
            Self::All => 0,
            Self::Images => 1,
            Self::Sounds => 2,
            Self::Models => 3,
        }
    }

    /// The one kind this mode browses (`None` for All).
    pub fn kind(self) -> Option<AssetKind> {
        match self {
            Self::All => None,
            Self::Images => Some(AssetKind::Image),
            Self::Sounds => Some(AssetKind::Audio),
            Self::Models => Some(AssetKind::Model),
        }
    }

    pub fn kinds(self) -> KindSet {
        self.kind().map_or(KindSet::ALL, KindSet::only)
    }

    pub fn label(self) -> &'static str {
        match self.kind() {
            None => "All",
            Some(kind) => kind.label(),
        }
    }

    /// What it shows, for its button's tooltip.
    pub fn description(self) -> &'static str {
        match self {
            Self::All => "Everything",
            Self::Images => "Pictures and textures",
            Self::Sounds => "Sounds, with a player",
            Self::Models => "3D models, with a 3D view",
        }
    }

    /// The shortcut that switches to it.
    pub fn shortcut(self) -> Action {
        match self {
            Self::All => Action::ModeAll,
            Self::Images => Action::ModeImages,
            Self::Sounds => Action::ModeSounds,
            Self::Models => Action::ModeModels,
        }
    }

    pub fn glyph(self) -> &'static str {
        match self.kind() {
            None => icon::SQUARES_FOUR,
            Some(kind) => theme::kind_icon(kind),
        }
    }

    pub fn color(self) -> Color32 {
        self.kind().map_or(theme::ACCENT, theme::kind_color)
    }

    /// Name in the settings table.
    pub fn key(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Images => "images",
            Self::Sounds => "sounds",
            Self::Models => "models",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.key() == key)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Grid,
    List,
}

/// What each mode remembers.
pub struct ModeView {
    pub layout: Layout,
    pub tile: f32,
    /// The order in folders.
    pub sort: SortBy,
    pub descending: bool,
    /// The order in Favorites, collections and categories: newest first unless changed.
    pub group_sort: SortBy,
    pub group_descending: bool,
    pub selection: Selection,
}

impl ModeView {
    pub fn new(mode: Mode) -> Self {
        Self {
            // Sounds are easiest to scan as a list with waveforms; pictures and models as tiles.
            layout: if mode == Mode::Sounds { Layout::List } else { Layout::Grid },
            tile: 128.0,
            sort: SortBy::Folder,
            descending: false,
            group_sort: SortBy::Added,
            group_descending: true,
            selection: Selection::default(),
        }
    }

    /// The order in use: the folder's or the groups'.
    pub fn order(&self, in_group: bool) -> (SortBy, bool) {
        if in_group { (self.group_sort, self.group_descending) } else { (self.sort, self.descending) }
    }

    #[expect(clippy::fn_params_excessive_bools, reason = "refactor: an enum instead of bool flags")]
    pub fn set_order(&mut self, in_group: bool, sort: SortBy, descending: bool) {
        if in_group {
            (self.group_sort, self.group_descending) = (sort, descending);
        } else {
            (self.sort, self.descending) = (sort, descending);
        }
    }
}

/// Selected assets: indices into `Library::entries`.
#[derive(Default)]
pub struct Selection {
    /// The one the inspector shows and the arrow keys move from.
    pub focus: Option<u32>,
    /// Where a shift-click range starts.
    pub anchor: Option<u32>,
    pub items: HashSet<u32>,
}

impl Selection {
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn contains(&self, index: u32) -> bool {
        self.items.contains(&index)
    }

    pub fn clear(&mut self) {
        self.focus = None;
        self.anchor = None;
        self.items.clear();
    }

    pub fn only(&mut self, index: u32) {
        self.items.clear();
        self.items.insert(index);
        self.focus = Some(index);
        self.anchor = Some(index);
    }

    /// Ctrl+click.
    pub fn toggle(&mut self, index: u32) {
        if self.items.remove(&index) {
            if self.focus == Some(index) {
                self.focus = self.items.iter().next().copied();
            }
        } else {
            self.items.insert(index);
            self.focus = Some(index);
        }
        self.anchor = Some(index);
    }

    /// Shift+click: everything in `view` from the anchor to `index`.
    pub fn extend_to(&mut self, view: &[u32], index: u32) {
        let anchor = self.anchor.unwrap_or(index);
        let position = |target: u32| view.iter().position(|&i| i == target);
        let (Some(a), Some(b)) = (position(anchor), position(index)) else {
            self.only(index);
            return;
        };
        self.items = view[a.min(b)..=a.max(b)].iter().copied().collect();
        self.focus = Some(index);
        self.anchor = Some(anchor);
    }

    pub fn all(&mut self, view: &[u32]) {
        self.items = view.iter().copied().collect();
        if self.focus.is_none_or(|focus| !self.items.contains(&focus)) {
            self.focus = view.first().copied();
        }
    }

    /// Carries the selection over to a rescanned library, where indices differ but keys don't.
    pub fn remap(&mut self, old_keys: &[i64], new_index: &HashMap<i64, u32>) {
        let map = |index: u32| old_keys.get(index as usize).and_then(|key| new_index.get(key)).copied();
        self.focus = self.focus.and_then(map);
        self.anchor = self.anchor.and_then(map);
        self.items = self.items.iter().filter_map(|&index| map(index)).collect();
    }

    /// Drops what's no longer shown.
    pub fn retain(&mut self, shown: &HashSet<u32>) {
        self.items.retain(|index| shown.contains(index));
        if self.focus.is_some_and(|focus| !shown.contains(&focus)) {
            self.focus = self.items.iter().next().copied();
        }
        if self.anchor.is_some_and(|anchor| !shown.contains(&anchor)) {
            self.anchor = self.focus;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_and_toggles() {
        let view = [10, 11, 12, 13, 14];
        let mut selection = Selection::default();
        selection.only(11);
        selection.extend_to(&view, 13);
        assert_eq!(selection.len(), 3);
        assert_eq!(selection.focus, Some(13));
        selection.extend_to(&view, 10);
        assert_eq!(selection.len(), 2, "the range restarts from the anchor");
        selection.toggle(14);
        assert!(selection.contains(14) && selection.focus == Some(14));
        selection.toggle(14);
        assert!(!selection.contains(14));
        assert!(selection.focus.is_some_and(|focus| selection.contains(focus)));
    }

    #[test]
    fn remap_follows_keys() {
        let mut selection = Selection::default();
        selection.only(1);
        let old_keys = [100, 200, 300];
        let new_index: HashMap<i64, u32> = [(200, 7), (300, 8)].into_iter().collect();
        selection.remap(&old_keys, &new_index);
        assert_eq!(selection.focus, Some(7));
        assert!(selection.contains(7));
    }
}
