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

use crate::asset::AssetKind;
use crate::detail::{Detail, DetailLoader};
use crate::library::{Entry, Library};
use crate::model::Model;
use crate::thumbs::THUMB_MAX;

use super::browse::{Look, MenuAction, channels, draw_preview};
use super::format::{format_age, format_size, format_time, grouped};
use super::image_view::{self, ImageView, Picture};
use super::mode::Selection;
use super::previews::Previews;
use super::sound::{self, Sound};
use super::viewport::Viewport;
use super::{theme, widgets};

/// Thumbnails shown for a multiple selection; the rest are counted.
const STRIP_MAX: usize = 48;

pub struct Inspector {
    pub image: ImageView,
    pub viewport: Viewport,
    loader: DetailLoader,
    /// The large preview of the focused asset, by key.
    detail: Option<(i64, DetailState)>,
}

enum DetailState {
    Loading,
    Image { texture: TextureHandle, size: [u32; 2] },
    Model(Arc<Model>),
    Failed(String),
}

pub struct InspectorArgs<'a> {
    pub library: &'a Library,
    pub selection: &'a Selection,
    pub previews: &'a mut Previews,
    pub sound: &'a mut Sound,
    pub render_state: Option<&'a RenderState>,
    /// The folder being browsed, for the empty state.
    pub dir: u32,
    pub kinds: [bool; 3],
}

#[derive(Default)]
pub struct InspectorOutput {
    /// A thumbnail in the selection strip was clicked: show that one.
    pub focus: Option<u32>,
    pub action: Option<MenuAction>,
}

impl Inspector {
    pub fn new(ctx: &egui::Context) -> Self {
        let ctx = ctx.clone();
        Self {
            image: ImageView::default(),
            viewport: Viewport::default(),
            loader: DetailLoader::start(Arc::new(move || ctx.request_repaint())),
            detail: None,
        }
    }

