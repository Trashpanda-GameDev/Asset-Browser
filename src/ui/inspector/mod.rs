//! The inspector: the pane on the right showing the selected asset large, with what's known about
//! it and what can be done with it. Each kind has its own section; several selected assets get a
//! summary and a strip of thumbnails.

use std::sync::Arc;

use eframe::egui::{
    self, Align, Align2, ColorImage, FontId, Layout, Rect, RichText, Sense, TextureFilter, TextureHandle,
    TextureOptions, Ui, UiBuilder, pos2, vec2,
};
use eframe::egui_wgpu::RenderState;
use egui_phosphor::regular as icon;

use crate::asset::{AssetKind, KindSet};
use crate::detail::{Detail, DetailModel, Loaded};
use crate::groups::{FAVORITES, GroupKind, Groups, Membership};
use crate::imaging::Pixels;
use crate::library::{Entry, Library};
use crate::playback::Playback;
use crate::sheets::{Saved, Sheet};

use super::asset_actions::{self, AssetAction};
use super::browse::{Look, PreviewOf, draw_preview};
use super::display::Display;
use super::format::{format_age, format_size};
use super::image_view::ImageView;
use super::keymap::{self, Action};
use super::mode::Selection;
use super::organize::{self, GroupCommand, Targets};
use super::previews::Previews;
use super::sheet_view::SheetView;
use super::sound::SoundCommand;
use super::viewport::Viewport;
use super::{theme, widgets};

mod audio;
mod image;
mod model;
mod selection;

use audio::{sound_properties, sound_section};
use image::image_properties;
use selection::{empty_state, selection_strip};

#[derive(Default)]
pub struct Inspector {
    pub image: ImageView,
    pub viewport: Viewport,
    pub sheet: SheetView,
    /// The display as the app has it, with what the pane's buttons change this frame: handed
    /// back in the output.
    display: Display,
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
    Model(Arc<DetailModel>),
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
            Err(error) => DetailState::Failed(error.to_string()),
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

