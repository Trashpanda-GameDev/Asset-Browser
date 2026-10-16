//! The folder tree, and the header's list of the library's folders. Counts follow the mode, and
//! folders with nothing of the mode's kind are left out. Only the rows on screen are drawn,
//! however many folders are open.
//!
//! With one folder in the library the tree starts at it; with several, at "All folders" above
//! them. Neither changes anything itself: they report a `FolderAction` for the app to carry out.

use std::collections::HashSet;
use std::path::PathBuf;

use eframe::egui::{self, Align2, FontId, Rect, Response, Sense, Ui, pos2, vec2};
use egui_phosphor::regular as icon;

use crate::asset::KindSet;
use crate::library::{DirNode, Library};

use super::format::grouped;
use super::{theme, widgets};

const ROW_HEIGHT: f32 = widgets::SIDEBAR_ROW_HEIGHT;
const INDENT: f32 = 14.0;

/// What the user asked of a folder.
pub enum FolderAction {
    /// Browse it.
    Select(u32),
    /// Open it in the file manager.
    Open(u32),
    CopyPath(u32),
    /// Choose a folder to add to the library.
    AddRoot,
    /// Take one of the library's folders out (its files are untouched).
    RemoveRoot(PathBuf),
    /// Ask "Delete?" for a folder inside the library's folders (to the Recycle Bin).
    Delete(u32),
}

/// Which folders are open, by `DirNode::key`, so it survives rescans and folders coming and
/// going. The tree's first row is always open; the library's folders start open.
#[derive(Default)]
pub struct FolderTree {
    expanded: HashSet<String>,
    /// The library's folders that were closed.
    collapsed: HashSet<String>,
}

/// What the folder tree shows.
#[derive(Clone, Copy)]
pub struct FolderArgs<'a> {
    pub library: &'a Library,
    /// What the mode browses, for the counts.
    pub kinds: KindSet,
    /// The folder browsed; none (`u32::MAX`) while a group is.
    pub selected: u32,
    /// The library's folders that couldn't be read at their last scan, by key: they show what
    /// was found there before.
    pub unreadable: &'a HashSet<String>,
}

struct Row {
    dir: u32,
    depth: usize,
    has_children: bool,
}

/// One of the library's folders (rather than a folder inside one, or the top).
fn is_root(node: &DirNode) -> bool {
    node.root.is_some() && node.lower.is_empty()
}

impl FolderTree {
    fn is_open(&self, library: &Library, dir: u32) -> bool {
        let node = library.dir(dir);
        if dir == library.home() {
            true
        } else if is_root(node) {
            !self.collapsed.contains(&node.key)
        } else {
            self.expanded.contains(&node.key)
        }
    }

    fn set_open(&mut self, library: &Library, dir: u32, open: bool) {
        let node = library.dir(dir);
        let (set, insert) = if is_root(node) { (&mut self.collapsed, !open) } else { (&mut self.expanded, open) };
        if insert {
            set.insert(node.key.clone());
        } else {
            set.remove(&node.key);
        }
    }

    /// Opens every folder above `dir`, so it can be seen.
    pub fn reveal(&mut self, library: &Library, dir: u32) {
        let chain = library.dir_chain(dir);
        for &index in &chain[..chain.len().saturating_sub(1)] {
            self.set_open(library, index, true);
        }
    }

