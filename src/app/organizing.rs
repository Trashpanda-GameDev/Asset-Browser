//! Favorites, collections and categories: what's asked of them from the sidebar, the menus, the
//! stars, drops and the pane, and "Delete this collection?". And how the user cut each picture
//! into a sprite sheet.

use std::path::{Path, PathBuf};

use eframe::egui::{self, Align, Layout, RichText};
use egui_phosphor::regular as icon;

use crate::asset::AssetKind;
use crate::groups::{self, FAVORITES, Groups, Membership};
use crate::library::Library;
use crate::sheets::{Saved, SheetStore};
use crate::ui::format::plural;
use crate::ui::organize::{GroupCommand, GroupEdit, Targets};
use crate::ui::{theme, widgets};

use super::BrowserApp;

/// Favorites, collections and categories, where their members are in the library, and what's
/// being done to them; and the user's sprite sheet choices.
pub(super) struct Organizing {
    pub(super) groups: Groups,
    pub(super) membership: Membership,
    pub(super) membership_dirty: bool,
    /// A group being named in the sidebar.
    pub(super) edit: Option<GroupEdit>,
    /// A group waiting for "Delete?" to be confirmed.
    pub(super) confirm_delete: Option<i64>,
    pub(super) sheets: SheetChoices,
}

impl Organizing {
    /// Both kept in the user data file at `path`.
    pub(super) fn open(path: &Path) -> Self {
        Self {
            groups: Groups::open(path),
            membership: Membership::default(),
            membership_dirty: true,
            edit: None,
            confirm_delete: None,
            sheets: SheetChoices { store: SheetStore::open(path), shown: None },
        }
    }

    /// Adds the files to the group; how many weren't in it already.
    fn add(&mut self, id: i64, paths: &[PathBuf]) -> usize {
        self.membership_dirty = true;
        self.groups.add(id, paths)
    }

    /// Takes the files (by `groups::path_key`) out of the group; how many were in it.
    fn remove(&mut self, id: i64, keys: &[String]) -> usize {
        self.membership_dirty = true;
        self.groups.remove(id, keys)
    }

    /// The group's name, for messages.
    fn name_of(&self, id: i64) -> String {
        self.groups.get(id).map_or_else(String::new, |group| group.name.clone())
    }
}

/// What the user chose for each picture's sprite sheet (to play it, their grid, the speed), read
/// for the picture in the pane once rather than every frame.
pub(super) struct SheetChoices {
    store: SheetStore,
    /// The focused picture's key (`groups::path_key`) and its saved choices.
    shown: Option<(String, Option<Saved>)>,
}

impl SheetChoices {
    /// The saved choices for the focused entry, when it's a picture.
    pub(super) fn of_focus(&mut self, library: &Library, focus: Option<u32>) -> Option<Saved> {
        let entry = library.entries().get(focus? as usize).filter(|entry| entry.kind == AssetKind::Image)?;
        let key = groups::path_key(&library.path_of(entry));
        match &self.shown {
            Some((shown, saved)) if *shown == key => *saved,
            _ => {
                let saved = self.store.get(&key);
                self.shown = Some((key, saved));
                saved
            }
        }
    }

    pub(super) fn save(&mut self, key: String, saved: Saved) {
        if self.store.set(&key, &saved) {
            self.shown = Some((key, Some(saved)));
        }
    }

    /// Forgets every picture's choices (Settings → Storage).
    pub(super) fn clear(&mut self) {
        self.store.clear();
        self.shown = None;
    }
}

impl BrowserApp {
    /// Applies what the user asked for from the sidebar, a menu, a star, a drop or the pane.
    pub(super) fn run_group_command(&mut self, command: GroupCommand) {
        // These can take assets out of the group being browsed (Uncategorized included).
        if matches!(
            command,
            GroupCommand::Add(..)
                | GroupCommand::Remove(..)
                | GroupCommand::ToggleFavorite(_)
                | GroupCommand::FinishEdit(_)
        ) {
            self.browsing.follow_focus = true;
        }
        match command {
            GroupCommand::Show(id) => self.browsing.show_group(Some(id)),
            GroupCommand::Add(id, targets) => self.add_to_group(id, targets),
            GroupCommand::Remove(id, targets) => self.remove_from_group(id, targets),
            GroupCommand::ToggleFavorite(targets) => self.toggle_favorite(targets),
            GroupCommand::New(kind, targets) => {
                let paths = self.resolve(targets);
                self.organizing.edit = Some(GroupEdit::new(kind, self.organizing.groups.unused_name(kind), paths));
            }
            GroupCommand::StartRename(id) => {
                self.organizing.edit = self.organizing.groups.get(id).map(GroupEdit::rename);
            }
            GroupCommand::FinishEdit(edit) => self.finish_edit(&edit),
            GroupCommand::Recolor(id, color) => self.organizing.groups.set_color(id, color),
            GroupCommand::AskDelete(id) => self.organizing.confirm_delete = Some(id),
            GroupCommand::RemoveMissing(id) => {
                let keys = self.organizing.membership.missing.get(&id).cloned().unwrap_or_default();
                let removed = self.organizing.remove(id, &keys);
                self.notify(format!("Took out {} that weren't found", plural(removed, "missing asset")));
            }
        }
    }

