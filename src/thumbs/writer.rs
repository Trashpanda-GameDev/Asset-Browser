//! The cache writer: one thread saving the workers' previews in batches, and clearing parts of
//! the cache in turn with them.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};

use super::ClearError;
use super::cache::{CacheWrite, put_cache};
use crate::storage::{self, Part};
use crate::{db, diag};

/// What the cache writer is given: a preview to save, or parts of the cache to clear (answered on
/// the channel when done).
pub(super) enum Store {
    Write(CacheWrite),
    Clear(Vec<Part>, Sender<Result<(), ClearError>>),
}

pub(super) fn writer_loop(db_path: &Path, jobs: &Receiver<Store>) {
    let conn = match db::open_cache(db_path) {
        Ok(conn) => conn,
        Err(err) => {
            let err = Arc::new(err);
            diag::report("Thumbnail cache writer", &err);
            while let Ok(job) = jobs.recv() {
                if let Store::Clear(_, done) = job {
                    // Fails only when nobody's waiting for the answer any more.
                    let _ = done.send(Err(ClearError::Unopened(err.clone())));
                }
            }
            return;
        }
    };
    while let Ok(first) = jobs.recv() {
        // Saves are batched into one transaction; a clear ends the batch and goes after it.
        let mut batch = Vec::new();
        let mut clear = None;
        match first {
            Store::Write(write) => batch.push(write),
            Store::Clear(parts, done) => clear = Some((parts, done)),
        }
        let deadline = Instant::now() + Duration::from_millis(100);
        while clear.is_none() && batch.len() < 512 {
            match jobs.recv_deadline(deadline) {
                Ok(Store::Write(write)) => batch.push(write),
                Ok(Store::Clear(parts, done)) => clear = Some((parts, done)),
                Err(_) => break,
            }
        }
        if !batch.is_empty()
            && let Err(err) = put_cache(&conn, &batch)
        {
            diag::report("Saving previews", err);
        }
        if let Some((parts, done)) = clear {
            // Fails only when nobody's waiting for the answer any more.
            let _ = done.send(storage::clear_cache(&conn, &parts).map_err(ClearError::from));
        }
    }
}
