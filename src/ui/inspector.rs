//! The inspector: the pane on the right showing the selected asset large, with what's known about
//! it and what can be done with it. Each kind has its own section; several selected assets get a
//! summary and a strip of thumbnails.

use std::sync::Arc;

use eframe::egui::{
    self, Align, Align2, Color32, ColorImage, FontId, Layout, Rect, RichText, Sense, TextureFilter, TextureHandle,
    TextureOptions, Ui, UiBuilder, pos2, vec2,
};
use eframe::egui_wgpu::RenderState;
use egui_phosphor::regular as icon;

use crate::asset::{AssetKind, KindSet};
use crate::detail::{Detail, Loaded};
use crate::groups::{FAVORITES, GroupKind, Groups, Membership};
use crate::imaging::Pixels;
use crate::library::{Entry, Library};
use crate::model::Model;
use crate::playback::Playback;
use crate::sheets::{Saved, Sheet, Source};

use super::asset_actions::{self, AssetAction};
use super::browse::{Look, draw_preview};
use super::display::{self, Display};
use super::format::{channels, dimensions, format_age, format_size, format_time, grouped, sample_rate};
use super::image_view::{ImageView, Picture};
use super::keymap::{self, Action};
use super::mode::Selection;
use super::organize::{self, GroupCommand, Targets};
use super::previews::Previews;
use super::sheet_view::SheetView;
use super::sound::{self, SoundArgs, SoundCommand};
use super::viewport::Viewport;
use super::{theme, widgets};

/// Thumbnails shown for a multiple selection; the rest are counted.
const STRIP_MAX: usize = 48;

#[derive(Default)]
pub struct Inspector {
    pub image: ImageView,
    pub viewport: Viewport,
    pub sheet: SheetView,
    /// The large preview of the focused asset, by key.
    detail: Option<(i64, DetailState)>,
}

enum DetailState {
    Loading,
    Image {
        texture: TextureHandle,
        size: [u32; 2],
        /// Kept for the sprite sheet player, which looks for empty frames in it.
        pixels: Arc<Pixels>,
        sheet: Option<Sheet>,
    },
    Model(Arc<Model>),
    Failed(String),
}

pub struct InspectorArgs<'a> {
    pub library: &'a Library,
    pub selection: &'a Selection,
    pub previews: &'a mut Previews,
    pub playback: &'a Playback,
    pub render_state: Option<&'a RenderState>,
    /// What's being browsed, for the empty state: its count of each kind.
    pub totals: [u32; 3],
    pub kinds: KindSet,
    pub groups: &'a Groups,
    pub membership: &'a Membership,
    /// What the user chose for the focused picture's sprite sheet before, if it's a picture.
    pub sheet_choices: Option<Saved>,
    pub display: Display,
}

#[derive(Default)]
pub struct InspectorOutput {
    /// A thumbnail in the selection strip was clicked: show that one.
    pub focus: Option<u32>,
    pub action: Option<AssetAction>,
    pub command: Option<GroupCommand>,
    pub sound: Option<SoundCommand>,
    /// The focused asset's large preview is wanted: the entry to load it for.
    pub load_detail: Option<u32>,
    /// The user changed a picture's sprite sheet: its key and the choices to save.
    pub sheet_choices: Option<(String, Saved)>,
    /// The user changed the backdrop, the turntable or the triangles in the pane.
    pub display: Option<Display>,
}

impl Inspector {
    /// Takes in a finished large preview.
    pub fn take_detail(&mut self, loaded: Loaded, ctx: &egui::Context) {
        let Some((key, state)) = &mut self.detail else { return };
        if *key != loaded.key {
            return; // the selection moved on
        }
        *state = match loaded.result {
            Ok(Detail::Image(pixels, sheet)) => {
                let image = ColorImage::from_rgba_unmultiplied([pixels.w as usize, pixels.h as usize], &pixels.rgba);
                let options = TextureOptions {
                    magnification: TextureFilter::Nearest,
                    minification: TextureFilter::Linear,
                    mipmap_mode: Some(TextureFilter::Linear),
                    ..TextureOptions::LINEAR
                };
                let texture = ctx.load_texture(format!("detail-{key}"), image, options);
                let size = [pixels.src_w, pixels.src_h];
                DetailState::Image { texture, size, pixels: Arc::new(pixels), sheet }
            }
            Ok(Detail::Model(model)) => DetailState::Model(model),
            Err(message) => DetailState::Failed(message),
        };
    }

