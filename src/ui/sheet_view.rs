//! The preview pane's sprite sheet player: a picture's frames played as an animation, its
//! animations to pick from, the grid to adjust when the frames were guessed, and an overview of
//! the whole sheet with the frames outlined and the current one marked (click a frame to show
//! it).
//!
//! What the user chooses for a picture (to animate it, their grid, the speed) is handed to the app
//! to save per file in the user data file (`take_choices`), and given back with the picture next
//! time (`apply`).
//!
//! An animated GIF plays here too, as a sheet of its own frames (`animated`). The frame is drawn
//! through the picture view (`ImageView`), so it zooms and pans like a picture.

use eframe::egui::{
    self, Align2, Color32, Mesh, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, TextureId, Ui, pos2, vec2,
};
use egui_phosphor::regular as icon;

use crate::imaging::Pixels;
use crate::sheets::{self, Frame, Grid, Guess, Saved, Sheet, Source, Split};

use super::browse::fit_rect;
use super::image_view::{Content, ImageView, Picture, Place};
use super::{theme, widgets};

/// Frames per second when the frames have no timing of their own and no speed was picked.
const DEFAULT_FPS: f32 = 8.0;

pub struct SheetView {
    /// The picture the state below is for (its `groups::path_key`).
    key: Option<String>,
    /// What was detected, to go back to from the user's own grid.
    detected: Option<Sheet>,
    /// What's played: the detected frames or the user's grid.
    pub sheet: Option<Sheet>,
    /// Showing the frames played instead of the whole picture.
    pub animate: bool,
    animation: usize,
    /// Where playback is.
    playhead: Playhead,
    /// The speed the user picked for this picture; without one it plays at `DEFAULT_FPS`.
    chosen_fps: Option<f32>,
    /// Play the frame durations the file gives (a data file's, a GIF's) instead of the speed.
    file_timing: bool,
    /// What the user changed, until the app takes it to save: the picture's key and the choices.
    changed: Option<(String, Saved)>,
}

/// Where an animation's playback is.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Playhead {
    /// Playing since this time (`InputState::time`), counted from the first frame.
    Playing { since: f64 },
    /// Stopped on this place in the animation.
    Paused { at: usize },
}

impl Default for SheetView {
    fn default() -> Self {
        Self {
            key: None,
            detected: None,
            sheet: None,
            animate: false,
            animation: 0,
            playhead: Playhead::Playing { since: 0.0 },
            chosen_fps: None,
            file_timing: true,
            changed: None,
        }
    }
}

impl SheetView {
    pub fn is_for(&self, key: &str) -> bool {
        self.key.as_deref() == Some(key)
    }

    /// Takes a newly shown picture, as detected: its frames play if they're certain.
    pub fn load(&mut self, key: String, detected: Option<Sheet>, now: f64) {
        self.sheet = detected.clone();
        self.animate = self.sheet.as_ref().is_some_and(Sheet::is_certain);
        self.chosen_fps = None;
        self.detected = detected;
        self.key = Some(key);
        self.animation = 0;
        self.playhead = Playhead::Playing { since: now };
        self.file_timing = true;
    }

    /// What the user chose for the picture last time, over what was detected.
    pub fn apply(&mut self, saved: Saved, pixels: &Pixels) {
        // A data file's or a GIF's own frames aren't overridden by a grid.
        let from_file = matches!(self.detected, Some(Sheet { source: Source::Data(_) | Source::Animated { .. }, .. }));
        if let Some(grid) = saved.grid
            && !from_file
        {
            self.sheet = Some(sheets::from_grid(grid, pixels, Guess::User));
        }
        self.animate = saved.animate && self.sheet.is_some();
        self.chosen_fps = saved.fps.map(|fps| fps.clamp(1.0, 60.0));
    }

    /// The pane moved to something that isn't a picture, or the choices were forgotten: the
    /// picture is taken again as detected.
    pub fn clear(&mut self) {
        self.key = None;
        self.sheet = None;
        self.detected = None;
        self.animate = false;
    }

    /// What the user changed since it was last asked, once: the picture's key and the choices to
    /// save.
    pub fn take_choices(&mut self) -> Option<(String, Saved)> {
        self.changed.take()
    }

    /// How many frames were found, for the button that starts playing them.
    pub fn frame_count(&self) -> Option<usize> {
        self.sheet.as_ref().map(|sheet| sheet.frames.len())
    }

    /// Switches between the picture and its frames played. With nothing detected, the frames
    /// come from a first guess at a grid, to adjust.
    pub fn toggle(&mut self, pixels: &Pixels, now: f64) {
        self.animate = !self.animate;
        if self.animate && self.sheet.is_none() {
            let grid = sheets::default_grid(pixels);
            self.sheet = Some(sheets::from_grid(grid, pixels, Guess::Default));
        }
        self.replay(now);
        self.choices_changed();
    }

