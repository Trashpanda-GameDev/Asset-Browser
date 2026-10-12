// Release builds open no console window on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use asset_browser::app::BrowserApp;
use eframe::egui;

fn main() -> eframe::Result {
    asset_browser::diag::install_panic_hook();
    // An optional folder to open; otherwise the last one opened.
    let start_root = std::env::args_os().nth(1).map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Asset Browser")
            .with_inner_size([1440.0, 880.0])
            .with_min_inner_size([760.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native("Asset Browser", options, Box::new(move |cc| Ok(Box::new(BrowserApp::new(cc, start_root)))))
}