    /// Whether the large preview of `entry` is to be loaded: it needs one and hasn't got it.
    fn want_detail(&mut self, entry: &Entry, previews: &Previews) -> bool {
        if self.detail.as_ref().is_some_and(|(key, _)| *key == entry.key) {
            return false;
        }
        let wanted = match entry.kind {
            // Every picture, small ones too: the loader also looks for sprite sheet frames in it.
            // Waits for the thumbnail so it's on screen first.
            AssetKind::Image => previews.dimensions(entry.key).is_some(),
            // Blender files only have the preview picture Blender saved.
            AssetKind::Model => !entry.extension().eq_ignore_ascii_case("blend"),
            AssetKind::Audio => false,
        };
        // Without one, lets go of the last one's picture or model.
        self.detail = wanted.then_some((entry.key, DetailState::Loading));
        wanted
    }

    #[expect(clippy::too_many_lines, reason = "refactor: split into named steps")]
    pub fn show(&mut self, ui: &mut Ui, args: InspectorArgs<'_>) -> InspectorOutput {
        let mut output = InspectorOutput::default();
        let InspectorArgs {
            library,
            selection,
            previews,
            playback,
            render_state,
            totals,
            kinds,
            groups,
            membership,
            sheet_choices,
            display,
        } = args;
        // The picture and 3D views draw with the app's display; their buttons change it here, to
        // be handed back.
        self.image.backdrop = display.backdrop;
        (self.viewport.auto_rotate, self.viewport.wireframe) = (display.turntable, display.wireframe);
        let Some(focus) = selection.focus.filter(|&focus| (focus as usize) < library.entries().len()) else {
            self.viewport.release();
            empty_state(ui, totals, kinds);
            return output;
        };
        // What the pane's buttons act on: the whole selection when the focus is part of it.
        let targets: Vec<u32> = if selection.contains(focus) {
            let mut items: Vec<u32> = selection.items.iter().copied().collect();
            items.sort_unstable();
            items
        } else {
            vec![focus]
        };
        let entry = library.entry(focus);
        previews.want(focus);
        if self.want_detail(entry, previews) {
            output.load_detail = Some(focus);
        }
        if entry.kind != AssetKind::Model {
            self.viewport.release();
        }
        if entry.kind == AssetKind::Image {
            self.load_sheet(library, entry, sheet_choices, ui.input(|input| input.time));
        } else {
            self.sheet.clear();
        }

        egui::ScrollArea::vertical().id_salt("inspector").auto_shrink([false; 2]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            let width = ui.available_width();
            match entry.kind {
                AssetKind::Image => self.image_section(ui, entry, previews, width),
                AssetKind::Audio => output.sound = sound_section(ui, library, focus, previews, playback),
                AssetKind::Model => self.model_section(ui, entry, previews, render_state, width),
            }
            ui.add_space(4.0);
            title(ui, library, entry);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                output.action = asset_actions::buttons(ui, targets.len());
                if favorite_button(ui, organize::all_in(membership, FAVORITES, &targets)).clicked() {
                    output.command = Some(GroupCommand::ToggleFavorite(Targets::Selection));
                }
            });
            ui.add_space(6.0);
            for kind in [GroupKind::Collection, GroupKind::Category] {
                widgets::section_label(ui, kind.title());
                if let Some(command) = organize::chips(ui, groups, membership, &targets, kind) {
                    output.command = Some(command);
                }
                ui.add_space(4.0);
            }
            // Every row is there from the start, with "…" until its value is known, so moving through
            // the assets with the keys doesn't make the rows below jump as values arrive.
            widgets::section_label(ui, "Details");
            match entry.kind {
                AssetKind::Image => {
                    image_properties(ui, entry, previews, self.detail_size(entry.key), self.gif_frames(entry));
                }
                AssetKind::Audio => sound_properties(ui, entry, previews),
                AssetKind::Model => self.model_properties(ui, entry, previews),
            }
            widgets::property(ui, "File size", format_size(entry.size));
            widgets::property(ui, "Modified", format_age(entry.mtime));
            // From the library folder it's in.
            let root = &library.root(entry.root).name;
            let folder = if entry.folder().is_empty() {
                root.clone()
            } else {
                format!("{root} › {}", entry.folder().replace('/', " › "))
            };
            widgets::property(ui, "Folder", RichText::new(folder).color(theme::TEXT_WEAK));
            // Last, since it only appears once the sound plays.
            if entry.kind == AssetKind::Audio
                && let Some(ready) = playback.now_for(entry.key).and_then(|now| now.ready_ms)
            {
                widgets::property(ui, "Started in", format!("{ready:.0} ms"))
                    .on_hover_text("From the request to the first audio handed to the sound card");
            }
            // Below the details: a sprite sheet's or GIF's frames load after the picture is shown, and
            // the player arriving here moves nothing above it.
            if entry.kind == AssetKind::Image {
                self.animation_section(ui, entry);
            }