    /// Keeps the user's choices for this picture for `take_choices`.
    fn choices_changed(&mut self) {
        let Some(key) = &self.key else { return };
        // The grid is only kept when it's the user's own; otherwise detection decides each time.
        let grid = self.sheet.as_ref().and_then(|sheet| match sheet.source {
            Source::Grid(grid, Guess::User | Guess::Default) => Some(grid),
            _ => None,
        });
        let fps = self.chosen_fps;
        self.changed = Some((key.clone(), Saved { animate: self.animate, grid, fps }));
    }

    fn fps(&self) -> f32 {
        self.chosen_fps.unwrap_or(DEFAULT_FPS)
    }

    fn is_playing(&self) -> bool {
        matches!(self.playhead, Playhead::Playing { .. })
    }

    /// Playing on from `position` in the animation, as if it had played up to it.
    fn playing_from(&self, position: usize, now: f64) -> Playhead {
        Playhead::Playing { since: now - position as f64 / self.fps() as f64 }
    }

    /// Starts over from the first frame if it's playing; a paused frame stays.
    fn replay(&mut self, now: f64) {
        if self.is_playing() {
            self.playhead = Playhead::Playing { since: now };
        }
    }

    /// Back to the first frame, playing or paused.
    fn rewind(&mut self, now: f64) {
        self.playhead = if self.is_playing() { Playhead::Playing { since: now } } else { Playhead::Paused { at: 0 } };
    }

    /// Where in the animation playback is (a position in its frame list).
    fn position(&self, now: f64) -> usize {
        let Some(animation) = self.current_animation() else { return 0 };
        let count = animation.frames.len().max(1);
        let since = match self.playhead {
            Playhead::Paused { at } => return at.min(count - 1),
            Playhead::Playing { since } => since,
        };
        let elapsed = (now - since).max(0.0);
        if let Some(durations) = self.durations() {
            let total: u64 = durations.iter().map(|&d| d as u64).sum::<u64>().max(1);
            let mut t = (elapsed * 1000.0) as u64 % total;
            for (position, &duration) in durations.iter().enumerate() {
                if t < duration as u64 {
                    return position;
                }
                t -= duration as u64;
            }
            return count - 1;
        }
        (elapsed * self.fps() as f64) as usize % count
    }

    /// The data file's frame durations for the current animation, when they're used.
    fn durations(&self) -> Option<Vec<u32>> {
        let sheet = self.sheet.as_ref()?;
        if !self.file_timing {
            return None;
        }
        self.current_animation()?.frames.iter().map(|&index| sheet.frames.get(index)?.duration_ms).collect()
    }

    fn current_animation(&self) -> Option<&sheets::Animation> {
        let sheet = self.sheet.as_ref()?;
        sheet.animations.get(self.animation.min(sheet.animations.len().saturating_sub(1)))
    }

    /// Draws the current frame from `sheet`, the whole sheet's picture, through the picture view
    /// (zoom, pan, backdrop), and keeps the animation moving.
    pub fn draw(&mut self, ui: &mut Ui, view: &mut ImageView, place: &Place, sheet_picture: &Picture) {
        let (rect, texture, sheet_size) = (place.rect, sheet_picture.texture, sheet_picture.size);
        let now = ui.input(|input| input.time);
        let position = self.position(now);
        let (Some(sheet), Some(animation)) = (self.sheet.as_ref(), self.current_animation()) else { return };
        let Some(frame) = animation.frames.get(position).and_then(|&index| sheet.frames.get(index)) else { return };
        // One box for all of the animation's frames, so a zoom holds still while it plays.
        let size = animation
            .frames
            .iter()
            .filter_map(|&index| sheet.frames.get(index))
            .fold([1, 1], |[w, h], frame| [w.max(frame.size[0]), h.max(frame.size[1])]);
        view.show_with(ui, place, Content { size, pixel_art: true }, |painter, area| {
            let scale = area.width() / size[0] as f32;
            let inset = vec2((size[0] - frame.size[0]) as f32, (size[1] - frame.size[1]) as f32) * scale / 2.0;
            let shown = vec2(frame.size[0] as f32, frame.size[1] as f32) * scale;
            draw_frame(painter, Rect::from_min_size(area.min + inset, shown), texture, sheet_size, frame);
        });

        let count = animation.frames.len();
        let label = format!("{} / {count}", position + 1);
        widgets::badge(&ui.painter_at(rect), rect.left_top() + vec2(8.0, 8.0), Align2::LEFT_TOP, &label, theme::TEXT);
        if self.is_playing() && count > 1 {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f32(1.0 / 60.0));
        }
    }

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

