//! The sprite sheet player's controls, under the playing frame: play and pause, step, the speed or
//! the file's timing, the sheet's animations, the grid when the frames were guessed, and an
//! overview of the whole sheet with the frames outlined.

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use egui_phosphor::regular as icon;

use crate::imaging::Pixels;
use crate::sheets::{self, Grid, Guess, Sheet, Source, Split};
use crate::ui::browse::fit_rect;
use crate::ui::image_view::Picture;
use crate::ui::{theme, widgets};

use super::{Playhead, SheetView};

impl SheetView {
    /// Transport, animations, grid and the sheet overview, under the playing frame.
    pub fn controls(&mut self, ui: &mut Ui, sheet_picture: &Picture, pixels: &Pixels) {
        let Some(sheet) = self.sheet.clone() else { return };
        let now = ui.input(|input| input.time);
        let count = self.current_animation().map_or(0, |animation| animation.frames.len());
        let shown = Shown { sheet: &sheet, now, position: self.position(now), count };
        let mut save = self.transport(ui, &shown);
        self.animation_buttons(ui, &shown);
        ui.label(egui::RichText::new(source_text(&sheet)).size(theme::size::SMALL).color(theme::TEXT_FAINT));
        let grid_change = sheet.grid().and_then(|grid| grid_editor(ui, grid));
        self.overview(ui, &shown, sheet_picture);
        if let Some(change) = grid_change {
            self.change_grid(change, pixels);
            save = true;
        }
        if save {
            self.choices_changed();
        }
    }

    /// Previous, play or pause, next, and the speed. Whether the user chose a speed.
    fn transport(&mut self, ui: &mut Ui, shown: &Shown<'_>) -> bool {
        let (position, count) = (shown.position, shown.count.max(1));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if widgets::icon_button(ui, icon::SKIP_BACK, "Previous frame", false).clicked() {
                self.playhead = Playhead::Paused { at: (position + count - 1) % count };
            }
            let (glyph, tip) = if self.is_playing() { (icon::PAUSE, "Pause") } else { (icon::PLAY, "Play") };
            if widgets::icon_button(ui, glyph, tip, self.is_playing()).clicked() {
                self.playhead = if self.is_playing() {
                    Playhead::Paused { at: position }
                } else {
                    self.playing_from(position, shown.now)
                };
            }
            if widgets::icon_button(ui, icon::SKIP_FORWARD, "Next frame", false).clicked() {
                self.playhead = Playhead::Paused { at: (position + 1) % count };
            }
            ui.add_space(6.0);
            self.speed(ui, shown)
        })
        .inner
    }

    /// The file's own timing, when it has one, or frames per second. Whether the user chose a
    /// speed.
    fn speed(&mut self, ui: &mut Ui, shown: &Shown<'_>) -> bool {
        let timed = shown.sheet.frames.iter().any(|frame| frame.duration_ms.is_some());
        if timed {
            let mut file = self.file_timing;
            let tip = match shown.sheet.source {
                Source::Animated { .. } => "Play the GIF at its own speed",
                _ => "Play the frame durations saved in the data file",
            };
            if widgets::toggle_chip(ui, icon::CLOCK, "File timing", &mut file, tip).changed() {
                self.file_timing = file;
                self.replay(shown.now);
            }
        }
        if timed && self.file_timing {
            return false;
        }
        let mut fps = self.fps();
        let speed = egui::DragValue::new(&mut fps).range(1.0..=60.0).speed(0.2).suffix(" fps").max_decimals(0);
        let chosen = ui.add(speed).on_hover_text("Frames per second").changed();
        if chosen {
            self.chosen_fps = Some(fps);
            if self.is_playing() {
                self.playhead = self.playing_from(shown.position, shown.now);
            }
        }
        chosen
    }

    /// A button for each of the sheet's animations, when it has several.
    fn animation_buttons(&mut self, ui: &mut Ui, shown: &Shown<'_>) {
        if shown.sheet.animations.len() <= 1 {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            for (index, animation) in shown.sheet.animations.iter().enumerate() {
                let label = format!("{} · {}", animation.name, animation.frames.len());
                if ui.add(egui::Button::selectable(index == self.animation, label)).clicked() {
                    self.animation = index;
                    self.rewind(shown.now);
                }
            }
        });
    }

    /// The whole sheet with its frames outlined, the one shown marked; click a frame to show it.
    fn overview(&mut self, ui: &mut Ui, shown: &Shown<'_>, sheet_picture: &Picture) {
        let width = ui.available_width();
        let [sw, sh] = sheet_picture.size.map(|side| side.max(1) as f32);
        let height = (width * sh / sw).clamp(40.0, 220.0);
        let (area, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
        let painter = ui.painter_at(area);
        painter.rect_filled(area, theme::RADIUS, theme::WELL);
        let fitted = fit_rect(area.shrink(4.0), sw, sh, false, ui.ctx().pixels_per_point());
        painter.image(sheet_picture.texture, fitted, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
        let scale = fitted.width() / sw;
        let to_screen = |rect: [u32; 4], rotated: bool| {
            let [x, y, w, h] = rect;
            let (w, h) = if rotated { (h, w) } else { (w, h) };
            Rect::from_min_size(
                fitted.min + vec2(x as f32 * scale, y as f32 * scale),
                vec2(w as f32 * scale, h as f32 * scale),
            )
        };
        let current = self.current_animation().and_then(|animation| animation.frames.get(shown.position)).copied();
        let in_animation: Vec<usize> =
            self.current_animation().map(|animation| animation.frames.clone()).unwrap_or_default();
        for (index, frame) in shown.sheet.frames.iter().enumerate() {
            let outline = to_screen(frame.rect, frame.rotated);
            let (width, color) = if Some(index) == current {
                (2.0, theme::ACCENT)
            } else if in_animation.contains(&index) {
                (1.0, theme::TEXT.gamma_multiply(0.45))
            } else {
                (1.0, theme::TEXT.gamma_multiply(0.15))
            };
            painter.rect_stroke(outline, 0.0, Stroke::new(width, color), StrokeKind::Inside);
        }
        if response.clicked()
            && let Some(pointer) = response.interact_pointer_pos()
            && let Some(index) =
                shown.sheet.frames.iter().position(|frame| to_screen(frame.rect, frame.rotated).contains(pointer))
        {
            self.show_frame(shown.sheet, index);
        }
        response.on_hover_text("Click a frame to show it");
    }

    /// Shows the sheet's frame `index`, paused, in the animation it belongs to.
    fn show_frame(&mut self, sheet: &Sheet, index: usize) {
        if let Some(animation) = sheet.animations.iter().position(|animation| animation.frames.contains(&index)) {
            self.animation = animation;
            let at = sheet.animations[animation].frames.iter().position(|&i| i == index).unwrap_or(0);
            self.playhead = Playhead::Paused { at };
        }
    }

    /// Plays the user's grid, or goes back to what was detected (a first guess, where nothing
    /// was).
    fn change_grid(&mut self, change: GridChange, pixels: &Pixels) {
        match change {
            GridChange::To(grid) => {
                self.sheet = Some(sheets::from_grid(grid, pixels, Guess::User));
                self.animation =
                    self.animation.min(self.sheet.as_ref().map_or(0, |s| s.animations.len().saturating_sub(1)));
            }
            GridChange::BackToDetected => {
                self.sheet = self.detected.clone().or_else(|| {
                    let grid = sheets::default_grid(pixels);
                    Some(sheets::from_grid(grid, pixels, Guess::Default))
                });
                self.animation = 0;
            }
        }
    }
}

/// The sheet as the controls show it this frame, and where playback was when they started.
struct Shown<'a> {
    sheet: &'a Sheet,
    /// `InputState::time`.
    now: f64,
    /// Where in the animation playback is, and how many frames it has.
    position: usize,
    count: usize,
}