            if selection.len() > 1 {
                ui.add_space(10.0);
                let look = Look::new(ui, self.image.backdrop);
                if let Some(index) = selection_strip(ui, library, selection, previews, look) {
                    output.focus = Some(index);
                }
            }
        });
        output.sheet_choices = self.sheet.take_choices();
        let shown = Display {
            backdrop: self.image.backdrop,
            turntable: self.viewport.auto_rotate,
            wireframe: self.viewport.wireframe,
        };
        output.display = (shown != display).then_some(shown);
        output
    }

    /// Hands the sprite sheet player the focused picture once it's loaded: what was detected in
    /// it, and what the user chose for it before.
    fn load_sheet(&mut self, library: &Library, entry: &Entry, saved: Option<Saved>, now: f64) {
        let Some((key, DetailState::Image { pixels, sheet, .. })) = &self.detail else { return };
        let path_key = crate::groups::path_key(&library.path_of(entry));
        if *key != entry.key || self.sheet.is_for(&path_key) {
            return;
        }
        self.sheet.load(path_key, sheet.clone(), now);
        if let Some(saved) = saved {
            self.sheet.apply(saved, pixels);
        }
    }

    fn detail_size(&self, key: i64) -> Option<[u32; 2]> {
        match &self.detail {
            Some((detail_key, DetailState::Image { size, sheet, .. })) if *detail_key == key => Some(match sheet {
                // What's loaded of a GIF is its frames side by side, not its size.
                Some(Sheet { source: Source::Animated { width, height, .. }, .. }) => [*width, *height],
                _ => *size,
            }),
            _ => None,
        }
    }

    /// A GIF's frames for its details ("24 · 2.4 s", "1" for a still one, "…" while they load);
    /// `None` for other pictures.
    fn gif_frames(&self, entry: &Entry) -> Option<RichText> {
        if !entry.extension().eq_ignore_ascii_case("gif") {
            return None;
        }
        Some(match &self.detail {
            Some((key, DetailState::Image { sheet, .. })) if *key == entry.key => match sheet {
                Some(sheet) if matches!(sheet.source, Source::Animated { .. }) => {
                    let total = match sheet.source {
                        Source::Animated { total, .. } => total,
                        _ => sheet.frames.len(),
                    };
                    let ms: u32 = sheet.frames.iter().filter_map(|frame| frame.duration_ms).sum();
                    RichText::new(format!("{} · {}", grouped(total), format_time(f64::from(ms) / 1000.0)))
                }
                _ => RichText::new("1"),
            },
            Some((key, DetailState::Failed(_))) if *key == entry.key => RichText::new("–"),
            _ => pending(),
        })
    }

    /// The sprite sheet player's controls, once the picture's frames are loaded and playing.
    fn animation_section(&mut self, ui: &mut Ui, entry: &Entry) {
        let Some((key, DetailState::Image { texture, size, pixels, .. })) = &self.detail else { return };
        if *key != entry.key || !self.sheet.animate || self.sheet.sheet.is_none() {
            return;
        }
        let (texture, size, pixels) = (texture.id(), *size, pixels.clone());
        ui.add_space(10.0);
        widgets::section_label(ui, "Animation");
        self.sheet.controls(ui, texture, size, &pixels);
    }

    fn image_section(&mut self, ui: &mut Ui, entry: &Entry, previews: &Previews, width: f32) {
        let height = (width * 0.85).clamp(180.0, 440.0);
        let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        let now = ui.input(|input| input.time);
        // The full picture, once loaded: what the sprite sheet player draws from.
        let loaded = match &self.detail {
            Some((key, DetailState::Image { texture, size, pixels, .. })) if *key == entry.key => {
                Some((texture.id(), *size, pixels.clone()))
            }
            _ => None,
        };
        let animating = self.sheet.animate && self.sheet.sheet.is_some() && loaded.is_some();

        if let (Some((texture, size, _)), true) = (&loaded, animating) {
            self.sheet.draw(ui, rect, entry.key, *texture, *size, &mut self.image, previews.checker.id());
        } else {
            let picture = match (&loaded, previews.texture(entry.key)) {
                (Some((texture, size, _)), _) => Some(Picture { texture: *texture, size: *size, pixel_art: false }),
                (None, Some(thumbnail)) => Some(Picture {
                    texture: thumbnail.handle.id(),
                    size: thumbnail.src_size,
                    pixel_art: thumbnail.unscaled,
                }),
                (None, None) => None,
            };
            match picture {
                Some(picture) => self.image.show(ui, rect, entry.key, &picture, previews.checker.id()),
                None => placeholder(ui, rect, entry, previews, Look::new(ui, self.image.backdrop)),
            }
        }
        if matches!(&self.detail, Some((key, DetailState::Loading)) if *key == entry.key) {
            spinner_badge(ui, rect);
        }
        let (sheet, backdrop) = (&mut self.sheet, &mut self.image.backdrop);
        overlay(ui, rect, |ui| {
            display::backdrop_buttons(ui, backdrop);
            if let Some((_, _, pixels)) = &loaded {
                widgets::divider(ui);
                if animation_button(ui, sheet.animate, sheet.frame_count()).clicked() {
                    sheet.toggle(pixels, now);
                }
            }
        });
    }

    #[expect(clippy::too_many_arguments, reason = "refactor: take fewer, grouped arguments")]
    fn model_section(
        &mut self,
        ui: &mut Ui,
        entry: &Entry,
        previews: &Previews,
        render_state: Option<&RenderState>,
        width: f32,
    ) {
        let height = (width * 0.9).clamp(200.0, 460.0);
        let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        let model = match &self.detail {
            Some((key, DetailState::Model(model))) if *key == entry.key => Some(model.clone()),
            _ => None,
        };
        let shown_3d = match (model, render_state) {
            (Some(model), Some(render_state)) => self.viewport.show(ui, rect, render_state, entry.key, &model),
            _ => false,
        };
        if shown_3d {
            overlay(ui, rect, |ui| self.viewport.controls(ui));
            return;
        }
        // Until the model is loaded (or when it can't be shown in 3D), the thumbnail stands in.
        ui.painter().rect_filled(rect, theme::CARD_RADIUS, theme::WELL);
        let inner = rect.shrink(16.0);
        draw_preview(ui.painter(), inner, entry, previews, None, Look::new(ui, self.image.backdrop), true);
        let note = match &self.detail {
            Some((key, DetailState::Loading)) if *key == entry.key => {
                spinner_badge(ui, rect);
                None
            }
            Some((key, DetailState::Failed(message))) if *key == entry.key => Some(message.as_str()),
            _ if entry.extension().eq_ignore_ascii_case("blend") => Some("The preview Blender saved in the file"),
            _ if render_state.is_none() => Some("3D view needs the wgpu renderer"),
            _ => None,
        };
        if let Some(note) = note {
            ui.painter().text(
                rect.center_bottom() + vec2(0.0, -10.0),
                Align2::CENTER_BOTTOM,
                note,
                FontId::proportional(theme::size::SMALL),
                theme::TEXT_FAINT,
            );
        }
    }

    fn model_properties(&self, ui: &mut Ui, entry: &Entry, previews: &Previews) {
        let loaded = match &self.detail {
            Some((key, DetailState::Model(model))) if *key == entry.key => Some(&model.info),
            _ => None,
        };
        let Some(info) = loaded.or(previews.models.get(&entry.key).map(|info| info.as_ref())) else {
            if let Some(message) = previews.failed.get(&entry.key) {
                widgets::property(ui, "Preview", RichText::new(message).color(theme::ERROR));
            } else if entry.extension().eq_ignore_ascii_case("blend") {
                widgets::property(ui, "Format", pending());
            } else {
                for name in ["Format", "Triangles", "Vertices", "Meshes", "Materials", "Textures", "Dimensions"] {
                    widgets::property(ui, name, pending());
                }
            }
            return;
        };
        widgets::property(ui, "Format", &info.format);
        if info.geometry {
            widgets::property(ui, "Triangles", grouped(info.triangles as usize));
            widgets::property(ui, "Vertices", grouped(info.vertices as usize));
            widgets::property(ui, "Meshes", grouped(info.meshes as usize));
            widgets::property(ui, "Materials", grouped(info.materials as usize));
            let textures = match (info.textures, info.missing_textures) {
                (0, 0) => "none".to_owned(),
                (found, 0) => grouped(found as usize),
                (found, missing) => format!("{found} ({missing} not found)"),
            };
            widgets::property(ui, "Textures", textures);
            let [x, y, z] = info.size;
            widgets::property(ui, "Dimensions", format!("{} × {} × {} m", metres(x), metres(y), metres(z)));
        }
        for (name, seconds) in &info.animations {
            let name = if name.is_empty() { "(unnamed)" } else { name.as_str() };
            widgets::property(ui, "Animation", format!("{name} · {}", format_time(*seconds as f64)));
        }
    }
}

