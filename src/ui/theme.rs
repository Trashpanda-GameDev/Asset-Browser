//! The look: a quiet dark palette with one accent colour, the OS's own UI font, Phosphor icons,
//! rounder and roomier widgets than egui's defaults.

use crate::platform::SystemFonts;
use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle,
    Visuals, style::ScrollStyle, vec2,
};

use crate::asset::AssetKind;
use crate::groups;

/// Behind the browsed assets.
pub const BG: Color32 = Color32::from_rgb(15, 16, 19);
/// Header, sidebars, status bar.
pub const PANEL: Color32 = Color32::from_rgb(21, 22, 26);
/// Tiles, inputs, buttons.
pub const CARD: Color32 = Color32::from_rgb(29, 31, 36);
pub const CARD_HOVER: Color32 = Color32::from_rgb(37, 40, 46);
pub const CARD_ACTIVE: Color32 = Color32::from_rgb(46, 49, 57);
/// Recessed areas: the waveform and 3D wells, the image background.
pub const WELL: Color32 = Color32::from_rgb(11, 12, 14);
pub const BORDER: Color32 = Color32::from_rgb(38, 41, 48);
pub const TEXT: Color32 = Color32::from_rgb(228, 230, 235);
pub const TEXT_WEAK: Color32 = Color32::from_rgb(150, 154, 164);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(100, 104, 115);
pub const ACCENT: Color32 = Color32::from_rgb(108, 140, 255);
/// Selected rows and tiles: the accent, see-through.
pub const ACCENT_SOFT: Color32 = Color32::from_rgba_unmultiplied_const(108, 140, 255, 46);
pub const ERROR: Color32 = Color32::from_rgb(244, 102, 102);
/// The favourite star.
pub const FAVORITE: Color32 = Color32::from_rgb(250, 199, 72);
/// What categories are coloured with, in the order new ones take them.
pub const CATEGORY_COLORS: [Color32; groups::CATEGORY_COLOR_COUNT as usize] = [
    Color32::from_rgb(239, 99, 99),
    Color32::from_rgb(242, 153, 74),
    Color32::from_rgb(236, 201, 75),
    Color32::from_rgb(104, 196, 104),
    Color32::from_rgb(64, 190, 180),
    Color32::from_rgb(92, 152, 240),
    Color32::from_rgb(167, 123, 240),
    Color32::from_rgb(236, 112, 178),
];

/// Each kind of asset's colour: tile accents, waveforms, badges.
pub fn kind_color(kind: AssetKind) -> Color32 {
    match kind {
        AssetKind::Image => Color32::from_rgb(92, 168, 255),
        AssetKind::Audio => Color32::from_rgb(255, 176, 72),
        AssetKind::Model => Color32::from_rgb(84, 210, 146),
    }
}

/// Each kind of asset's icon.
pub fn kind_icon(kind: AssetKind) -> &'static str {
    match kind {
        AssetKind::Image => egui_phosphor::regular::IMAGE,
        AssetKind::Audio => egui_phosphor::regular::WAVEFORM,
        AssetKind::Model => egui_phosphor::regular::CUBE,
    }
}

pub const RADIUS: u8 = 6;
pub const CARD_RADIUS: u8 = 8;

pub fn category_color(index: u8) -> Color32 {
    CATEGORY_COLORS[index as usize % CATEGORY_COLORS.len()]
}

/// The window's title bar in the header's colour, so the two read as one bar
/// (`platform::color_title_bar`).
pub fn color_title_bar(ctx: &egui::Context, window: &impl raw_window_handle::HasWindowHandle) {
    let rgb = |color: Color32| [color.r(), color.g(), color.b()];
    // Dark wherever the OS has a dark title bar; exactly the header's colour on Windows 11.
    ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(egui::viewport::SystemTheme::Dark));
    crate::platform::color_title_bar(window, rgb(PANEL), rgb(TEXT_WEAK), rgb(BORDER));
}