    /// Takes in a finished large preview.
    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Some(loaded) = self.loader.try_recv() {
            let Some((key, state)) = &mut self.detail else { continue };
            if *key != loaded.key {
                continue; // the selection moved on
            }
            *state = match loaded.result {
                Ok(Detail::Image(pixels)) => {
                    let image =
                        ColorImage::from_rgba_unmultiplied([pixels.w as usize, pixels.h as usize], &pixels.rgba);
                    let options = TextureOptions {
                        magnification: TextureFilter::Nearest,
                        minification: TextureFilter::Linear,
                        mipmap_mode: Some(TextureFilter::Linear),
                        ..TextureOptions::LINEAR
                    };
                    let texture = ctx.load_texture(format!("detail-{key}"), image, options);
                    DetailState::Image { texture, size: [pixels.src_w, pixels.src_h] }
                }
                Ok(Detail::Model(model)) => DetailState::Model(model),
                Err(message) => DetailState::Failed(message),
            };
        }
    }

    /// Asks for the large preview of `entry` if it needs one and hasn't got it.
    fn want_detail(&mut self, library: &Library, entry: &Entry, previews: &Previews) {
        if self.detail.as_ref().is_some_and(|(key, _)| *key == entry.key) {
            return;
        }
        let wanted = match entry.kind {
            // Waits for the thumbnail, which says how big the picture is; small pictures are their
            // own thumbnail already.
            AssetKind::Image => previews.dimensions(entry.key).is_some_and(|[w, h]| w > THUMB_MAX || h > THUMB_MAX),
            // Blender files only have the preview picture Blender saved.
            AssetKind::Model => !entry.extension().eq_ignore_ascii_case("blend"),
            AssetKind::Audio => false,
        };
        if wanted {
            self.detail = Some((entry.key, DetailState::Loading));
            self.loader.request(entry.key, library.path_of(entry), entry.kind);
        } else {
            // Lets go of the last one's picture or model.
            self.detail = None;
        }
    }

    pub fn show(&mut self, ui: &mut Ui, args: InspectorArgs<'_>) -> InspectorOutput {
        let mut output = InspectorOutput::default();
        let InspectorArgs { library, selection, previews, sound, render_state, dir, kinds } = args;
        let Some(focus) = selection.focus.filter(|&focus| (focus as usize) < library.entries.len()) else {
            self.viewport.release();
            empty_state(ui, library, dir, kinds);
            return output;
        };
        let entry = &library.entries[focus as usize];
        previews.want(focus);
        self.want_detail(library, entry, previews);
        if entry.kind != AssetKind::Model {
            self.viewport.release();
        }

        egui::ScrollArea::vertical().id_salt("inspector").auto_shrink([false; 2]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            let width = ui.available_width();
            match entry.kind {
                AssetKind::Image => self.image_section(ui, entry, previews, width),
                AssetKind::Audio => sound_section(ui, library, focus, previews, sound),
                AssetKind::Model => self.model_section(ui, entry, previews, render_state, width),
            }
            ui.add_space(4.0);
            title(ui, library, entry);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                let several = selection.len() > 1 && selection.contains(focus);
                if widgets::button(ui, icon::ARROW_SQUARE_OUT, "Open")
                    .on_hover_text("Open in its default app (Enter)")
                    .clicked()
                {
                    output.action = Some(MenuAction::Open);
                }
                if widgets::button(ui, icon::FOLDER_OPEN, "Show in folder").clicked() {
                    output.action = Some(MenuAction::Reveal);
                }
                let copy = if several { format!("Copy {} paths", selection.len()) } else { "Copy path".to_owned() };
                if widgets::button(ui, icon::COPY, &copy).clicked() {
                    output.action = Some(MenuAction::CopyPaths);
                }
            });
            ui.add_space(6.0);
            widgets::section_label(ui, "Details");
            match entry.kind {
                AssetKind::Image => image_properties(ui, entry, previews, self.detail_size(entry.key)),
                AssetKind::Audio => sound_properties(ui, entry, previews, sound),
                AssetKind::Model => self.model_properties(ui, entry, previews),
            }
            widgets::property(ui, "File size", format_size(entry.size));
            widgets::property(ui, "Modified", format_age(entry.mtime));
            let folder = if entry.folder().is_empty() {
                "(top folder)".to_owned()
            } else {
                entry.folder().replace('/', " › ")
            };
            widgets::property(ui, "Folder", RichText::new(folder).color(theme::TEXT_WEAK));

            if selection.len() > 1 {
                ui.add_space(10.0);
                let look = Look::new(ui, self.image.backdrop);
                if let Some(index) = selection_strip(ui, library, selection, previews, look) {
                    output.focus = Some(index);
                }
            }
        });
        output
    }

    fn detail_size(&self, key: i64) -> Option<[u32; 2]> {
        match &self.detail {
            Some((detail_key, DetailState::Image { size, .. })) if *detail_key == key => Some(*size),
            _ => None,
        }
    }

    fn image_section(&mut self, ui: &mut Ui, entry: &Entry, previews: &Previews, width: f32) {
        let height = (width * 0.85).clamp(180.0, 440.0);
        let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        let thumbnail = previews.texture(entry.key);
        let picture = match (&self.detail, thumbnail) {
            (Some((key, DetailState::Image { texture, size })), _) if *key == entry.key => {
                Some(Picture { texture: texture.id(), size: *size, pixel_art: false })
            }
            (_, Some(thumbnail)) => Some(Picture {
                texture: thumbnail.handle.id(),
                size: thumbnail.src_size,
                pixel_art: thumbnail.unscaled,
            }),
            _ => None,
        };
        match picture {
            Some(picture) => self.image.show(ui, rect, entry.key, &picture, previews.checker.id()),
            None => placeholder(ui, rect, entry, previews, Look::new(ui, self.image.backdrop)),
        }
        if matches!(&self.detail, Some((key, DetailState::Loading)) if *key == entry.key) {
            spinner_badge(ui, rect);
        }
        overlay(ui, rect, |ui| image_view::backdrop_buttons(ui, &mut self.image.backdrop));
    }

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
                FontId::proportional(11.5),
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

fn sound_section(ui: &mut Ui, library: &Library, focus: u32, previews: &Previews, sound: &mut Sound) {
    let entry = &library.entries[focus as usize];
    let waveform = previews.waveforms.get(&entry.key).map(|waveform| waveform.as_ref());
    sound::scrubber(ui, 120.0, sound, library, focus, waveform, previews.failed.contains_key(&entry.key));
    let duration =
        sound.now_for(entry.key).and_then(|now| now.duration).or(waveform.map(|waveform| waveform.duration as f64));
    sound::transport(ui, sound, library, focus, duration);
}

fn sound_properties(ui: &mut Ui, entry: &Entry, previews: &Previews, sound: &Sound) {
    if let Some(waveform) = previews.waveforms.get(&entry.key) {
        widgets::property(ui, "Length", format_time(waveform.duration as f64));
        widgets::property(ui, "Sample rate", format!("{:.1} kHz", waveform.sample_rate as f32 / 1000.0));
        widgets::property(ui, "Channels", channels(waveform.channels));
    } else if let Some(message) = previews.failed.get(&entry.key) {
        widgets::property(ui, "Preview", RichText::new(message).color(theme::ERROR));
    }
    widgets::property(ui, "Format", entry.extension().to_uppercase());
    if let Some(ready) = sound.now_for(entry.key).and_then(|now| now.ready_ms) {
        widgets::property(ui, "Started in", format!("{ready:.0} ms"))
            .on_hover_text("From the request to the first audio handed to the sound card");
    }
}