/// What the user did to the grid.
#[derive(Clone, Copy)]
enum GridChange {
    To(Grid),
    BackToDetected,
}

/// Where the frames came from.
fn source_text(sheet: &Sheet) -> String {
    match &sheet.source {
        Source::Animated { total, .. } if *total > sheet.frames.len() => {
            format!("The first {} of the GIF's {} frames", sheet.frames.len(), total)
        }
        Source::Animated { .. } => "The GIF's own frames".to_owned(),
        Source::Data(name) => format!("Frames and animations from {name}"),
        Source::Grid(_, Guess::FileName) => "Frames counted from the file name".to_owned(),
        Source::Grid(_, Guess::Gaps) => "Frames found between transparent gaps".to_owned(),
        Source::Grid(_, Guess::SquareStrip) => "Frames guessed as squares".to_owned(),
        Source::Grid(_, Guess::User) => "Your grid".to_owned(),
        Source::Grid(_, Guess::Default) => "A first guess: set the grid below".to_owned(),
    }
}

/// The grid's numbers to change, and a button back to the detected grid.
fn grid_editor(ui: &mut Ui, mut grid: Grid) -> Option<GridChange> {
    let before = grid;
    let mut change = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        ui.label(egui::RichText::new("Grid").color(theme::TEXT_WEAK));
        ui.add(egui::DragValue::new(&mut grid.columns).range(1..=256).speed(0.1).suffix(" across"));
        ui.add(egui::DragValue::new(&mut grid.rows).range(1..=256).speed(0.1).suffix(" down"));
        egui::ComboBox::from_id_salt("sheet-split").width(130.0).selected_text(split_label(grid.split)).show_ui(
            ui,
            |ui| {
                for split in [Split::Rows, Split::Columns, Split::All] {
                    ui.selectable_value(&mut grid.split, split, split_label(split));
                }
            },
        );
    });
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        ui.label(egui::RichText::new("Border").color(theme::TEXT_WEAK));
        ui.add(egui::DragValue::new(&mut grid.margin).range(0..=256).speed(0.1).suffix(" px"));
        ui.label(egui::RichText::new("Gap").color(theme::TEXT_WEAK));
        ui.add(egui::DragValue::new(&mut grid.spacing).range(0..=256).speed(0.1).suffix(" px"));
        let reset =
            widgets::icon_button_sized(ui, icon::ARROW_COUNTER_CLOCKWISE, "Back to the detected grid", false, 24.0);
        if reset.clicked() {
            change = Some(GridChange::BackToDetected);
        }
    });
    if grid != before {
        change = Some(GridChange::To(grid));
    }
    change
}

fn split_label(split: Split) -> &'static str {
    match split {
        Split::Rows => "A row each",
        Split::Columns => "A column each",
        Split::All => "One animation",
    }
}
