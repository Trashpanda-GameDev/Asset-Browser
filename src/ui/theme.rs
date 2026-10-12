//! The look: a quiet dark palette with one accent colour, the OS's own UI font, Phosphor icons,
//! rounder and roomier widgets than egui's defaults.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle,
    Visuals, style::ScrollStyle, vec2,
};

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
/// What categories are coloured with, in the order new ones take them (`groups::CATEGORY_COLORS`).
pub const CATEGORY_COLORS: [Color32; 8] = [
    Color32::from_rgb(239, 99, 99),
    Color32::from_rgb(242, 153, 74),
    Color32::from_rgb(236, 201, 75),
    Color32::from_rgb(104, 196, 104),
    Color32::from_rgb(64, 190, 180),
    Color32::from_rgb(92, 152, 240),
    Color32::from_rgb(167, 123, 240),
    Color32::from_rgb(236, 112, 178),
];

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

/// The family used for titles.
pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}

/// Phosphor's filled icons (a filled star for favourites); the outlined ones are in every family.
pub fn filled_icons() -> FontFamily {
    FontFamily::Name("phosphor-fill".into())
}

pub fn apply(ctx: &egui::Context) {
    ctx.set_fonts(fonts());
    ctx.all_styles_mut(|style| {
        style.visuals = visuals();
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(11.5)),
            (TextStyle::Body, FontId::proportional(13.0)),
            (TextStyle::Button, FontId::proportional(13.0)),
            (TextStyle::Heading, FontId::new(17.0, semibold())),
            (TextStyle::Monospace, FontId::monospace(12.5)),
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
fn fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let proportional = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mut semibold_family = proportional.clone();

    if let Some(bytes) = read_first(REGULAR_FONTS) {
        fonts.font_data.insert("ui".into(), FontData::from_owned(bytes).into());
        if let Some(family) = fonts.families.get_mut(&FontFamily::Proportional) {
            family.insert(0, "ui".into());
        }
        semibold_family.insert(0, "ui".into());
    }
    if let Some(bytes) = read_first(SEMIBOLD_FONTS) {
        fonts.font_data.insert("ui-semibold".into(), FontData::from_owned(bytes).into());
        semibold_family.insert(0, "ui-semibold".into());
    }
    if let Some(bytes) = read_first(MONOSPACE_FONTS) {
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

#[cfg(windows)]
const REGULAR_FONTS: &[&str] = &["%WINDIR%/Fonts/segoeui.ttf"];
#[cfg(windows)]
const SEMIBOLD_FONTS: &[&str] = &["%WINDIR%/Fonts/seguisb.ttf", "%WINDIR%/Fonts/segoeuib.ttf"];
#[cfg(windows)]
const MONOSPACE_FONTS: &[&str] = &["%WINDIR%/Fonts/CascadiaMono.ttf", "%WINDIR%/Fonts/consola.ttf"];

#[cfg(target_os = "macos")]
const REGULAR_FONTS: &[&str] = &["/System/Library/Fonts/SFNS.ttf"];
#[cfg(target_os = "macos")]
const SEMIBOLD_FONTS: &[&str] = &[];
#[cfg(target_os = "macos")]
const MONOSPACE_FONTS: &[&str] = &["/System/Library/Fonts/SFNSMono.ttf"];

#[cfg(not(any(windows, target_os = "macos")))]
const REGULAR_FONTS: &[&str] = &[
    "/usr/share/fonts/truetype/inter/Inter-Regular.ttf",
    "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
];
#[cfg(not(any(windows, target_os = "macos")))]
const SEMIBOLD_FONTS: &[&str] = &[
    "/usr/share/fonts/truetype/inter/Inter-SemiBold.ttf",
    "/usr/share/fonts/truetype/noto/NotoSans-SemiBold.ttf",
    "/usr/share/fonts/noto/NotoSans-SemiBold.ttf",
];
#[cfg(not(any(windows, target_os = "macos")))]
const MONOSPACE_FONTS: &[&str] = &["/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"];

fn read_first(candidates: &[&str]) -> Option<Vec<u8>> {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".to_owned());
    candidates.iter().find_map(|path| std::fs::read(path.replace("%WINDIR%", &windir)).ok())
}