/// Draws one frame into `canvas` (the whole frame's area): its pixels at their offset, turned
/// back upright if the sheet stores them rotated.
fn draw_frame(painter: &egui::Painter, canvas: Rect, texture: TextureId, sheet_size: [u32; 2], frame: &Frame) {
    let scale = canvas.width() / frame.size[0].max(1) as f32;
    let [x, y, w, h] = frame.rect.map(|v| v as f32);
    let quad = Rect::from_min_size(
        canvas.min + vec2(frame.offset[0] as f32 * scale, frame.offset[1] as f32 * scale),
        vec2(w * scale, h * scale),
    );
    let [sw, sh] = sheet_size.map(|side| side.max(1) as f32);
    if !frame.rotated {
        let uv = Rect::from_min_max(pos2(x / sw, y / sh), pos2((x + w) / sw, (y + h) / sh));
        painter.image(texture, quad, uv, Color32::WHITE);
        return;
    }
    // Stored turned 90° clockwise: the sheet holds an h × w region whose top-right corner is the
    // frame's top-left.
    let (u0, v0, u1, v1) = (x / sw, y / sh, (x + h) / sw, (y + w) / sh);
    let mut mesh = Mesh::with_texture(texture);
    let corners = [
        (quad.left_top(), pos2(u1, v0)),
        (quad.right_top(), pos2(u1, v1)),
        (quad.right_bottom(), pos2(u0, v1)),
        (quad.left_bottom(), pos2(u0, v0)),
    ];
    for (pos, uv) in corners {
        mesh.vertices.push(egui::epaint::Vertex { pos, uv, color: Color32::WHITE });
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(Shape::mesh(mesh));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheets::Grid;

    fn grid(columns: u32) -> Grid {
        Grid { columns, rows: 1, margin: 0, spacing: 0, split: Split::Rows }
    }

    #[test]
    fn the_users_choices_go_over_what_was_detected() {
        let pixels = Pixels { w: 64, h: 16, src_w: 64, src_h: 16, rgba: vec![255; 64 * 16 * 4] };
        let mut view = SheetView::default();
        // Counted from the file name: certain, so it plays.
        view.load("walk_strip4.png".into(), Some(sheets::from_grid(grid(4), &pixels, Guess::FileName)), 0.0);
        assert!(view.animate);
        assert_eq!(view.chosen_fps, None);

        view.apply(Saved { animate: false, grid: Some(grid(2)), fps: Some(90.0) }, &pixels);
        assert!(!view.animate);
        assert!(
            matches!(view.sheet.as_ref().map(|sheet| &sheet.source), Some(Source::Grid(g, Guess::User)) if *g == grid(2))
        );
        assert_eq!(view.chosen_fps, Some(60.0));
        // Showing what was saved is no change to save.
        assert!(view.take_choices().is_none());

        // Playing them is.
        view.toggle(&pixels, 1.0);
        let (key, saved) = view.take_choices().unwrap();
        assert_eq!(key, "walk_strip4.png");
        assert_eq!(saved, Saved { animate: true, grid: Some(grid(2)), fps: Some(60.0) });
    }

    #[test]
    fn the_playhead_plays_pauses_and_starts_over() {
        let pixels = Pixels { w: 64, h: 16, src_w: 64, src_h: 16, rgba: vec![255; 64 * 16 * 4] };
        let mut view = SheetView::default();
        view.load("walk_strip4.png".into(), Some(sheets::from_grid(grid(4), &pixels, Guess::FileName)), 10.0);
        // Four frames at 8 a second: a quarter of a second in is the third.
        assert_eq!(view.position(10.25), 2);
        view.playhead = Playhead::Paused { at: 3 };
        assert_eq!(view.position(99.0), 3);
        // Playing the animation again keeps a paused frame; an animation's button goes back to
        // the first.
        view.replay(20.0);
        assert_eq!(view.playhead, Playhead::Paused { at: 3 });
        view.rewind(20.0);
        assert_eq!(view.playhead, Playhead::Paused { at: 0 });
        // Play carries on from the frame shown.
        view.playhead = view.playing_from(2, 30.0);
        assert_eq!(view.position(30.0), 2);
        view.rewind(31.0);
        assert_eq!(view.position(31.0), 0);
    }

    #[test]
    fn a_grid_never_goes_over_a_data_files_frames() {
        let pixels = Pixels { w: 64, h: 16, src_w: 64, src_h: 16, rgba: vec![255; 64 * 16 * 4] };
        let from_data =
            Sheet { source: Source::Data("walk.json".into()), ..sheets::from_grid(grid(4), &pixels, Guess::Gaps) };
        let mut view = SheetView::default();
        view.load("walk.png".into(), Some(from_data), 0.0);
        view.apply(Saved { animate: true, grid: Some(grid(2)), fps: None }, &pixels);
        assert!(matches!(view.sheet.as_ref().map(|sheet| &sheet.source), Some(Source::Data(_))));
        assert!(view.animate);
    }
}
