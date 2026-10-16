//! How assets are shown everywhere: the backdrop behind pictures, and the 3D view's turntable and
//! triangles. The app keeps them with the settings; the views draw with them, and say when the
//! user changes one.

use eframe::egui::{Color32, Ui};
use egui_phosphor::regular as icon;

use super::widgets;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Display {
    pub backdrop: Backdrop,
    /// The 3D view turns the model slowly.
    pub turntable: bool,
    /// The 3D view shows the triangles' edges.
    pub wireframe: bool,
}

impl Default for Display {
    fn default() -> Self {
        Self { backdrop: Backdrop::Checker, turntable: false, wireframe: false }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backdrop {
    Checker,
    Dark,
    Light,
}

impl Backdrop {
    pub const ALL: [Backdrop; 3] = [Self::Checker, Self::Dark, Self::Light];
    pub const LIGHT: Color32 = Color32::from_gray(222);
    /// Text over the light backdrop.
    pub const LIGHT_TEXT: Color32 = Color32::from_gray(90);

    pub fn key(self) -> &'static str {
        match self {
            Self::Checker => "checker",
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|backdrop| backdrop.key() == key)
    }

    pub fn glyph(self) -> &'static str {
        match self {
            Self::Checker => icon::CHECKERBOARD,
            Self::Dark => icon::MOON,
            Self::Light => icon::SUN,
        }
    }

    pub fn tooltip(self) -> &'static str {
        match self {
            Self::Checker => "Checkerboard behind transparency",
            Self::Dark => "Dark background",
            Self::Light => "Light background",
        }
    }
}

/// The backdrop switch shown above the picture.
pub fn backdrop_buttons(ui: &mut Ui, backdrop: &mut Backdrop) {
    for option in Backdrop::ALL {
        if widgets::icon_button_sized(ui, option.glyph(), option.tooltip(), *backdrop == option, 24.0).clicked() {
            *backdrop = option;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Saved in the settings (`image.backdrop`): changing one loses everyone's choice.
    #[test]
    fn stored_backdrop_keys_never_change() {
        assert_eq!(Backdrop::ALL.map(Backdrop::key), ["checker", "dark", "light"]);
        assert_eq!(Backdrop::from_key("light"), Some(Backdrop::Light));
    }
}
