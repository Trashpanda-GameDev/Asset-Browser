// Release builds open no console window on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use asset_browser::app::BrowserApp;
use eframe::egui;

fn main() -> eframe::Result {
    // Before anything else: the setup program and the updater start the app to tell it about an
    // install, update or uninstall, and it exits straight after. And a downloaded update that
    // didn't get installed when the app closed is installed now, restarting it.
    velopack::VelopackApp::build().run();
    asset_browser::diag::install_panic_hook();
    // An optional folder to add to the library (or show, if it's in already).
    let start_root = std::env::args_os().nth(1).map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Asset Browser")
            .with_icon(window_icon())
            .with_inner_size([1440.0, 880.0])
            .with_min_inner_size([760.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native("Asset Browser", options, Box::new(move |cc| Ok(Box::new(BrowserApp::new(cc, start_root)))))
}

/// The title bar's, taskbar's and Alt+Tab's icon: the largest picture in the app icon, which
/// eframe scales to each size. (An empty one means no icon rather than eframe's own.)
fn window_icon() -> egui::IconData {
    match image::load_from_memory(include_bytes!("../assets/icon.ico")) {
        Ok(picture) => {
            let picture = picture.into_rgba8();
            egui::IconData { width: picture.width(), height: picture.height(), rgba: picture.into_raw() }
        }
        Err(_) => egui::IconData::default(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_window_gets_the_largest_icon() {
        let icon = super::window_icon();
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(icon.rgba.len(), 256 * 256 * 4);
        // Transparent corner, solid middle.
        assert_eq!(icon.rgba[3], 0);
        assert_eq!(icon.rgba[(128 * 256 + 128) * 4 + 3], 255);
    }
}
