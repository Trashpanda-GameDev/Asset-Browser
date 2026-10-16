//! The previews the window has: thumbnails on the GPU, waveforms, model facts, and failures.
//!
//! Every frame the views say which assets they show, most important first (`want`); at the end of
//! the frame that list replaces the workers' queue (`submit`), so scrolled-past work is dropped.

use std::cell::Cell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use eframe::egui::{self, Color32, ColorImage, TextureFilter, TextureHandle, TextureOptions};

use crate::asset::AssetKind;
use crate::audio::Waveform;
use crate::library::{Entry, Library};
use crate::model::ModelInfo;
use crate::thumbs::{Preview, Thumb, ThumbJob, ThumbPixels, ThumbService};

/// Thumbnails kept on the GPU, until the memory setting says otherwise (`set_max_textures`). The
/// least recently drawn go first; scrolling back reloads them from the cache in about a
/// millisecond each.
const MAX_TEXTURES: usize = 1500;
/// Limits GPU uploads per frame so a burst of finished thumbnails can't stall a frame.
const UPLOADS_PER_FRAME: usize = 48;

pub struct CachedTexture {
    pub handle: TextureHandle,
    /// The original picture's size (for models, the rendered size).
    pub src_size: [u32; 2],
    /// Stored at its original size (pixel art, icons): drawn with sharp, whole pixels.
    pub unscaled: bool,
    last_used: Cell<u64>,
}

pub struct Previews {
    textures: HashMap<i64, CachedTexture>,
    pub waveforms: HashMap<i64, Arc<Waveform>>,
    pub models: HashMap<i64, Arc<ModelInfo>>,
    pub failed: HashMap<i64, String>,
    /// Finished previews waiting to be taken in (thumbnails wait for their GPU upload).
    arrived: VecDeque<Thumb>,
    arrived_keys: HashSet<i64>,
    last_wanted: Vec<i64>,
    wanted: Vec<u32>,
    wanted_set: HashSet<u32>,
    /// Drawn behind pictures with transparency.
    pub checker: TextureHandle,
    frame: u64,
    max_textures: usize,
}

impl Previews {
    pub fn new(ctx: &egui::Context) -> Self {
        let (light, dark) = (Color32::from_gray(46), Color32::from_gray(36));
        let checker = ctx.load_texture(
            "checker",
            ColorImage::new([2, 2], vec![light, dark, dark, light]),
            TextureOptions::NEAREST_REPEAT,
        );
        Self {
            textures: HashMap::new(),
            waveforms: HashMap::new(),
            models: HashMap::new(),
            failed: HashMap::new(),
            arrived: VecDeque::new(),
            arrived_keys: HashSet::new(),
            last_wanted: Vec::new(),
            wanted: Vec::new(),
            wanted_set: HashSet::new(),
            checker,
            frame: 0,
            max_textures: MAX_TEXTURES,
        }
    }

    /// How many thumbnails may stay on the GPU; any over go at the end of the frame.
    pub fn set_max_textures(&mut self, count: usize) {
        self.max_textures = count.max(100);
    }

    /// Forgets what's held of the cache's parts that were cleared, so they're made again as
    /// they're shown: thumbnails (and model pictures), waveforms, model facts.
    #[expect(clippy::fn_params_excessive_bools, reason = "refactor: an enum instead of bool flags")]
    pub fn forget(&mut self, thumbnails: bool, waveforms: bool, models: bool) {
        if thumbnails {
            self.textures.clear();
        }
        if waveforms {
            self.waveforms.clear();
        }
        if models {
            // A model without its facts counts as not previewed, so its picture is made again too.
            self.models.clear();
        }
        // Failures were cached with them; they're tried again too.
        self.failed.clear();
        self.arrived.clear();
        self.arrived_keys.clear();
        self.last_wanted.clear();
    }

    pub fn begin_frame(&mut self) {
        self.frame += 1;
        self.forget_wanted();
    }

    /// The library was rebuilt: what this frame asked for is entry indices into the old one.
    /// (The previews themselves are kept by cache key, so they stay good.)
    pub fn forget_wanted(&mut self) {
        self.wanted.clear();
        self.wanted_set.clear();
    }