fn metres(value: f32) -> String {
    if value >= 10.0 {
        format!("{value:.1}")
    } else if value >= 0.1 {
        format!("{value:.2}")
    } else {
        format!("{value:.3}")
    }
}

fn sound_section(
    ui: &mut Ui,
    library: &Library,
    focus: u32,
    previews: &Previews,
    playback: &Playback,
) -> Option<SoundCommand> {
    let key = library.entry(focus).key;
    let sound = SoundArgs {
        playback,
        library,
        index: focus,
        waveform: previews.waveforms.get(&key).map(|waveform| waveform.as_ref()),
        failed: previews.failed.contains_key(&key),
    };
    sound::player(ui, 120.0, &sound)
}

fn sound_properties(ui: &mut Ui, entry: &Entry, previews: &Previews) {
    if let Some(waveform) = previews.waveforms.get(&entry.key) {
        widgets::property(ui, "Length", format_time(waveform.duration as f64));
        widgets::property(ui, "Sample rate", sample_rate(waveform.sample_rate));
        widgets::property(ui, "Channels", channels(waveform.channels));
    } else if let Some(message) = previews.failed.get(&entry.key) {
        widgets::property(ui, "Preview", RichText::new(message).color(theme::ERROR));
    } else {
        for name in ["Length", "Sample rate", "Channels"] {
            widgets::property(ui, name, pending());
        }
    }
    widgets::property(ui, "Format", entry.extension().to_uppercase());
}

