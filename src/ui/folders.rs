//! The folder tree. Counts follow the mode, and folders with nothing of the mode's kind are left
//! out. Only the rows on screen are drawn, however many folders are open.

use std::collections::HashSet;

use eframe::egui::{self, Align2, FontId, Rect, Sense, Ui, pos2, vec2};
use egui_phosphor::regular as icon;

use crate::library::Library;

use super::format::grouped;
use super::theme;

const ROW_HEIGHT: f32 = 26.0;
const INDENT: f32 = 14.0;

/// Which folders are open, by lowercase path. The root is always open.
#[derive(Default)]
pub struct FolderTree {
    expanded: HashSet<String>,
}

struct Row {
    dir: u32,
    depth: usize,
    has_children: bool,
}

impl FolderTree {
    /// Opens every folder above `dir`, so it can be seen.
    pub fn reveal(&mut self, library: &Library, dir: u32) {
        let chain = library.dir_chain(dir);
        for &index in &chain[..chain.len().saturating_sub(1)] {
            self.expanded.insert(library.dirs[index as usize].lower.clone());
        }
    }

    /// Draws the tree; returns the folder clicked.
    pub fn show(&mut self, ui: &mut Ui, library: &Library, kinds: [bool; 3], selected: u32) -> Option<u32> {
        let rows = self.rows(library, kinds);
        let mut clicked = None;
        let mut toggled = None;
        // `show_rows` adds the item spacing to every row: set it to nothing before, not inside.
        ui.spacing_mut().item_spacing.y = 0.0;
        egui::ScrollArea::vertical().id_salt("folders").auto_shrink([false; 2]).show_rows(
            ui,
            ROW_HEIGHT,
            rows.len(),
            |ui, range| {
                for row in &rows[range] {
                    let node = &library.dirs[row.dir as usize];
                    let (rect, response) =
                        ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
                    let is_selected = row.dir == selected;
                    let open = row.dir == 0 || self.expanded.contains(&node.lower);
                    let painter = ui.painter();
                    let row_rect = rect.shrink2(vec2(4.0, 1.0));
                    if is_selected {
                        painter.rect_filled(row_rect, theme::RADIUS, theme::ACCENT_SOFT);
                    } else if response.hovered() {
                        painter.rect_filled(row_rect, theme::RADIUS, theme::CARD_HOVER);
                    }
                    let mut x = rect.left() + 10.0 + row.depth as f32 * INDENT;
                    let y = rect.center().y;
                    // The caret opens and closes; the rest of the row picks the folder.
                    let caret = Rect::from_center_size(pos2(x + 6.0, y), vec2(18.0, ROW_HEIGHT));
                    if row.has_children && row.dir != 0 {
                        let caret_response = ui.interact(caret, ui.id().with(("caret", row.dir)), Sense::click());
                        let color = if caret_response.hovered() { theme::TEXT } else { theme::TEXT_FAINT };
                        let glyph = if open { icon::CARET_DOWN } else { icon::CARET_RIGHT };
                        painter.text(caret.center(), Align2::CENTER_CENTER, glyph, FontId::proportional(12.0), color);
                        if caret_response.clicked() {
                            toggled = Some(row.dir);
                        }
                    }
                    x += 16.0;
                    let folder_glyph = if row.dir == 0 {
                        icon::HARD_DRIVES
                    } else if open && row.has_children {
                        icon::FOLDER_OPEN
                    } else {
                        icon::FOLDER
                    };
                    let folder_color = if is_selected { theme::ACCENT } else { theme::TEXT_WEAK };
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
                    let name = if row.dir == 0 { root_name(library) } else { node.name.clone() };
                    let name_color = if is_selected { theme::TEXT } else { theme::TEXT.gamma_multiply(0.88) };
                    let mut job =
                        egui::text::LayoutJob::simple_singleline(name, FontId::proportional(13.0), name_color);
                    job.wrap = egui::text::TextWrapping {
                        max_width: (count_x - x - 8.0).max(10.0),
                        max_rows: 1,
                        break_anywhere: true,
                        overflow_character: Some('…'),
                    };
                    let galley = painter.layout_job(job);
                    painter.galley(pos2(x, y - galley.size().y / 2.0), galley, name_color);
                    painter.galley(pos2(count_x, y - count_galley.size().y / 2.0), count_galley, theme::TEXT_FAINT);

                    if response.clicked() && toggled.is_none() {
                        clicked = Some(row.dir);
                    }
                    if response.double_clicked() && row.has_children {
                        toggled = Some(row.dir);
                    }
                    response.on_hover_text(if row.dir == 0 {
                        library.root.display().to_string()
                    } else {
                        node.name.clone()
                    });
                }
            },
        );
        if let Some(dir) = toggled {
            let lower = &library.dirs[dir as usize].lower;
            if !self.expanded.remove(lower) {
                self.expanded.insert(lower.clone());
            }
        }
        if let Some(dir) = clicked {
            // Picking a closed folder opens it too.
            self.expanded.insert(library.dirs[dir as usize].lower.clone());
        }
        clicked
    }

    /// The folders to show, in order, with nothing of the mode's kinds left out.
    fn rows(&self, library: &Library, kinds: [bool; 3]) -> Vec<Row> {
        let shown = |dir: u32| library.dirs[dir as usize].total(kinds) > 0;
        let mut rows = Vec::new();
        let mut stack = vec![(0u32, 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            let node = &library.dirs[dir as usize];
            let has_children = node.children.iter().any(|&child| shown(child));
            rows.push(Row { dir, depth, has_children });
            if dir == 0 || self.expanded.contains(&node.lower) {
                // Reversed onto the stack, so they come out in order.
                for &child in node.children.iter().rev().filter(|&&child| shown(child)) {
                    stack.push((child, depth + 1));
                }
            }
        }
        rows
    }
}

/// The root folder's own name ("Resources" for F:\Resources).
pub fn root_name(library: &Library) -> String {
    library
        .root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| library.root.display().to_string())
}
