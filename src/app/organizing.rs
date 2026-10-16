//! Favorites, collections and categories: what's asked of them from the sidebar, the menus, the
//! stars, drops and the pane, and "Delete this collection?".

use eframe::egui::{self, Align, Layout, RichText};
use egui_phosphor::regular as icon;

use crate::groups::{self, FAVORITES, Groups};
use crate::ui::organize::{GroupCommand, GroupEdit, plural};
use crate::ui::{theme, widgets};

use super::BrowserApp;

impl BrowserApp {
    /// Applies what the user asked for from the sidebar, a menu, a star, a drop or the pane.
    pub(super) fn run_group_command(&mut self, command: GroupCommand) {
        let name_of = |groups: &Groups, id: i64| groups.get(id).map_or_else(String::new, |group| group.name.clone());
        // These can take assets out of the group being browsed (Uncategorized included).
        if matches!(
            command,
            GroupCommand::Add(..)
                | GroupCommand::Remove(..)
                | GroupCommand::ToggleFavorite(_)
                | GroupCommand::FinishEdit(_)
        ) {
            self.follow_focus = true;
        }
        match command {
            GroupCommand::Show(id) => self.show_group(Some(id)),
            GroupCommand::Add(id, targets) => {
                let paths = self.resolve(targets);
                let added = self.groups.add(id, &paths);
                self.membership_dirty = true;
                let name = name_of(&self.groups, id);
                self.notify(match added {
                    0 if paths.is_empty() => return,
                    0 => format!("Already in {name}"),
                    n => format!("Added {} to {name}", plural(n, "asset")),
                });
            }
            GroupCommand::Remove(id, targets) => {
                let keys: Vec<String> = self.resolve(targets).iter().map(|path| groups::path_key(path)).collect();
                let removed = self.groups.remove(id, &keys);
                self.membership_dirty = true;
                if removed > 0 {
                    let name = name_of(&self.groups, id);
                    self.notify(format!(
                        "Took {} out of {name}; the files stay where they are",
                        plural(removed, "asset")
                    ));
                }
            }
            GroupCommand::ToggleFavorite(targets) => {
                let paths = self.resolve(targets);
                if paths.is_empty() {
                    return;
                }
                let keys: Vec<String> = paths.iter().map(|path| groups::path_key(path)).collect();
                if keys.iter().all(|key| self.groups.contains(FAVORITES, key)) {
                    let removed = self.groups.remove(FAVORITES, &keys);
                    self.notify(format!("Took {} out of Favorites", plural(removed, "asset")));
                } else {
                    let added = self.groups.add(FAVORITES, &paths);
                    self.notify(format!("Added {} to Favorites", plural(added, "asset")));
                }
                self.membership_dirty = true;
            }
            GroupCommand::New(kind, targets) => {
                let paths = self.resolve(targets);
                self.edit = Some(GroupEdit::new(kind, self.groups.unused_name(kind), paths));
            }
            GroupCommand::StartRename(id) => {
                self.edit = self.groups.get(id).map(GroupEdit::rename);
            }
            GroupCommand::FinishEdit(edit) => match edit.id {
                Some(id) => {
                    self.groups.rename(id, &edit.name);
                }
                None => {
                    if let Some(id) = self.groups.create(edit.kind, &edit.name) {
                        let added = self.groups.add(id, &edit.add);
                        self.membership_dirty = true;
                        if added > 0 {
                            self.notify(format!("Made {} with {}", edit.name.trim(), plural(added, "asset")));
                        }
                    }
                }
            },
            GroupCommand::Recolor(id, color) => self.groups.set_color(id, color),
            GroupCommand::AskDelete(id) => self.confirm_delete = Some(id),
            GroupCommand::RemoveMissing(id) => {
                let keys = self.membership.missing.get(&id).cloned().unwrap_or_default();
                let removed = self.groups.remove(id, &keys);
                self.membership_dirty = true;
                self.notify(format!("Took out {} that weren't found", plural(removed, "missing asset")));
            }
        }
    }

    /// "Delete this collection?" The files are never touched.
    pub(super) fn delete_dialog(&mut self, ctx: &egui::Context) {
        let Some(id) = self.confirm_delete else { return };
        let Some(group) = self.groups.get(id) else {
            self.confirm_delete = None;
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
                if self.groups.delete(id) {
                    self.membership_dirty = true;
                    if self.group == Some(id) {
                        self.show_group(None);
                    }
                    self.notify(format!("Deleted the {noun} \u{201c}{name}\u{201d}"));
                }
                self.confirm_delete = None;
            }
            Some(false) => self.confirm_delete = None,
            None => {}
        }
    }
}