/// `frames` is a GIF's frame count and length (`Inspector::gif_frames`).
fn image_properties(
    ui: &mut Ui,
    entry: &Entry,
    previews: &Previews,
    detail: Option<[u32; 2]>,
    frames: Option<RichText>,
) {
    if let Some([w, h]) = detail.or(previews.dimensions(entry.key)) {
        widgets::property(ui, "Dimensions", format!("{} px", dimensions([w, h])));
    } else if let Some(message) = previews.failed.get(&entry.key) {
        widgets::property(ui, "Preview", RichText::new(message).color(theme::ERROR));
    } else {
        widgets::property(ui, "Dimensions", pending());
    }
    if let Some(frames) = frames {
        widgets::property(ui, "Frames", frames);
    }
    let format = entry.extension().to_uppercase();
    let format = if format == "PSD" { "PSD (flattened picture)".to_owned() } else { format };
    widgets::property(ui, "Format", format);
}

/// A detail that isn't known yet: its row is shown anyway, so the rows below stay where they are.
fn pending() -> RichText {
    RichText::new("…").color(theme::TEXT_FAINT)
}

/// The asset's name, kind and path.
fn title(ui: &mut Ui, library: &Library, entry: &Entry) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let kind = format!("{} {}", theme::kind_icon(entry.kind), entry.kind.singular().to_uppercase());
        widgets::badge_widget(ui, &kind, theme::kind_color(entry.kind));
        widgets::badge_widget(ui, &entry.extension().to_uppercase(), theme::TEXT_WEAK);
    });
    ui.add(
        egui::Label::new(
            RichText::new(entry.name()).font(FontId::new(theme::size::LARGE, theme::semibold())).color(theme::TEXT),
        )
        .wrap(),
    );
    let path = crate::platform::native(&library.path_of(entry));
    let response = ui.add(
        egui::Label::new(RichText::new(&path).size(theme::size::SMALL).color(theme::TEXT_FAINT))
            .wrap()
            .sense(Sense::click()),
    );
    if response.on_hover_text("Click to copy the path").clicked() {
        ui.ctx().copy_text(path);
    }
}