fn image_properties(ui: &mut Ui, entry: &Entry, previews: &Previews, detail: Option<[u32; 2]>) {
    if let Some([w, h]) = detail.or(previews.dimensions(entry.key)) {
        widgets::property(ui, "Dimensions", format!("{w} × {h} px"));
    } else if let Some(message) = previews.failed.get(&entry.key) {
        widgets::property(ui, "Preview", RichText::new(message).color(theme::ERROR));
    }
    let format = entry.extension().to_uppercase();
    let format = if format == "PSD" { "PSD (flattened picture)".to_owned() } else { format };
    widgets::property(ui, "Format", format);
}

/// The asset's name, kind and path.
fn title(ui: &mut Ui, library: &Library, entry: &Entry) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let kind = format!("{} {}", entry.kind.icon(), entry.kind.singular().to_uppercase());
        widgets::badge_widget(ui, &kind, entry.kind.color());
        widgets::badge_widget(ui, &entry.extension().to_uppercase(), theme::TEXT_WEAK);
    });
    ui.add(
        egui::Label::new(RichText::new(entry.name()).font(FontId::new(16.0, theme::semibold())).color(theme::TEXT))
            .wrap(),
    );
    let path = crate::platform::native(&library.path_of(entry));
    let response =
        ui.add(egui::Label::new(RichText::new(&path).size(11.5).color(theme::TEXT_FAINT)).wrap().sense(Sense::click()));
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
    let total: u64 = items.iter().map(|&index| library.entries[index as usize].size).sum();
    widgets::section_label(ui, &format!("{} selected · {}", grouped(items.len()), format_size(total)));
    let mut counts = [0usize; 3];
    for &index in &items {
        counts[library.entries[index as usize].kind.index()] += 1;
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
            let entry = &library.entries[index as usize];
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
fn empty_state(ui: &mut Ui, library: &Library, dir: u32, kinds: [bool; 3]) {
    ui.add_space(ui.available_height() * 0.25);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(icon::CURSOR_CLICK).size(40.0).color(theme::TEXT_FAINT));
        ui.add_space(6.0);
        ui.label(RichText::new("Nothing selected").font(FontId::new(15.0, theme::semibold())).color(theme::TEXT));
        ui.label(
            RichText::new("Click an asset to see it here.\nCtrl- or Shift-click to pick several.")
                .size(12.0)
                .color(theme::TEXT_WEAK),
        );
        if let Some(node) = library.dirs.get(dir as usize) {
            ui.add_space(18.0);
            for kind in AssetKind::ALL.into_iter().filter(|kind| kinds[kind.index()]) {
                let count = node.totals[kind.index()];
                ui.label(
                    RichText::new(format!(
                        "{}  {} {}",
                        kind.icon(),
                        grouped(count as usize),
                        kind.label().to_lowercase()
                    ))
                    .size(12.5)
                    .color(if count > 0 { theme::TEXT_WEAK } else { theme::TEXT_FAINT }),
                );
            }
        }
    });
}

/// A stand-in while the preview is made.
fn placeholder(ui: &Ui, rect: Rect, entry: &Entry, previews: &Previews, look: Look) {
    ui.painter().rect_filled(rect, theme::CARD_RADIUS, theme::WELL);
    draw_preview(ui.painter(), rect.shrink(24.0), entry, previews, None, look, true);
}

/// A small spinner in the corner while the large preview loads.
fn spinner_badge(ui: &mut Ui, rect: Rect) {
    let spinner = Rect::from_min_size(rect.left_top() + vec2(10.0, 10.0), vec2(16.0, 16.0));
    ui.put(spinner, egui::Spinner::new().size(14.0).color(theme::TEXT_WEAK));
}

/// Buttons over the top-right corner of a preview.
fn overlay(ui: &mut Ui, rect: Rect, add: impl FnOnce(&mut Ui)) {
    let area =
        Rect::from_min_max(pos2(rect.left() + 8.0, rect.top() + 8.0), pos2(rect.right() - 8.0, rect.top() + 36.0));
    let mut child = ui.new_child(UiBuilder::new().max_rect(area).layout(Layout::right_to_left(Align::Min)));
    let frame = egui::Frame::new().fill(Color32::from_black_alpha(140)).corner_radius(theme::RADIUS).inner_margin(2);
    frame.show(&mut child, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            add(ui);
        });
    });
}
