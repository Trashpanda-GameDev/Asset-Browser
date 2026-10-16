//! Above the browsed assets: where you are, how many there are, and how they're shown.

use eframe::egui::{self, Align, Layout, RichText, Sense, vec2};
use egui_phosphor::regular as icon;

use crate::groups::{self, GroupKind, UNCATEGORIZED};
use crate::library::SortBy;
use crate::ui::format::grouped;
use crate::ui::mode::{Layout as ViewLayout, Mode};
use crate::ui::organize::GroupCommand;
use crate::ui::{theme, widgets};

use super::BrowserApp;

impl BrowserApp {
    /// Above the assets: where you are, how many there are, and how they're shown.
    #[expect(clippy::too_many_lines, reason = "refactor: split into named steps")]
    pub(super) fn toolbar(&mut self, ui: &mut egui::Ui) {
        let Some(library) = &self.library else { return };
        // A folder's path, or the group's name with what's not here.
        let crumbs: Vec<(u32, String)> = match self.group {
            Some(_) => Vec::new(),
            None => library
                .dir_chain(self.selected_dir)
                .into_iter()
                // "All folders" only with several.
                .filter(|&dir| dir >= library.home())
                .map(|dir| (dir, library.dirs[dir as usize].name.clone()))
                .collect(),
        };
        // `None` for the kind is Uncategorized.
        let group_title = match self.group {
            Some(UNCATEGORIZED) => Some((None, groups::UNCATEGORIZED_NAME.to_owned(), 0)),
            Some(id) => self.groups.get(id).map(|group| (Some(group.kind), group.name.clone(), group.color)),
            None => None,
        };
        let missing = self.group.and_then(|id| self.membership.missing.get(&id)).map_or(0, Vec::len);
        let elsewhere = self.group.and_then(|id| self.membership.elsewhere.get(&id)).copied().unwrap_or(0);
        let outside = match library.roots.as_slice() {
            [root] => root.name.clone(),
            _ => "the library's folders".to_owned(),
        };
        let mode = self.mode;
        let in_group = self.in_group_order();
        let shown = self.view.len();
        let mut go_to = None;
        let mut command = None;
        ui.horizontal(|ui| {
            ui.set_min_height(32.0);
            ui.spacing_mut().item_spacing.x = 2.0;
            if let Some((kind, name, color)) = &group_title {
                let title = RichText::new(name).font(egui::FontId::new(13.5, theme::semibold())).color(theme::TEXT);
                match kind {
                    Some(GroupKind::Favorites) => {
                        let star = RichText::new(icon::STAR).family(theme::filled_icons()).color(theme::FAVORITE);
                        ui.label(star);
                    }
                    Some(GroupKind::Collection) => {
                        ui.label(RichText::new(icon::STACK).size(14.0).color(theme::TEXT_WEAK));
                    }
                    Some(GroupKind::Category) => {
                        let (dot, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                        ui.painter().circle_filled(dot.center(), 5.0, theme::category_color(*color));
                    }
                    None => {
                        let (dot, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                        ui.painter().circle_stroke(dot.center(), 4.5, egui::Stroke::new(1.5, theme::TEXT_WEAK));
                    }
                }
                ui.add_space(4.0);
                ui.label(title);
            }
            for (i, (dir, name)) in crumbs.iter().enumerate() {
                if i > 0 {
                    ui.label(RichText::new(icon::CARET_RIGHT).size(11.0).color(theme::TEXT_FAINT));
                }
                let last = i + 1 == crumbs.len();
                let text = if last {
                    RichText::new(name).font(egui::FontId::new(13.5, theme::semibold())).color(theme::TEXT)
                } else {
                    RichText::new(name).size(13.0).color(theme::TEXT_WEAK)
                };
                if ui.add(egui::Label::new(text).sense(Sense::click())).clicked() && !last {
                    go_to = Some(*dir);
                }
            }
            ui.add_space(10.0);
            let view = &mut self.modes[mode.index()];
            let mut count = format!("{} {}", grouped(shown), if shown == 1 { "item" } else { "items" });
            if view.selection.len() > 1 {
                count.push_str(&format!(" · {} selected", grouped(view.selection.len())));
            }
            if elsewhere > 0 {
                count.push_str(&format!(" · {} outside {outside}", grouped(elsewhere)));
            }
            ui.label(RichText::new(count).size(12.0).color(theme::TEXT_FAINT));
            if missing > 0
                && let Some(id) = self.group
            {
                ui.add_space(6.0);
                let text = RichText::new(format!("{}  {} missing", icon::WARNING, grouped(missing)))
                    .size(12.0)
                    .color(theme::ERROR);
                let response = ui.add(egui::Label::new(text).sense(Sense::click()));
                if response
                    .on_hover_text("Moved, renamed or deleted since they were added. Click to take them out.")
                    .clicked()
                {
                    command = Some(GroupCommand::RemoveMissing(id));
                }
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if view.layout == ViewLayout::Grid {
                    ui.add_sized(vec2(96.0, 20.0), egui::Slider::new(&mut view.tile, 64.0..=256.0).show_value(false))
                        .on_hover_text("Tile size");
                    ui.label(RichText::new(icon::SQUARES_FOUR).size(14.0).color(theme::TEXT_FAINT));
                }
                let (sort, descending) = view.order(in_group);
                let arrow = if descending { icon::SORT_DESCENDING } else { icon::SORT_ASCENDING };
                let options: &[SortBy] = if in_group { &SortBy::IN_GROUPS } else { &SortBy::ALL };
                let sort_menu = ui.menu_button(format!("{arrow}  {}", sort.label()), |ui| {
                    ui.set_min_width(150.0);
                    for &option in options {
                        if ui.selectable_label(option == sort, option.label()).clicked() {
                            view.set_order(in_group, option, option.starts_descending());
                            self.view_dirty = true;
                            ui.close();
                        }
                    }
                    ui.separator();
                    for (down, glyph, label) in
                        [(false, icon::SORT_ASCENDING, "Ascending"), (true, icon::SORT_DESCENDING, "Descending")]
                    {
                        if ui.selectable_label(descending == down, format!("{glyph}  {label}")).clicked() {
                            view.set_order(in_group, sort, down);
                            self.view_dirty = true;
                            ui.close();
                        }
                    }
                });
                sort_menu.response.on_hover_text("Order");
                // Each mode's own view options.
                match mode {
                    Mode::All | Mode::Sounds => {
                        widgets::toggle_chip(
                            ui,
                            icon::PLAY_CIRCLE,
                            "Auto-play",
                            &mut self.sound.autoplay,
                            "Play sounds as soon as they're selected, by click or arrow key",
                        );
                    }
                    Mode::Images => crate::ui::image_view::backdrop_buttons(ui, &mut self.inspector.image.backdrop),
                    Mode::Models => {
                        widgets::toggle_chip(
                            ui,
                            icon::ARROWS_CLOCKWISE,
                            "Turntable",
                            &mut self.inspector.viewport.auto_rotate,
                            "Turn the model in the preview pane slowly",
                        );
                    }
                }
            });
        });
        if let Some(dir) = go_to {
            self.select_dir(dir);
        }
        if let Some(command) = command {
            self.run_group_command(command);
        }
    }
}
