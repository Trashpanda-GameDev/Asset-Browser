//! Screenshots of the window, to check the look by eye: the main window, each settings page and
//! "What's new", then the main window again at the end, saved as PNGs. It's the app itself,
//! opening the library as usual, and it closes once it's done (about 18 s). A folder given after
//! the first is added to the library and shown, as on the app's command line; what changes there
//! during the run shows in the last picture.
//!
//! ```text
//! cargo run --release --bin shots -- <folder to save them in> [<folder to show>]
//! ```

use std::path::{Path, PathBuf};
use std::time::Instant;

use asset_browser::app::BrowserApp;
use asset_browser::ui::settings::Page;
use eframe::egui;

enum Step {
    Shot(&'static str),
    Settings(Option<Page>),
    WhatsNew(Option<bool>),
    Quit,
}

/// When (seconds after the start) and what.
const PLAN: [(f32, Step); 17] = [
    (5.0, Step::Shot("main")),
    (5.5, Step::Settings(Some(Page::Updates))),
    (6.5, Step::Shot("settings-updates")),
    (7.0, Step::Settings(Some(Page::Keyboard))),
    (8.0, Step::Shot("settings-keyboard")),
    (8.5, Step::Settings(Some(Page::Storage))),
    (11.5, Step::Shot("settings-storage")),
    (12.0, Step::Settings(Some(Page::Memory))),
    (13.0, Step::Shot("settings-memory")),
    (13.5, Step::Settings(None)),
    (13.6, Step::WhatsNew(Some(false))),
    (14.5, Step::Shot("whats-new")),
    (15.0, Step::WhatsNew(Some(true))),
    (16.0, Step::Shot("every-version")),
    (16.5, Step::WhatsNew(None)),
    (17.5, Step::Shot("main-end")),
    (18.0, Step::Quit),
];

struct Shots {
    app: BrowserApp,
    out: PathBuf,
    start: Instant,
    next: usize,
}

impl eframe::App for Shots {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.app.ui(ui, frame);
        let ctx = ui.ctx().clone();
        let taken: Vec<(String, std::sync::Arc<egui::ColorImage>)> = ctx.input(|input| {
            input
                .raw
                .events
                .iter()
                .filter_map(|event| match event {
                    egui::Event::Screenshot { user_data, image, .. } => {
                        let name = user_data.data.as_ref()?.downcast_ref::<String>()?.clone();
                        Some((name, image.clone()))
                    }
                    _ => None,
                })
                .collect()
        });
        for (name, image) in taken {
            save(&self.out.join(format!("{name}.png")), &image);
        }
        let elapsed = self.start.elapsed().as_secs_f32();
        while let Some((at, step)) = PLAN.get(self.next)
            && *at <= elapsed
        {
            match step {
                Step::Shot(name) => {
                    let user_data = egui::UserData::new((*name).to_owned());
                    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(user_data));
                }
                Step::Settings(page) => self.app.show_settings(*page),
                Step::WhatsNew(Some(every_version)) => self.app.show_whats_new(*every_version),
                Step::WhatsNew(None) => self.app.hide_whats_new(),
                Step::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            }
            self.next += 1;
        }
        ctx.request_repaint();
    }

    fn on_exit(&mut self) {
        self.app.on_exit();
    }
}

fn save(path: &Path, image: &egui::ColorImage) {
    let rgba: Vec<u8> = image.pixels.iter().flat_map(|color| color.to_array()).collect();
    let [w, h] = image.size;
    match image::save_buffer(path, &rgba, w as u32, h as u32, image::ExtendedColorType::Rgba8) {
        Ok(()) => println!("saved {}", path.display()),
        Err(err) => eprintln!("couldn't save {}: {err}", path.display()),
    }
}

fn main() -> eframe::Result {
    let out = std::env::args_os().nth(1).map_or_else(|| PathBuf::from("."), PathBuf::from);
    let show = std::env::args_os().nth(2).map(PathBuf::from);
    let _ = std::fs::create_dir_all(&out);
    let options = eframe::NativeOptions {
        // Not focused when it opens, so keys meant for another window don't land in this one.
        viewport: egui::ViewportBuilder::default()
            .with_title("Asset Browser (screenshots)")
            .with_inner_size([1440.0, 880.0])
            .with_active(false),
        ..Default::default()
    };
    eframe::run_native(
        "Asset Browser screenshots",
        options,
        Box::new(move |cc| Ok(Box::new(Shots { app: BrowserApp::new(cc, show), out, start: Instant::now(), next: 0 }))),
    )
}