/// N selected, their size, and their thumbnails to pick from.
fn selection_strip(
    ui: &mut Ui,
    library: &Library,
    selection: &Selection,
    previews: &mut Previews,
    look: Look,
) -> Option<u32> {
    let mut items: Vec<u32> = selection.items.iter().copied().collect();
    items.sort_unstable();
    let total: u64 = items.iter().map(|&index| library.entry(index).size).sum();
    widgets::section_label(ui, &format!("{} selected · {}", grouped(items.len()), format_size(total)));
    let mut counts = [0usize; 3];
    for &index in &items {
        counts[library.entry(index).kind.index()] += 1;
    }
    let summary: Vec<String> = AssetKind::ALL
        .into_iter()
        .filter(|kind| counts[kind.index()] > 0)
        .map(|kind| format!("{} {}", grouped(counts[kind.index()]), kind.label().to_lowercase()))
        .collect();
    ui.label(RichText::new(summary.join(" · ")).color(theme::TEXT_WEAK));

    let mut clicked = None;
    let side = 52.0;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        for &index in items.iter().take(STRIP_MAX) {
            let entry = library.entry(index);
            let (rect, response) = ui.allocate_exact_size(vec2(side, side), Sense::click());
            let focused = selection.focus == Some(index);
            ui.painter().rect_filled(
                rect,
                theme::RADIUS,
                if response.hovered() { theme::CARD_HOVER } else { theme::CARD },
            );
            draw_preview(ui.painter(), rect.shrink(4.0), entry, previews, None, look, false);
            if focused {
                ui.painter().rect_stroke(
                    rect,
                    theme::RADIUS,
                    egui::Stroke::new(1.5, theme::ACCENT),
                    egui::StrokeKind::Inside,
                );
            }
            if response.on_hover_text(entry.name()).clicked() {
                clicked = Some(index);
            }
        }
        if items.len() > STRIP_MAX {
            ui.label(RichText::new(format!("+{}", grouped(items.len() - STRIP_MAX))).color(theme::TEXT_FAINT));
        }
    });
    for &index in items.iter().take(STRIP_MAX) {
        previews.want(index);
    }
    clicked
}

/// Shown when nothing is selected.
fn empty_state(ui: &mut Ui, totals: [u32; 3], kinds: KindSet) {
    ui.add_space(ui.available_height() * 0.25);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(icon::CURSOR_CLICK).size(theme::size::PANE_ICON).color(theme::TEXT_FAINT));
        ui.add_space(6.0);
        ui.label(
            RichText::new("Nothing selected")
                .font(FontId::new(theme::size::TITLE, theme::semibold()))
                .color(theme::TEXT),
        );
        ui.label(
            RichText::new("Click an asset to see it here.\nCtrl- or Shift-click to pick several.")
                .size(theme::size::CAPTION)
                .color(theme::TEXT_WEAK),
        );
        ui.add_space(18.0);
        for kind in AssetKind::ALL.into_iter().filter(|kind| kinds.contains(*kind)) {
            let count = totals[kind.index()];
            let noun = if count == 1 { kind.singular() } else { kind.label() }.to_lowercase();
            ui.label(
                RichText::new(format!("{}  {} {noun}", theme::kind_icon(kind), grouped(count as usize)))
                    .size(theme::size::LABEL)
                    .color(if count > 0 { theme::TEXT_WEAK } else { theme::TEXT_FAINT }),
            );
        }
    });
}