/// Text and icons on the accent colour.
pub const ON_ACCENT: Color32 = Color32::WHITE;
/// Where a sound is, over its waveform.
pub const PLAYHEAD: Color32 = Color32::WHITE;
/// The toast's background, a step above the panels.
pub const TOAST: Color32 = Color32::from_rgb(40, 43, 51);
/// A sound's tile, top to bottom.
pub const SOUND_TILE: [Color32; 2] = [Color32::from_rgb(44, 47, 56), Color32::from_rgb(29, 31, 37)];
/// The top of the 3D view's backdrop, which darkens to `WELL` at the bottom.
pub const VIEWPORT_TOP: Color32 = Color32::from_rgb(34, 37, 44);
/// The checkerboard behind transparency: its light and dark squares.
pub const CHECKER: [Color32; 2] = [Color32::from_gray(46), Color32::from_gray(36)];
/// Over the app, behind a window such as the settings.
pub const SCRIM: Color32 = Color32::from_black_alpha(150);

/// Dark shades behind what's drawn over pictures, so it reads on light ones too.
pub mod shade {
    use eframe::egui::Color32;

    pub const BADGE: Color32 = Color32::from_black_alpha(150);
    /// The pane's buttons over a picture or model.
    pub const OVERLAY: Color32 = Color32::from_black_alpha(140);
    /// Around a category's dot on a tile.
    pub const DOT: Color32 = Color32::from_black_alpha(120);
    /// Around a tile's star: a favourite's, and the outline to click.
    pub const FAVORITE: Color32 = Color32::from_black_alpha(110);
    pub const STAR: Color32 = Color32::from_black_alpha(80);
}

/// Text sizes, smallest first: every size the window draws text and icons at, named for what
/// they're used for. New text takes the one that fits rather than a number of its own.
pub mod size {
    /// File types on tiles, section labels in capitals.
    pub const TINY: f32 = 10.5;
    /// Print over pictures and waveforms (hints, times), and small marks such as × and ›.
    pub const FINE: f32 = 11.0;
    /// Notes, counts beside names, the status bar (egui's Small).
    pub const SMALL: f32 = 11.5;
    /// The toolbar's counts, chips, the sound's clock; spinners beside small text.
    pub const CAPTION: f32 = 12.0;
    /// Names under tiles, the list's columns, properties, a question's note (egui's Monospace).
    pub const LABEL: f32 = 12.5;
    /// Most text: rows, menus, buttons (egui's Body and Button).
    pub const BODY: f32 = 13.0;
    /// Where the user is, in the toolbar; What's new's notes.
    pub const EMPHASIS: f32 = 13.5;
    /// Small headings, a view's empty message; icons beside text, spinners beside headings.
    pub const MEDIUM: f32 = 14.0;
    /// A question's title, a release's name, the empty pane's; icons on their own.
    pub const TITLE: f32 = 15.0;
    /// The pane's asset name, the settings' version, the welcome, icons in buttons.
    pub const LARGE: f32 = 16.0;
    /// egui's Heading.
    pub const HEADING: f32 = 17.0;
    /// The hint over the window while a folder is dragged onto it.
    pub const BANNER: f32 = 18.0;
    /// A window's title: the settings' page, What's new.
    pub const WINDOW_TITLE: f32 = 20.0;
    /// The icon in the empty preview pane.
    pub const PANE_ICON: f32 = 40.0;
    /// The icon in the welcome, before any folder is added.
    pub const WELCOME_ICON: f32 = 44.0;
}

/// The family used for titles.
pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}

/// Phosphor's filled icons (a filled star for favourites); the outlined ones are in every family.
pub fn filled_icons() -> FontFamily {
    FontFamily::Name("phosphor-fill".into())
}