    pub fn show(&mut self, ui: &mut Ui, mut args: InspectorArgs<'_>) -> InspectorOutput {
        let mut output = InspectorOutput::default();
        // The picture and 3D views draw with the app's display; their buttons change it here, to
        // be handed back.
        let display = args.display;
        self.display = display;
        (self.viewport.auto_rotate, self.viewport.wireframe) = (display.turntable, display.wireframe);
        let library = args.library;
        let Some(focus) = args.selection.focus.filter(|&focus| (focus as usize) < library.entries().len()) else {
            self.viewport.release();
            empty_state(ui, args.totals, args.kinds);
            return output;
        };
        output.load_detail = self.follow(ui, &mut args, focus);
        egui::ScrollArea::vertical().id_salt("inspector").auto_shrink([false; 2]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            self.sections(ui, &mut args, focus, &mut output);
        });
        output.sheet_choices = self.sheet.take_choices();
        let shown = Display {
            backdrop: self.display.backdrop,
            turntable: self.viewport.auto_rotate,
            wireframe: self.viewport.wireframe,
        };
        output.display = (shown != display).then_some(shown);
        output
    }

    /// Gets ready for the focused asset: asks for its thumbnail and large preview, lets go of the
    /// last one's, and hands a picture to the sprite sheet player. The entry whose large preview
    /// is to be loaded, if it is.
    fn follow(&mut self, ui: &Ui, args: &mut InspectorArgs<'_>, focus: u32) -> Option<u32> {
        let entry = args.library.entry(focus);
        args.previews.want(focus);
        let load = self.want_detail(entry, args.previews).then_some(focus);
        if entry.kind != AssetKind::Model {
            self.viewport.release();
        }
        if entry.kind == AssetKind::Image {
            self.load_sheet(args.library, entry, args.sheet_choices, ui.input(|input| input.time));
        } else {
            self.sheet.clear();
        }
        load
    }

    /// The pane, top to bottom: the asset large, its name, what can be done with it, its groups,
    /// its details, a sprite sheet's player, and the rest of the selection.
    fn sections(&mut self, ui: &mut Ui, args: &mut InspectorArgs<'_>, focus: u32, output: &mut InspectorOutput) {
        let (library, entry) = (args.library, args.library.entry(focus));
        let width = ui.available_width();
        match entry.kind {
            AssetKind::Image => self.image_section(ui, entry, args.previews, width),
            AssetKind::Audio => output.sound = sound_section(ui, library, focus, args.previews, args.playback),
            AssetKind::Model => self.model_section(ui, entry, args, width),
        }
        ui.add_space(4.0);
        title(ui, library, entry);
        let targets = targets(args.selection, focus);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            output.action = asset_actions::buttons(ui, targets.len());
            if favorite_button(ui, organize::all_in(args.membership, FAVORITES, &targets)).clicked() {
                output.command = Some(GroupCommand::ToggleFavorite(Targets::Selection));
            }
        });
        ui.add_space(6.0);
        for kind in [GroupKind::Collection, GroupKind::Category] {
            widgets::section_label(ui, kind.title());
            if let Some(command) = organize::chips(ui, args.groups, args.membership, &targets, kind) {
                output.command = Some(command);
            }
            ui.add_space(4.0);
        }
        self.details(ui, args, entry);
        // Below the details: a sprite sheet's or GIF's frames load after the picture is shown, and
        // the player arriving here moves nothing above it.
        if entry.kind == AssetKind::Image {
            self.animation_section(ui, entry);
        }
        if args.selection.len() > 1 {
            ui.add_space(10.0);
            let look = Look::new(ui, self.display.backdrop);
            if let Some(index) = selection_strip(ui, library, args.selection, args.previews, look) {
                output.focus = Some(index);
            }
        }
    }

    /// The asset's details. Every row is there from the start, with "…" until its value is
    /// known, so moving through the assets with the keys doesn't make the rows below jump as
    /// values arrive.
    fn details(&self, ui: &mut Ui, args: &InspectorArgs<'_>, entry: &Entry) {
        widgets::section_label(ui, "Details");
        match entry.kind {
            AssetKind::Image => {
                image_properties(ui, entry, args.previews, self.detail_size(entry.key), self.gif_frames(entry));
            }
            AssetKind::Audio => sound_properties(ui, entry, args.previews),
            AssetKind::Model => self.model_properties(ui, entry, args.previews),
        }
        widgets::property(ui, "File size", format_size(entry.size));
        widgets::property(ui, "Modified", format_age(entry.mtime));
        // From the library folder it's in.
        let root = &args.library.root(entry.root).name;
        let folder = if entry.folder().is_empty() {
            root.clone()
        } else {
            format!("{root} › {}", entry.folder().replace('/', " › "))
        };
        widgets::property(ui, "Folder", RichText::new(folder).color(theme::TEXT_WEAK));
        // Last, since it only appears once the sound plays.
        if entry.kind == AssetKind::Audio
            && let Some(ready) = args.playback.now_for(entry.key).and_then(|now| now.ready_ms)
        {
            widgets::property(ui, "Started in", format!("{ready:.0} ms"))
                .on_hover_text("From the request to the first audio handed to the sound card");
        }
    }
}

/// A detail that isn't known yet: its row is shown anyway, so the rows below stay where they are.
fn pending() -> RichText {
    RichText::new("…").color(theme::TEXT_FAINT)
}

/// What the pane's buttons act on: the whole selection, in order, when the focus is part of it.
fn targets(selection: &Selection, focus: u32) -> Vec<u32> {
    if selection.contains(focus) {
        let mut items: Vec<u32> = selection.items.iter().copied().collect();
        items.sort_unstable();
        items
    } else {
        vec![focus]
    }
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
    let preview = PreviewOf { entry, previews, playing: None, large: true };
    draw_preview(ui.painter(), rect.shrink(24.0), &preview, look);
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
