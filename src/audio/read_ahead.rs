//! Reads the sounds next to the selection into memory ahead of time, within a memory budget.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::work::{self, WorkList};

/// Read-ahead: files bigger than this stream from disk instead (long music, mostly).
const READ_AHEAD_MAX_FILE: u64 = 24 * 1024 * 1024;
/// What the sounds read ahead may hold in all; the memory setting changes it
/// (`set_read_ahead_budget`).
static READ_AHEAD_BUDGET: AtomicU64 = AtomicU64::new(160 * 1024 * 1024);

/// How much the sounds read ahead may hold, from the next one read.
pub fn set_read_ahead_budget(bytes: u64) {
    READ_AHEAD_BUDGET.store(bytes, Ordering::Relaxed);
}

/// Reads the sounds next to the selection into memory, so moving to one starts it without
/// waiting on the disk.
pub struct ReadAhead {
    shared: Arc<ReadAheadShared>,
}

#[derive(Default)]
struct ReadAheadShared {
    wanted: WorkList<(i64, PathBuf)>,
    cache: Mutex<ByteCache>,
}

#[derive(Default)]
struct ByteCache {
    files: HashMap<i64, Arc<[u8]>>,
    order: VecDeque<i64>,
    bytes: usize,
}

impl ReadAhead {
    pub fn start() -> Self {
        let shared = Arc::new(ReadAheadShared::default());
        let worker = shared.clone();
        work::spawn("read-ahead", move || read_ahead_loop(&worker));
        Self { shared }
    }

    /// Replaces the list of files to read: (key, path, size). Big files are skipped.
    pub fn want(&self, files: Vec<(i64, PathBuf, u64)>) {
        self.shared.wanted.replace(
            files.into_iter().filter(|(_, _, size)| *size <= READ_AHEAD_MAX_FILE).map(|(key, path, _)| (key, path)),
        );
    }

    pub fn get(&self, key: i64) -> Option<Arc<[u8]>> {
        work::lock(&self.shared.cache).files.get(&key).cloned()
    }
}

impl Drop for ReadAhead {
    fn drop(&mut self) {
        self.shared.wanted.close();
    }
}

fn read_ahead_loop(shared: &ReadAheadShared) {
    while let Some((key, path)) = shared.wanted.take() {
        if work::lock(&shared.cache).files.contains_key(&key) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else { continue };
        let mut cache = work::lock(&shared.cache);
        cache.bytes += bytes.len();
        cache.files.insert(key, bytes.into());
        cache.order.push_back(key);
        while cache.bytes as u64 > READ_AHEAD_BUDGET.load(Ordering::Relaxed) {
            let Some(oldest) = cache.order.pop_front() else { break };
            if let Some(evicted) = cache.files.remove(&oldest) {
                cache.bytes -= evicted.len();
            }
        }
    }
}