/// Sets the look up, with the OS's own UI fonts where they were found.
pub fn apply(ctx: &egui::Context, system: SystemFonts) {
    ctx.set_fonts(fonts(system));
    ctx.all_styles_mut(|style| {
        style.visuals = visuals();
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(size::SMALL)),
            (TextStyle::Body, FontId::proportional(size::BODY)),
            (TextStyle::Button, FontId::proportional(size::BODY)),
            (TextStyle::Heading, FontId::new(size::HEADING, semibold())),
            (TextStyle::Monospace, FontId::monospace(size::LABEL)),
        ]
        .into();
        let spacing = &mut style.spacing;
        spacing.item_spacing = vec2(8.0, 6.0);
        spacing.button_padding = vec2(10.0, 4.0);
        spacing.interact_size = vec2(28.0, 26.0);
        spacing.window_margin = Margin::same(12);
        spacing.menu_margin = Margin::same(6);
        spacing.indent = 14.0;
        spacing.slider_width = 110.0;
        spacing.slider_rail_height = 4.0;
        spacing.icon_width = 14.0;
        spacing.icon_width_inner = 8.0;
        spacing.scroll = ScrollStyle { bar_width: 8.0, floating_width: 3.0, ..ScrollStyle::floating() };
        style.interaction.selectable_labels = false;
        style.animation_time = 0.12;
    });
}

fn visuals() -> Visuals {
    let mut v = Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = Color32::from_rgb(26, 28, 33);
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_shadow = Shadow { offset: [0, 10], blur: 28, spread: 0, color: Color32::from_black_alpha(120) };
    v.popup_shadow = Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(110) };
    v.extreme_bg_color = WELL;
    v.text_edit_bg_color = Some(CARD);
    v.faint_bg_color = CARD;
    v.code_bg_color = CARD;
    v.hyperlink_color = ACCENT;
    v.error_fg_color = ERROR;
    v.selection.bg_fill = ACCENT.gamma_multiply(0.55);
    v.selection.stroke = Stroke::new(1.0, TEXT);
    v.text_cursor.stroke = Stroke::new(2.0, ACCENT);
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Circle;
    v.indent_has_left_vline = false;
    v.collapsing_header_frame = false;
    v.striped = false;
    v.interact_cursor = Some(egui::CursorIcon::PointingHand);

    let radius = CornerRadius::same(RADIUS);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = PANEL;
    w.noninteractive.weak_bg_fill = PANEL;
    w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    w.noninteractive.corner_radius = radius;
    w.inactive.bg_fill = CARD;
    w.inactive.weak_bg_fill = CARD;
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    w.inactive.corner_radius = radius;
    w.hovered.bg_fill = CARD_HOVER;
    w.hovered.weak_bg_fill = CARD_HOVER;
    w.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(56, 60, 70));
    w.hovered.fg_stroke = Stroke::new(1.5, TEXT);
    w.hovered.corner_radius = radius;
    w.hovered.expansion = 0.0;
    w.active.bg_fill = CARD_ACTIVE;
    w.active.weak_bg_fill = CARD_ACTIVE;
    w.active.bg_stroke = Stroke::new(1.0, ACCENT);
    w.active.fg_stroke = Stroke::new(2.0, TEXT);
    w.active.corner_radius = radius;
    w.active.expansion = 0.0;
    w.open = w.hovered;
    v
}

/// The OS's UI font when it can be found (Segoe UI on Windows), with Phosphor icons mixed in, and
/// egui's own fonts behind them for anything else (emoji, rare scripts).
fn fonts(system: SystemFonts) -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let proportional = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mut semibold_family = proportional.clone();

    if let Some(bytes) = system.regular {
        fonts.font_data.insert("ui".into(), FontData::from_owned(bytes).into());
        if let Some(family) = fonts.families.get_mut(&FontFamily::Proportional) {
            family.insert(0, "ui".into());
        }
        semibold_family.insert(0, "ui".into());
    }
    if let Some(bytes) = system.semibold {
        fonts.font_data.insert("ui-semibold".into(), FontData::from_owned(bytes).into());
        semibold_family.insert(0, "ui-semibold".into());
    }
    if let Some(bytes) = system.monospace {
        fonts.font_data.insert("ui-mono".into(), FontData::from_owned(bytes).into());
        if let Some(family) = fonts.families.get_mut(&FontFamily::Monospace) {
            family.insert(0, "ui-mono".into());
        }
    }
    // Icons go right after the text font, so they're found in ordinary labels.
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    egui_phosphor::add_font_bytes_as_family(&mut fonts, "phosphor-fill", egui_phosphor::Variant::Fill.font_bytes());
    semibold_family.insert(semibold_family.len().min(1), "phosphor".into());
    fonts.families.insert(semibold(), semibold_family);
    fonts
}