    /// Takes in finished previews, uploading a limited number of thumbnails per frame.
    pub fn drain(&mut self, thumbs: &ThumbService, ctx: &egui::Context) {
        while let Some(thumb) = thumbs.try_recv() {
            self.arrived_keys.insert(thumb.key);
            self.arrived.push_back(thumb);
        }
        let mut uploads = 0;
        while uploads < UPLOADS_PER_FRAME {
            let Some(thumb) = self.arrived.pop_front() else { break };
            self.arrived_keys.remove(&thumb.key);
            match thumb.result {
                Ok(Preview::Image(pixels)) => {
                    uploads += 1;
                    let unscaled = pixels.is_unscaled();
                    self.upload(ctx, thumb.key, &pixels, unscaled);
                }
                Ok(Preview::Model(pixels, info)) => {
                    uploads += 1;
                    self.upload(ctx, thumb.key, &pixels, false);
                    self.models.insert(thumb.key, info);
                }
                Ok(Preview::Sound(waveform)) => {
                    self.waveforms.insert(thumb.key, waveform);
                }
                Err(message) => {
                    self.failed.insert(thumb.key, message);
                }
            }
        }
        if !self.arrived.is_empty() {
            ctx.request_repaint();
        }
    }

    fn upload(&mut self, ctx: &egui::Context, key: i64, pixels: &ThumbPixels, unscaled: bool) {
        let options = if unscaled {
            let mut options = TextureOptions::NEAREST;
            options.minification = TextureFilter::Linear;
            options
        } else {
            TextureOptions::LINEAR.with_mipmap_mode(Some(TextureFilter::Linear))
        };
        let image = ColorImage::from_rgba_unmultiplied([pixels.w as usize, pixels.h as usize], &pixels.rgba);
        let handle = ctx.load_texture(format!("thumb-{key}"), image, options);
        let texture = CachedTexture {
            handle,
            src_size: [pixels.src_w, pixels.src_h],
            unscaled,
            last_used: Cell::new(self.frame),
        };
        self.textures.insert(key, texture);
    }

    /// The thumbnail for `key`, marked as drawn this frame.
    pub fn texture(&self, key: i64) -> Option<&CachedTexture> {
        let texture = self.textures.get(&key)?;
        texture.last_used.set(self.frame);
        Some(texture)
    }

    /// The original picture's size, once its thumbnail has been made.
    pub fn dimensions(&self, key: i64) -> Option<[u32; 2]> {
        self.textures.get(&key).map(|texture| texture.src_size)
    }

    fn has_preview(&self, entry: &Entry) -> bool {
        match entry.kind {
            AssetKind::Image => self.textures.contains_key(&entry.key),
            AssetKind::Audio => self.waveforms.contains_key(&entry.key),
            AssetKind::Model => self.textures.contains_key(&entry.key) && self.models.contains_key(&entry.key),
        }
    }

    /// Asks for `index`'s preview this frame. Earlier asks are made first.
    pub fn want(&mut self, index: u32) {
        if self.wanted_set.insert(index) {
            self.wanted.push(index);
        }
    }

    /// Hands this frame's list to the workers, unless it's the same as last frame's.
    pub fn submit(&mut self, thumbs: &ThumbService, library: &Library) {
        let mut jobs_for = Vec::with_capacity(self.wanted.len());
        for &index in &self.wanted {
            let Some(entry) = library.entries.get(index as usize) else { continue };
            if !self.has_preview(entry)
                && !self.failed.contains_key(&entry.key)
                && !self.arrived_keys.contains(&entry.key)
            {
                jobs_for.push(entry);
            }
        }
        let keys: Vec<i64> = jobs_for.iter().map(|entry| entry.key).collect();
        if keys == self.last_wanted {
            return;
        }
        let jobs = jobs_for
            .iter()
            .map(|entry| ThumbJob { key: entry.key, path: library.path_of(entry), kind: entry.kind })
            .collect();
        thumbs.set_wanted(jobs);
        self.last_wanted = keys;
    }

    pub fn evict(&mut self) {
        let max = self.max_textures;
        if self.textures.len() <= max {
            return;
        }
        let mut by_age: Vec<(u64, i64)> =
            self.textures.iter().map(|(&key, texture)| (texture.last_used.get(), key)).collect();
        by_age.sort_unstable();
        let excess = self.textures.len() - max + max / 10;
        for (last_used, key) in by_age.into_iter().take(excess) {
            if last_used == self.frame {
                break;
            }
            self.textures.remove(&key);
        }
    }
}
