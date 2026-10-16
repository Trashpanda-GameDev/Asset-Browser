//! Loads the large preview of the one asset the inspector shows: the picture at up to
//! `DETAIL_MAX` pixels (an animated GIF's frames instead), or the whole model for the 3D view.
//! One thread, newest request wins: when the selection moves on while something loads, only the
//! latest one is loaded next, and a long GIF gives up between frames.

use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender};
use fast_image_resize as fr;

use crate::animated::{self, GifError};
use crate::asset::AssetKind;
use crate::imaging::{self, PictureError, Pixels};
use crate::model::{self, LoadOptions, Model, ModelError};
use crate::sheets::{self, Sheet};
use crate::work::{self, Wake, WorkList};

/// Longest side of an inspector picture, and of a model's textures in the 3D view.
const DETAIL_MAX: u32 = 2048;

pub enum Detail {
    /// The picture and its frames: a sprite sheet's, or for an animated GIF all its frames laid
    /// out side by side (`animated`).
    Image(Pixels, Option<Sheet>),
    Model(Arc<DetailModel>),
}

/// A model for the 3D view, with what its upload to the GPU needs made here, off the window's
/// thread.
pub struct DetailModel {
    pub model: Model,
    /// Each texture's smaller sizes, for its mip chain (`imaging::mip_levels`).
    pub mips: Vec<Vec<(u32, u32, Vec<u8>)>>,
    /// The wireframe: every triangle edge once (`Model::edges`).
    pub edges: Vec<u32>,
}

impl DetailModel {
    fn prepare(model: Model) -> Self {
        let mips = model
            .textures
            .iter()
            .map(|texture| imaging::mip_levels(texture.width, texture.height, &texture.rgba))
            .collect();
        let edges = model.edges();
        Self { model, mips, edges }
    }
}

/// Why there's no large preview. The inspector shows the message.
#[derive(Debug, thiserror::Error)]
pub enum DetailError {
    #[error("{0}")]
    Picture(#[from] PictureError),
    #[error("{0}")]
    Gif(#[from] GifError),
    #[error("{0}")]
    Model(#[from] ModelError),
    #[error("sounds have no large preview")]
    NoneForSounds,
    #[error("the decoder crashed on this file")]
    Crashed,
}

pub struct Loaded {
    pub key: i64,
    pub result: Result<Detail, DetailError>,
}

struct Request {
    key: i64,
    path: PathBuf,
    kind: AssetKind,
}

pub struct DetailLoader {
    /// At most one request: a new one replaces the one waiting.
    requests: Arc<WorkList<Request>>,
    results: Receiver<Loaded>,
}

impl DetailLoader {
    /// `on_result` runs on the loader thread after each load, e.g. to wake the UI.
    pub fn start(on_result: Wake) -> Self {
        let requests = Arc::new(WorkList::new());
        let (sender, results) = crossbeam_channel::unbounded();
        let worker = requests.clone();
        work::spawn("detail", move || run(&worker, &sender, &*on_result));
        Self { requests, results }
    }

    /// Loads this next, replacing anything still waiting.
    pub fn request(&self, key: i64, path: PathBuf, kind: AssetKind) {
        self.requests.replace([Request { key, path, kind }]);
    }

    pub fn try_recv(&self) -> Option<Loaded> {
        self.results.try_recv().ok()
    }
}

impl Drop for DetailLoader {
    fn drop(&mut self) {
        self.requests.close();
    }
}

fn run(requests: &WorkList<Request>, results: &Sender<Loaded>, on_result: &(dyn Fn() + Send + Sync)) {
    let mut resizer = fr::Resizer::new();
    while let Some(request) = requests.take() {
        // A newer request (or closing) makes a long load give up between frames.
        let superseded = || requests.superseded();
        // Decoders can panic on malformed files; that file just fails.
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| match request.kind {
            AssetKind::Image => load_picture(&request.path, &mut resizer, &superseded),
            AssetKind::Model => model::load(&request.path, &LoadOptions { max_texture: DETAIL_MAX })
                .map(|model| Detail::Model(Arc::new(DetailModel::prepare(model))))
                .map_err(DetailError::from),
            AssetKind::Audio => Err(DetailError::NoneForSounds),
        }))
        .unwrap_or_else(|_| Err(DetailError::Crashed));
        imaging::trim_resizer(&mut resizer);
        if results.send(Loaded { key: request.key, result }).is_err() {
            return;
        }
        on_result();
    }
}

/// The picture, and its frames: an animated GIF's own, or a sprite sheet's when it looks like one.
fn load_picture(path: &Path, resizer: &mut fr::Resizer, superseded: &dyn Fn() -> bool) -> Result<Detail, DetailError> {
    if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("gif")) {
        let bytes = std::fs::read(path).map_err(PictureError::from)?;
        match animated::load_gif(&bytes, resizer, superseded) {
            Ok(Some((pixels, sheet))) => return Ok(Detail::Image(pixels, Some(sheet))),
            Err(err) if superseded() => return Err(err.into()),
            // A still GIF is a picture like any other; so is one whose frames can't all be read.
            Ok(None) | Err(_) => {}
        }
    }
    let pixels = imaging::make_image(path, resizer, DETAIL_MAX)?;
    let sheet = sheets::detect(path, &pixels);
    Ok(Detail::Image(pixels, sheet))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_stay_as_they_are_shown() {
        assert_eq!(DetailError::NoneForSounds.to_string(), "sounds have no large preview");
        assert_eq!(DetailError::Crashed.to_string(), "the decoder crashed on this file");
        // A reader's error reads as it did.
        assert_eq!(DetailError::from(PictureError::NoPixels).to_string(), "image has no pixels");
        assert_eq!(DetailError::from(GifError::Stopped).to_string(), "no longer wanted");
    }
}