    /// Draws the tree; returns what was asked of a folder.
    #[expect(clippy::too_many_lines, clippy::cognitive_complexity, reason = "refactor: split into named steps")]
    pub fn show(&mut self, ui: &mut Ui, args: &FolderArgs<'_>) -> Option<FolderAction> {
        let FolderArgs { library, kinds, selected, unreadable } = *args;
        let rows = self.rows(library, kinds);
        let mut action = None;
        let mut toggled = None;
        // `show_rows` adds the item spacing to every row: set it to nothing before, not inside.
        ui.spacing_mut().item_spacing.y = 0.0;
        egui::ScrollArea::vertical().id_salt("folders").auto_shrink([false; 2]).show_rows(
            ui,
            ROW_HEIGHT,
            rows.len(),
            |ui, range| {
                for row in &rows[range] {
                    let node = library.dir(row.dir);
                    let is_selected = row.dir == selected;
                    let (rect, response) = widgets::sidebar_row(ui, is_selected);
                    let open = self.is_open(library, row.dir);
                    let painter = ui.painter();
                    let mut x = rect.left() + 10.0 + row.depth as f32 * INDENT;
                    let y = rect.center().y;
                    // The caret opens and closes; the rest of the row picks the folder.
                    let caret = Rect::from_center_size(pos2(x + 6.0, y), vec2(18.0, ROW_HEIGHT));
                    if row.has_children && row.dir != library.home() {
                        let caret_response = ui.interact(caret, ui.id().with(("caret", row.dir)), Sense::click());
                        let color = if caret_response.hovered() { theme::TEXT } else { theme::TEXT_FAINT };
                        let glyph = if open { icon::CARET_DOWN } else { icon::CARET_RIGHT };
                        painter.text(caret.center(), Align2::CENTER_CENTER, glyph, FontId::proportional(12.0), color);
                        if caret_response.clicked() {
                            toggled = Some(row.dir);
                        }
                    }
                    x += 16.0;
                    let unreadable = is_root(node) && unreadable.contains(&node.key);
                    let folder_glyph = if node.root.is_none() {
                        icon::FOLDERS
                    } else if unreadable {
                        icon::WARNING_CIRCLE
                    } else if is_root(node) {
                        icon::HARD_DRIVES
                    } else if open && row.has_children {
                        icon::FOLDER_OPEN
                    } else {
                        icon::FOLDER
                    };
                    let folder_color = if unreadable {
                        theme::ERROR
                    } else if is_selected {
                        theme::ACCENT
                    } else {
                        theme::TEXT_WEAK
                    };
                    painter.text(
                        pos2(x, y),
                        Align2::LEFT_CENTER,
                        folder_glyph,
                        FontId::proportional(15.0),
                        folder_color,
                    );
                    x += 22.0;

                    let count = grouped(node.total(kinds) as usize);
                    let count_galley = painter.layout_no_wrap(count, FontId::proportional(11.5), theme::TEXT_FAINT);
                    let count_x = rect.right() - 12.0 - count_galley.size().x;
                    let name_color = if is_selected { theme::TEXT } else { theme::TEXT.gamma_multiply(0.88) };
                    let galley = widgets::elided(
                        painter,
                        node.name.clone(),
                        FontId::proportional(13.0),
                        name_color,
                        (count_x - x - 8.0).max(10.0),
                    );
                    painter.galley(pos2(x, y - galley.size().y / 2.0), galley, name_color);
                    painter.galley(pos2(count_x, y - count_galley.size().y / 2.0), count_galley, theme::TEXT_FAINT);

                    if response.clicked() && toggled.is_none() {
                        action = Some(FolderAction::Select(row.dir));
                    }
                    if response.double_clicked() && row.has_children {
                        toggled = Some(row.dir);
                    }
                    if let Some(asked) = folder_menu(&response, library, row.dir) {
                        action = Some(asked);
                    }
                    response.on_hover_text(match node.root {
                        None => format!("Everything in the library's {} folders", library.roots().len()),
                        Some(root) if is_root(node) => {
                            let path = crate::platform::native(&library.root(root).path);
                            if unreadable {
                                format!(
                                    "{path}
Can't be read now (unplugged, moved or renamed?). Showing what was found there last time."
                                )
                            } else {
                                path
                            }
                        }
                        Some(_) => node.name.clone(),
                    });
                }
            },
        );
        if let Some(dir) = toggled {
            let open = self.is_open(library, dir);
            self.set_open(library, dir, !open);
        }
        if let Some(FolderAction::Select(dir)) = action {
            // Picking a closed folder opens it too.
            self.set_open(library, dir, true);
        }
        action
    }