    fn add_to_group(&mut self, id: i64, targets: Targets) {
        let paths = self.resolve(targets);
        let added = self.organizing.add(id, &paths);
        let name = self.organizing.name_of(id);
        self.notify(match added {
            0 if paths.is_empty() => return,
            0 => format!("Already in {name}"),
            n => format!("Added {} to {name}", plural(n, "asset")),
        });
    }

    fn remove_from_group(&mut self, id: i64, targets: Targets) {
        let keys: Vec<String> = self.resolve(targets).iter().map(|path| groups::path_key(path)).collect();
        let removed = self.organizing.remove(id, &keys);
        if removed > 0 {
            let name = self.organizing.name_of(id);
            self.notify(format!("Took {} out of {name}; the files stay where they are", plural(removed, "asset")));
        }
    }

    /// Stars the assets, or takes the star off when they all have it.
    fn toggle_favorite(&mut self, targets: Targets) {
        let paths = self.resolve(targets);
        if paths.is_empty() {
            return;
        }
        let keys: Vec<String> = paths.iter().map(|path| groups::path_key(path)).collect();
        if keys.iter().all(|key| self.organizing.groups.contains(FAVORITES, key)) {
            let removed = self.organizing.remove(FAVORITES, &keys);
            self.notify(format!("Took {} out of Favorites", plural(removed, "asset")));
        } else {
            let added = self.organizing.add(FAVORITES, &paths);
            self.notify(format!("Added {} to Favorites", plural(added, "asset")));
        }
    }

    /// A group named in the sidebar: renamed, or made with the assets it was started from.
    fn finish_edit(&mut self, edit: &GroupEdit) {
        match edit.id {
            Some(id) => {
                self.organizing.groups.rename(id, &edit.name);
            }
            None => {
                if let Some(id) = self.organizing.groups.create(edit.kind, &edit.name) {
                    let added = self.organizing.add(id, &edit.add);
                    if added > 0 {
                        self.notify(format!("Made {} with {}", edit.name.trim(), plural(added, "asset")));
                    }
                }
            }
        }
    }

    /// "Delete this collection?" The files are never touched.
    pub(super) fn delete_dialog(&mut self, ctx: &egui::Context) {
        let Some(id) = self.organizing.confirm_delete else { return };
        let Some(group) = self.organizing.groups.get(id) else {
            self.organizing.confirm_delete = None;
            return;
        };
        let (name, noun, count) = (group.name.clone(), group.kind.noun(), group.members.len());
        let mut decision = None;
        let modal = egui::Modal::new(egui::Id::new("delete-group")).show(ctx, |ui| {
            ui.set_width(340.0);
            ui.label(
                RichText::new(format!("Delete the {noun} \u{201c}{name}\u{201d}?"))
                    .font(egui::FontId::new(15.0, theme::semibold())),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!(
                    "{} will stay where {} now.",
                    plural(count, "file"),
                    if count == 1 { "it is" } else { "they are" }
                ))
                .color(theme::TEXT_WEAK),
            );
            ui.add_space(12.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::button(ui, icon::TRASH, "Delete").clicked() {
                    decision = Some(true);
                }
                if widgets::button(ui, "", "Cancel").clicked() {
                    decision = Some(false);
                }
            });
        });
        if modal.should_close() && decision.is_none() {
            decision = Some(false);
        }
        match decision {
            Some(true) => {
                if self.organizing.groups.delete(id) {
                    self.organizing.membership_dirty = true;
                    if self.browsing.group == Some(id) {
                        self.browsing.show_group(None);
                    }
                    self.notify(format!("Deleted the {noun} \u{201c}{name}\u{201d}"));
                }
                self.organizing.confirm_delete = None;
            }
            Some(false) => self.organizing.confirm_delete = None,
            None => {}
        }
    }
}
