//! Makes previews on a pool of worker threads, on-screen items first: thumbnails for images and
//! models, waveforms for sounds.
//!
//! The UI hands over the full list of previews it wants each frame (`set_wanted`), visible ones
//! first. That list replaces the queue, so items scrolled past are dropped instead of being made
//! before the ones now on screen. Each preview is made once and then served from the SQLite
//! cache, whose writes go through one writer thread in batches.

mod cache;
mod worker;
mod writer;

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};

use crate::asset::AssetKind;
use crate::audio::Waveform;
use crate::imaging::Pixels;
use crate::model::ModelInfo;
use crate::storage::Part;
use crate::work::{self, Wake};
use worker::Worker;
use writer::{Store, writer_loop};

/// Longest side of a stored thumbnail, in pixels. Images no bigger than this are stored as they
/// are, so pixel art keeps its exact pixels.
pub const THUMB_MAX: u32 = 192;

/// Longest side of the textures loaded for thumbnails. A model fills at most 192 pixels, so more
/// texture detail would only be averaged away.
pub const THUMB_TEXTURE: u32 = 256;

pub struct ThumbJob {
    pub key: i64,
    pub path: PathBuf,
    pub kind: AssetKind,
}

pub enum Preview {
    Image(Pixels),
    Sound(Arc<Waveform>),
    /// A model's thumbnail and what's in the file. The picture is a render, `THUMB_MAX` square
    /// with a transparent background (`src_w` and `src_h` 0), or for a .blend the preview
    /// Blender saved (`src_w` × `src_h` its size).
    Model(Pixels, Arc<ModelInfo>),
}

pub struct Thumb {
    pub key: i64,
    pub result: Result<Preview, String>,
    pub micros: u64,
}

#[derive(Default)]
pub struct ThumbStats {
    pub made: AtomicU64,
    pub from_cache: AtomicU64,
    pub failed: AtomicU64,
    pub make_micros: AtomicU64,
}

struct Shared {
    state: Mutex<State>,
    wake_workers: Condvar,
}

#[derive(Default)]
struct State {
    queue: VecDeque<ThumbJob>,
    in_flight: HashSet<i64>,
    shutdown: bool,
}

pub struct ThumbService {
    shared: Arc<Shared>,
    results: Receiver<Thumb>,
    workers: Vec<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
    /// To the writer, for clearing the cache in turn with what it saves.
    store: Option<Sender<Store>>,
    pub stats: Arc<ThumbStats>,
}

/// One worker per core, leaving one for the UI and the OS.
pub fn default_threads() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get()).saturating_sub(1).max(1)
}

impl ThumbService {
    /// `on_result` runs on a worker thread after each thumbnail, e.g. to wake the UI.
    pub fn start(db_path: &Path, on_result: &Wake) -> Self {
        Self::start_with_threads(db_path, default_threads(), on_result)
    }

    /// Every worker reads its own file, so many reads are in flight at once. Measured on a
    /// 5,400 rpm laptop disk (`bench`, 1,000 fresh images each run, 19 workers): 1 read at a
    /// time made 43 thumbnails/s, 2 made 43/s, 4 made 72/s, all 19 made 86-95/s, and 32 made
    /// 100/s. A spinning disk serves queued requests in the order that needs the least seeking,
    /// so it does best with many queued, and it tops out near 100 random files/s however
    /// they're sent. Capping reads made it slower, so there is no cap.
    pub fn start_with_threads(db_path: &Path, threads: usize, on_result: &Wake) -> Self {
        let threads = threads.max(1);
        let shared = Arc::new(Shared { state: Mutex::default(), wake_workers: Condvar::new() });
        let stats = Arc::new(ThumbStats::default());
        let (result_tx, results) = crossbeam_channel::unbounded();
        let (store_tx, store_rx) = crossbeam_channel::unbounded();

        let writer = {
            let db_path = db_path.to_path_buf();
            work::spawn("thumb-writer", move || writer_loop(&db_path, &store_rx))
        };
        let workers = (0..threads)
            .filter_map(|index| {
                let worker = Worker {
                    shared: shared.clone(),
                    results: result_tx.clone(),
                    store: store_tx.clone(),
                    on_result: on_result.clone(),
                    stats: stats.clone(),
                };
                let db_path = db_path.to_path_buf();
                work::spawn(&format!("thumb-{index}"), move || worker.run(&db_path))
            })
            .collect();

        Self { shared, results, workers, writer, store: Some(store_tx), stats }
    }

    /// Clears parts of the cache, after what's waiting to be saved and before anything after it,
    /// then gives the space back. Answers on the channel when done (it can take seconds).
    pub fn clear(&self, parts: Vec<Part>) -> Receiver<Result<(), String>> {
        let (done_tx, done) = crossbeam_channel::bounded(1);
        let sent = self.store.as_ref().is_some_and(|store| store.send(Store::Clear(parts, done_tx.clone())).is_ok());
        if !sent {
            // Can't fail: the receiver is right here.
            let _ = done_tx.send(Err("the cache isn't open".to_owned()));
        }
        done
    }

    pub fn thread_count(&self) -> usize {
        self.workers.len()
    }

    /// Replaces the queue. Jobs already being worked on are skipped.
    pub fn set_wanted(&self, jobs: Vec<ThumbJob>) {
        let mut state = work::lock(&self.shared.state);
        state.queue.clear();
        for job in jobs {
            if !state.in_flight.contains(&job.key) {
                state.queue.push_back(job);
            }
        }
        let any = !state.queue.is_empty();
        drop(state);
        if any {
            self.shared.wake_workers.notify_all();
        }
    }

    /// (queued, being made)
    pub fn pending(&self) -> (usize, usize) {
        let state = work::lock(&self.shared.state);
        (state.queue.len(), state.in_flight.len())
    }

    pub fn try_recv(&self) -> Option<Thumb> {
        self.results.try_recv().ok()
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Option<Thumb> {
        self.results.recv_timeout(timeout).ok()
    }
}

impl Drop for ThumbService {
    fn drop(&mut self) {
        {
            let mut state = work::lock(&self.shared.state);
            state.shutdown = true;
            state.queue.clear();
        }
        self.shared.wake_workers.notify_all();
        for worker in self.workers.drain(..) {
            // A worker that panicked was logged by the panic hook.
            let _ = worker.join();
        }
        // The writer stops once every sender is gone (the workers' and this one), after committing
        // what's left.
        self.store = None;
        if let Some(writer) = self.writer.take() {
            // A writer that panicked was logged by the panic hook.
            let _ = writer.join();
        }
    }
}