    /// The folders to show, in order, with nothing of the mode's kinds left out.
    fn rows(&self, library: &Library, kinds: KindSet) -> Vec<Row> {
        let home = library.home();
        let shown = |dir: u32| library.dir(dir).total(kinds) > 0;
        let mut rows = Vec::new();
        let mut stack = vec![(home, 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            let node = library.dir(dir);
            // A folder of the library shows even when empty (just added, or still scanning).
            let child_shown = |child: u32| shown(child) || is_root(library.dir(child));
            let has_children = node.children.iter().any(|&child| child_shown(child));
            rows.push(Row { dir, depth, has_children });
            if self.is_open(library, dir) {
                // Reversed onto the stack, so they come out in order.
                for &child in node.children.iter().rev().filter(|&&child| child_shown(child)) {
                    stack.push((child, depth + 1));
                }
            }
        }
        rows
    }
}

/// A folder's right-click menu.
fn folder_menu(response: &Response, library: &Library, dir: u32) -> Option<FolderAction> {
    let node = library.dir(dir);
    let mut action = None;
    response.context_menu(|ui| {
        ui.set_min_width(190.0);
        if node.root.is_some() {
            if ui.button(format!("{}  Open folder", icon::ARROW_SQUARE_OUT)).clicked() {
                action = Some(FolderAction::Open(dir));
                ui.close();
            }
            if ui.button(format!("{}  Copy path", icon::COPY)).clicked() {
                action = Some(FolderAction::CopyPath(dir));
                ui.close();
            }
            ui.separator();
        }
        if ui.button(format!("{}  Add a folder…", icon::FOLDER_PLUS)).clicked() {
            action = Some(FolderAction::AddRoot);
            ui.close();
        }
        if let Some(root) = node.root.filter(|_| is_root(node)) {
            let path = library.root(root).path.clone();
            let remove = ui
                .button(format!("{}  Remove from library", icon::FOLDER_MINUS))
                .on_hover_text("The files stay where they are");
            if remove.clicked() {
                action = Some(FolderAction::RemoveRoot(path));
                ui.close();
            }
        } else if node.root.is_some() {
            // Folders inside the library's folders; those themselves are only ever removed.
            ui.separator();
            let delete = ui
                .button(format!("{}  Delete folder…", icon::TRASH))
                .on_hover_text(format!("To the {}, with everything in it", crate::platform::trash_name()));
            if delete.clicked() {
                action = Some(FolderAction::Delete(dir));
                ui.close();
            }
        }
    });
    action
}

/// The header's list of the library's folders: each to go to or take out, and one to add.
pub fn library_menu(ui: &mut Ui, library: &Library, kinds: KindSet) -> Option<FolderAction> {
    let mut action = None;
    ui.set_width(300.0);
    for (index, root) in library.roots().iter().enumerate() {
        let dir = Library::root_dir(index as u16);
        let (rect, response) = ui.allocate_exact_size(vec2(300.0, 30.0), Sense::click());
        let remove_rect = Rect::from_center_size(pos2(rect.right() - 14.0, rect.center().y), vec2(22.0, 22.0));
        let remove = ui.interact(remove_rect, response.id.with("remove"), Sense::click());
        let painter = ui.painter();
        if response.hovered() || remove.hovered() {
            painter.rect_filled(rect, theme::RADIUS, theme::CARD_HOVER);
        }
        let y = rect.center().y;
        painter.text(
            pos2(rect.left() + 18.0, y),
            Align2::CENTER_CENTER,
            icon::HARD_DRIVES,
            FontId::proportional(15.0),
            theme::TEXT_WEAK,
        );
        let count = grouped(library.dir(dir).total(kinds) as usize);
        let count = painter.layout_no_wrap(count, FontId::proportional(11.5), theme::TEXT_FAINT);
        let count_x = remove_rect.left() - 6.0 - count.size().x;
        let name = widgets::elided(
            painter,
            root.name.clone(),
            FontId::proportional(13.0),
            theme::TEXT,
            (count_x - rect.left() - 44.0).max(10.0),
        );
        painter.galley(pos2(rect.left() + 34.0, y - name.size().y / 2.0), name, theme::TEXT);
        painter.galley(pos2(count_x, y - count.size().y / 2.0), count, theme::TEXT_FAINT);
        let x_color = if remove.hovered() { theme::TEXT } else { theme::TEXT_FAINT };
        painter.text(remove_rect.center(), Align2::CENTER_CENTER, icon::X, FontId::proportional(12.0), x_color);
        if remove.on_hover_text("Remove from library (the files stay where they are)").clicked() {
            action = Some(FolderAction::RemoveRoot(root.path.clone()));
            ui.close();
        } else if response.on_hover_text(crate::platform::native(&root.path)).clicked() {
            action = Some(FolderAction::Select(dir));
            ui.close();
        }
    }
    ui.separator();
    if ui.button(format!("{}  Add a folder…", icon::FOLDER_PLUS)).clicked() {
        action = Some(FolderAction::AddRoot);
        ui.close();
    }
    action
}
