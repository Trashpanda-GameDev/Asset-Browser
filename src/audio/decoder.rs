//! The decoder thread: takes the player's commands, decodes the sound and keeps the buffer
//! filled `BUFFER_SECONDS` ahead of the playhead.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, TryRecvError};

use super::resample::Resampler;
use super::stream::SoundStream;
use super::{Command, Info, Shared};
use crate::{diag, work};

#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "refactor: take fewer, grouped arguments; split into named steps"
)]
pub(super) fn decoder_loop(
    shared: &Shared,
    mut buffer: rtrb::Producer<f32>,
    commands: &Receiver<Command>,
    rate: u32,
    channels: usize,
    on_change: &(dyn Fn() + Send + Sync),
) {
    let mut current: Option<(SoundStream, Resampler)> = None;
    let mut ended = false;
    let mut pending: Vec<f32> = Vec::new();
    let mut pending_at = 0;
    let mut awaiting_first: Option<Instant> = None;
    loop {
        // Sleep until told something when there's nothing to decode; while the buffer is full,
        // wake every few ms to top it up; otherwise only check for commands in passing.
        let command = if current.is_none() || ended {
            match commands.recv() {
                Ok(command) => Some(command),
                Err(_) => return,
            }
        } else if pending_at < pending.len() && buffer.slots() == 0 {
            match commands.recv_timeout(Duration::from_millis(5)) {
                Ok(command) => Some(command),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        } else {
            match commands.try_recv() {
                Ok(command) => Some(command),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => return,
            }
        };

        if let Some(command) = command {
            flush(shared, &buffer);
            pending.clear();
            pending_at = 0;
            ended = false;
            shared.decoder_done.store(false, Ordering::Release);
            shared.finished.store(false, Ordering::Release);
            match command {
                Command::Play { key, source, extension, requested } => {
                    shared.paused.store(false, Ordering::Relaxed);
                    let mut info = Info { key: Some(key), ..Info::default() };
                    match SoundStream::open(source, &extension) {
                        Ok(stream) => {
                            info.duration = stream.duration;
                            current = Some((stream, Resampler::default()));
                            awaiting_first = Some(requested);
                        }
                        Err(err) => {
                            // Shown under the player; logged too, like every error the window shows.
                            diag::note("Playing a sound", &err);
                            info.error = Some(err);
                            current = None;
                            shared.finished.store(true, Ordering::Release);
                        }
                    }
                    shared.played_frames.store(0, Ordering::Relaxed);
                    *work::lock(&shared.info) = info;
                }
                Command::Seek(seconds) => {
                    if let Some((stream, resampler)) = &mut current {
                        let result = stream.seek(seconds);
                        *resampler = Resampler::default();
                        let mut info = work::lock(&shared.info);
                        match result {
                            Ok(at) => info.start = at,
                            Err(err) => {
                                diag::note("Seeking in a sound", &err);
                                info.error = Some(err);
                            }
                        }
                        shared.played_frames.store(0, Ordering::Relaxed);
                    }
                }
                Command::Stop => {
                    current = None;
                    shared.played_frames.store(0, Ordering::Relaxed);
                    *work::lock(&shared.info) = Info::default();
                }
            }
            on_change();
            continue;
        }

        let Some((stream, resampler)) = current.as_mut() else { continue };
        if pending_at < pending.len() {
            let (pushed, _) = buffer.push_partial_slice(&pending[pending_at..]);
            pending_at += pushed.len();
            if !pushed.is_empty()
                && let Some(requested) = awaiting_first.take()
            {
                work::lock(&shared.info).ready_ms = Some(requested.elapsed().as_secs_f32() * 1000.0);
                on_change();
            }
            continue;
        }
        match stream.next() {
            Ok(Some((source_rate, source_channels))) => {
                resampler.process(&stream.samples, source_channels, source_rate, channels, rate, &mut pending);
                pending_at = 0;
            }
            Ok(None) if shared.looping.load(Ordering::Relaxed) && stream.seek(0.0).is_ok() => {
                // Carry on from the start without flushing, so the loop has no gap.
                *resampler = Resampler::default();
            }
            Ok(None) => {
                ended = true;
                shared.decoder_done.store(true, Ordering::Release);
            }
            Err(err) => {
                diag::note("Playing a sound", &err);
                work::lock(&shared.info).error = Some(err);
                ended = true;
                shared.decoder_done.store(true, Ordering::Release);
                on_change();
            }
        }
    }
}

/// Has the callback drop everything buffered, and waits until it has. The callback runs every
/// few milliseconds; if the device has stopped, give up rather than hang.
fn flush(shared: &Shared, buffer: &rtrb::Producer<f32>) {
    if buffer.slots() == buffer.buffer().capacity() {
        return; // nothing buffered
    }
    shared.flush.store(true, Ordering::Release);
    let start = Instant::now();
    while shared.flush.load(Ordering::Acquire) {
        let waited = start.elapsed();
        if waited > Duration::from_millis(100) {
            shared.flush.store(false, Ordering::Release);
            break;
        }
        if waited < Duration::from_millis(20) {
            std::thread::yield_now();
        } else {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
