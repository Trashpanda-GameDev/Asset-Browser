//! Loads the large preview of the one asset the inspector shows: the picture at up to
//! `DETAIL_MAX` pixels, or the whole model for the 3D view. One thread, newest request wins: when
//! the selection moves on while something loads, only the latest one is loaded next.

use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};

use crossbeam_channel::{Receiver, Sender};
use fast_image_resize as fr;

use crate::asset::AssetKind;
use crate::model::{self, LoadOptions, Model};
use crate::sheets::{self, Sheet};
use crate::thumbs::{self, ThumbPixels};

/// Longest side of an inspector picture, and of a model's textures in the 3D view.
pub const DETAIL_MAX: u32 = 2048;

pub enum Detail {
    /// The picture, and its frames if it looks like a sprite sheet.
    Image(ThumbPixels, Option<Sheet>),
    Model(Arc<Model>),
}

pub struct Loaded {
    pub key: i64,
    pub result: Result<Detail, String>,
}

struct Request {
    key: i64,
    path: PathBuf,
    kind: AssetKind,
}

#[derive(Default)]
struct Slot {
    next: Option<Request>,
    shutdown: bool,
}

pub struct DetailLoader {
    slot: Arc<(Mutex<Slot>, Condvar)>,
    results: Receiver<Loaded>,
}

impl DetailLoader {
    /// `on_result` runs on the loader thread after each load, e.g. to wake the UI.
    pub fn start(on_result: Arc<dyn Fn() + Send + Sync>) -> Self {
        let slot = Arc::new((Mutex::new(Slot::default()), Condvar::new()));
        let (sender, results) = crossbeam_channel::unbounded();
        let worker = slot.clone();
        let spawned =
            std::thread::Builder::new().name("detail".into()).spawn(move || run(&worker, &sender, &*on_result));
        if let Err(err) = spawned {
            crate::diag::report("Starting the preview loader", err);
        }
        Self { slot, results }
    }

    /// Loads this next, replacing anything still waiting.
    pub fn request(&self, key: i64, path: PathBuf, kind: AssetKind) {
        let (slot, wake) = &*self.slot;
        slot.lock().unwrap().next = Some(Request { key, path, kind });
        wake.notify_one();
    }

    pub fn try_recv(&self) -> Option<Loaded> {
        self.results.try_recv().ok()
    }
}

impl Drop for DetailLoader {
    fn drop(&mut self) {
        let (slot, wake) = &*self.slot;
        slot.lock().unwrap().shutdown = true;
        wake.notify_one();
    }
}

fn run(slot: &(Mutex<Slot>, Condvar), results: &Sender<Loaded>, on_result: &(dyn Fn() + Send + Sync)) {
    let mut resizer = fr::Resizer::new();
    loop {
        let request = {
            let (lock, wake) = slot;
            let mut slot = lock.lock().unwrap();
            loop {
                if slot.shutdown {
                    return;
                }
                if let Some(request) = slot.next.take() {
                    break request;
                }
                slot = wake.wait(slot).unwrap();
            }
        };
        // Decoders can panic on malformed files; that file just fails.
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| match request.kind {
            AssetKind::Image => thumbs::make_image(&request.path, &mut resizer, DETAIL_MAX).map(|pixels| {
                let view = sheets::Pixels {
                    rgba: &pixels.rgba,
                    w: pixels.w,
                    h: pixels.h,
                    src_w: pixels.src_w,
                    src_h: pixels.src_h,
                };
                let sheet = sheets::detect(&request.path, &view);
                Detail::Image(pixels, sheet)
            }),
            AssetKind::Model => model::load(&request.path, &LoadOptions { max_texture: DETAIL_MAX })
                .map(|model| Detail::Model(Arc::new(model))),
            AssetKind::Audio => Err("sounds have no large preview".to_owned()),
        }))
        .unwrap_or_else(|_| Err("the decoder crashed on this file".into()));
        if results.send(Loaded { key: request.key, result }).is_err() {
            return;
        }
        on_result();
    }
}
