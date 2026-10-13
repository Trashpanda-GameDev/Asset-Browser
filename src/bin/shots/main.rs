//! Screenshots of the window, to check the look by eye: the main window, each settings page and
//! "What's new", then the main window again at the end, saved as PNGs. It's the app itself,
//! opening the library as usual, and it closes once it's done (about 18 s). A folder given after
//! the first is added to the library and shown, as on the app's command line; what changes there
//! during the run shows in the last picture.
//!
//! With `--demo`, the README's pictures instead (into `docs/images`): the app on a library of
//! made-up assets (`demo.rs`), written to the folder given after (an empty one, as its path shows
//! in the pictures) or else to `target/demo`. The app keeps its cache and settings in
//! `target/demo` too, so the user's own library is left alone.
//!
//! ```text
//! cargo run --release --bin shots -- <folder to save them in> [<folder to show>]
//! cargo run --release --bin shots -- --demo docs/images [<empty folder for the made-up assets>]
//! ```

mod demo;

use std::path::{Path, PathBuf};
use std::time::Instant;

use asset_browser::app::{BrowserApp, ScreenshotView};
use asset_browser::db;
use asset_browser::library::SortBy;
use asset_browser::ui::mode::{Layout, Mode};
use asset_browser::ui::settings::Page;
use eframe::egui;

enum Step {
    Shot(&'static str),
    Settings(Option<Page>),
    WhatsNew(Option<bool>),
    View(ScreenshotView),
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

/// The README's pictures, of the made-up library.
const DEMO: [(f32, Step); 16] = [
    // By name, so every kind of asset is in the first rows.
    (3.0, Step::View(ScreenshotView { sort: Some(SortBy::Name), file: Some("Models/crystal.obj"), ..grid(Mode::All) })),
    (5.5, Step::Shot("overview")),
    (5.7, Step::View(ScreenshotView { file: Some("Backgrounds/forest_day.png"), ..grid(Mode::Images) })),
    (7.0, Step::Shot("images")),
    (7.2, Step::View(ScreenshotView { file: Some("Characters/slime_bounce.png"), ..grid(Mode::Images) })),
    (8.9, Step::Shot("sprite-sheet")),
    (
        9.1,
        Step::View(ScreenshotView {
            file: Some("Music/menu_theme.wav"),
            ..ScreenshotView::new(Mode::Sounds, Layout::List)
        }),
    ),
    (10.5, Step::Shot("sounds")),
    // Bigger tiles: there are only eight models.
    (10.7, Step::View(ScreenshotView { tile: Some(200.0), file: Some("Models/house.obj"), ..grid(Mode::Models) })),
    (13.0, Step::Shot("models")),
    (13.2, Step::View(ScreenshotView { group: Some("Pickups"), file: Some("Items/coin.png"), ..grid(Mode::All) })),
    (14.5, Step::Shot("organize")),
    (14.7, Step::Settings(Some(Page::Keyboard))),
    (15.7, Step::Shot("settings")),
    (15.9, Step::Settings(None)),
    (16.3, Step::Quit),
];

const fn grid(mode: Mode) -> ScreenshotView {
    ScreenshotView::new(mode, Layout::Grid)
}

struct Shots {
    app: BrowserApp,
    plan: &'static [(f32, Step)],
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
        while let Some((at, step)) = self.plan.get(self.next)
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
                Step::View(view) => self.app.show_view(*view),
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

/// As a PNG without alpha, compressed as far as it goes: they're kept in the repository.
fn save(path: &Path, image: &egui::ColorImage) {
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    let rgb: Vec<u8> = image.pixels.iter().flat_map(|color| [color.r(), color.g(), color.b()]).collect();
    let [w, h] = image.size;
    let written = std::fs::File::create(path).map_err(image::ImageError::IoError).and_then(|file| {
        let encoder =
            PngEncoder::new_with_quality(std::io::BufWriter::new(file), CompressionType::Best, FilterType::Adaptive);
        image::ImageEncoder::write_image(encoder, &rgb, w as u32, h as u32, image::ExtendedColorType::Rgb8)
    });
    match written {
        Ok(()) => println!("saved {}", path.display()),
        Err(err) => eprintln!("couldn't save {}: {err}", path.display()),
    }
}

fn main() -> eframe::Result {
    let mut args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let demo = args.first().is_some_and(|arg| arg.as_os_str() == "--demo");
    if demo {
        args.remove(0);
    }
    let out = args.first().cloned().unwrap_or_else(|| PathBuf::from("."));
    let _ = std::fs::create_dir_all(&out);
    let (show, plan, size): (_, &'static [(f32, Step)], _) = if demo {
        // The app's own files go in the build folder, made afresh each time.
        let dir = std::path::absolute("target/demo").unwrap_or_else(|_| PathBuf::from("target/demo"));
        let _ = std::fs::remove_dir_all(&dir);
        // SAFETY: no other thread has started yet, so nothing reads the environment meanwhile.
        unsafe { std::env::set_var(db::HOME_VARIABLE, dir.join("home")) };
        // The preview pane shows the files' paths, so a short folder reads best (F:\Assets, say);
        // else one beside the app's files. Not the temporary folder: its path has the user's name.
        let library = match args.get(1) {
            Some(folder) => std::path::absolute(folder).unwrap_or_else(|_| folder.clone()),
            None => dir.join("Sample Game Assets"),
        };
        if let Err(err) = demo::write(&library) {
            eprintln!("couldn't write the made-up library: {err}");
            std::process::exit(1);
        }
        demo::organize(&library);
        // Tall enough for a sprite sheet's controls, below its details in the preview pane.
        (Some(library), &DEMO, [1280.0, 1024.0])
    } else {
        (args.get(1).cloned(), &PLAN, [1440.0, 880.0])
    };
    let options = eframe::NativeOptions {
        // Not focused when it opens, so keys meant for another window don't land in this one.
        viewport: egui::ViewportBuilder::default()
            .with_title("Asset Browser (screenshots)")
            .with_inner_size(size)
            .with_active(false),
        ..Default::default()
    };
    eframe::run_native(
        "Asset Browser screenshots",
        options,
        Box::new(move |cc| {
            Ok(Box::new(Shots { app: BrowserApp::new(cc, show), plan, out, start: Instant::now(), next: 0 }))
        }),
    )
}