/// Switches the picture to its frames played and back. Lit up with the frame count when frames
/// were found but aren't shown, as an invitation.
fn animation_button(ui: &mut Ui, animating: bool, frames: Option<usize>) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(vec2(if frames.is_some() && !animating { 52.0 } else { 24.0 }, 24.0), Sense::click());
    let painter = ui.painter();
    let invite = frames.is_some() && !animating;
    let fill = if animating {
        theme::ACCENT_SOFT
    } else if response.hovered() {
        theme::CARD_HOVER
    } else if invite {
        theme::ACCENT.gamma_multiply(0.25)
    } else {
        Color32::TRANSPARENT
    };
    painter.rect_filled(rect, theme::RADIUS, fill);
    let color = if animating || invite { theme::ACCENT } else { theme::TEXT_WEAK };
    if invite {
        painter.text(
            pos2(rect.left() + 13.0, rect.center().y),
            Align2::CENTER_CENTER,
            icon::FILM_STRIP,
            FontId::proportional(theme::size::MEDIUM),
            color,
        );
        let count = frames.map(|n| n.to_string()).unwrap_or_default();
        painter.text(
            pos2(rect.right() - 8.0, rect.center().y),
            Align2::RIGHT_CENTER,
            count,
            FontId::proportional(theme::size::CAPTION),
            color,
        );
    } else {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            icon::FILM_STRIP,
            FontId::proportional(theme::size::MEDIUM),
            color,
        );
    }
    let tip = match (animating, frames) {
        (true, _) => "Show the whole picture".to_owned(),
        (false, Some(n)) => format!("Play its {n} frames as an animation"),
        (false, None) => "Play it as a sprite sheet (set the grid after)".to_owned(),
    };
    response.on_hover_text(tip)
}

/// The pane's star: "Favorite", filled once it is one.
fn favorite_button(ui: &mut Ui, favorite: bool) -> egui::Response {
    let label = if favorite { "Favorite" } else { "Add to Favorites" };
    let galley = ui.painter().layout_no_wrap(label.to_owned(), FontId::proportional(theme::size::BODY), theme::TEXT);
    let size = vec2(galley.size().x + 44.0, 28.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let fill = if favorite {
        theme::FAVORITE.gamma_multiply(0.18)
    } else if response.hovered() {
        theme::CARD_HOVER
    } else {
        theme::CARD
    };
    let painter = ui.painter();
    painter.rect_filled(rect, theme::RADIUS, fill);
    let family = if favorite { theme::filled_icons() } else { egui::FontFamily::Proportional };
    let star = if favorite || response.hovered() { theme::FAVORITE } else { theme::TEXT };
    painter.text(
        pos2(rect.left() + 18.0, rect.center().y),
        Align2::CENTER_CENTER,
        icon::STAR,
        FontId::new(theme::size::TITLE, family),
        star,
    );
    painter.galley(pos2(rect.left() + 32.0, rect.center().y - galley.size().y / 2.0), galley, theme::TEXT);
    let tip = keymap::tooltip(&response.ctx, organize::favorite_action(favorite), Action::Favorite);
    response.on_hover_text(tip)
}

/// A stand-in while the preview is made.
fn placeholder(ui: &Ui, rect: Rect, entry: &Entry, previews: &Previews, look: Look) {
    ui.painter().rect_filled(rect, theme::CARD_RADIUS, theme::WELL);
    draw_preview(ui.painter(), rect.shrink(24.0), entry, previews, None, look, true);
}

/// A small spinner in the corner while the large preview loads.
fn spinner_badge(ui: &mut Ui, rect: Rect) {
    let spinner = Rect::from_min_size(rect.left_top() + vec2(10.0, 10.0), vec2(16.0, 16.0));
    ui.put(spinner, egui::Spinner::new().size(theme::size::MEDIUM).color(theme::TEXT_WEAK));
}

/// Buttons over the top-right corner of a preview.
fn overlay(ui: &mut Ui, rect: Rect, add: impl FnOnce(&mut Ui)) {
    let area =
        Rect::from_min_max(pos2(rect.left() + 8.0, rect.top() + 8.0), pos2(rect.right() - 8.0, rect.top() + 36.0));
    let mut child = ui.new_child(UiBuilder::new().max_rect(area).layout(Layout::right_to_left(Align::Min)));
    let frame = egui::Frame::new().fill(theme::shade::OVERLAY).corner_radius(theme::RADIUS).inner_margin(2);
    frame.show(&mut child, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            add(ui);
        });
    });
}
