//! The decoder thread: takes the player's commands, decodes the sound and keeps the buffer
//! filled `BUFFER_SECONDS` ahead of the playhead.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, TryRecvError};

use super::resample::Resampler;
use super::stream::SoundStream;
use super::{Command, Format, Info, Shared, Source};
use crate::{diag, work};

/// Runs the decoder thread until the player is dropped.
pub(super) fn decoder_loop(
    shared: &Shared,
    buffer: rtrb::Producer<f32>,
    commands: &Receiver<Command>,
    device: Format,
    on_change: &(dyn Fn() + Send + Sync),
) {
    Decoder::new(shared, buffer, device, on_change).run(commands);
}

/// What the decoder thread holds between packets.
struct Decoder<'a> {
    shared: &'a Shared,
    buffer: rtrb::Producer<f32>,
    /// What everything is converted to: the device's rate and channels.
    device: Format,
    /// Runs when what `Player::now` reports changes other than by the playhead moving.
    on_change: &'a (dyn Fn() + Send + Sync),
    /// The sound being decoded, and its conversion to the device.
    current: Option<(SoundStream, Resampler)>,
    /// Decoded to its end (and not looping): nothing to do until the next command.
    ended: bool,
    /// Converted samples that didn't fit in the buffer yet, from `pending_at` on.
    pending: Vec<f32>,
    pending_at: usize,
    /// When the sound was asked for, until its first audio is in the buffer.
    awaiting_first: Option<Instant>,
}

/// What the decoder does next.
enum Next {
    Command(Command),
    /// No command: carry on with the sound.
    Nothing,
    /// The player was dropped.
    PlayerGone,
}

impl<'a> Decoder<'a> {
    fn new(
        shared: &'a Shared,
        buffer: rtrb::Producer<f32>,
        device: Format,
        on_change: &'a (dyn Fn() + Send + Sync),
    ) -> Self {
        Self {
            shared,
            buffer,
            device,
            on_change,
            current: None,
            ended: false,
            pending: Vec::new(),
            pending_at: 0,
            awaiting_first: None,
        }
    }

    fn run(mut self, commands: &Receiver<Command>) {
        loop {
            match self.next(commands) {
                Next::Command(command) => self.obey(command),
                Next::Nothing if self.has_pending() => self.push_pending(),
                Next::Nothing => self.decode_packet(),
                Next::PlayerGone => return,
            }
        }
    }

    /// Sleeps until told something when there's nothing to decode; while the buffer is full,
    /// wakes every few ms to top it up; otherwise only checks for a command in passing.
    fn next(&self, commands: &Receiver<Command>) -> Next {
        if self.current.is_none() || self.ended {
            commands.recv().map_or(Next::PlayerGone, Next::Command)
        } else if self.has_pending() && self.buffer.slots() == 0 {
            match commands.recv_timeout(Duration::from_millis(5)) {
                Ok(command) => Next::Command(command),
                Err(RecvTimeoutError::Timeout) => Next::Nothing,
                Err(RecvTimeoutError::Disconnected) => Next::PlayerGone,
            }
        } else {
            match commands.try_recv() {
                Ok(command) => Next::Command(command),
                Err(TryRecvError::Empty) => Next::Nothing,
                Err(TryRecvError::Disconnected) => Next::PlayerGone,
            }
        }
    }

    /// Drops what's buffered, then does what the player asked.
    fn obey(&mut self, command: Command) {
        flush(self.shared, &self.buffer);
        self.pending.clear();
        self.pending_at = 0;
        self.ended = false;
        self.shared.decoder_done.store(false, Ordering::Release);
        self.shared.finished.store(false, Ordering::Release);
        match command {
            Command::Play { key, source, extension, requested } => self.play(key, source, &extension, requested),
            Command::Seek(seconds) => self.seek(seconds),
            Command::Stop => self.stop(),
        }
        (self.on_change)();
    }

    fn play(&mut self, key: i64, source: Source, extension: &str, requested: Instant) {
        self.shared.paused.store(false, Ordering::Relaxed);
        let mut info = Info { key: Some(key), ..Info::default() };
        match SoundStream::open(source, extension) {
            Ok(stream) => {
                info.duration = stream.duration;
                self.current = Some((stream, Resampler::default()));
                self.awaiting_first = Some(requested);
            }
            Err(err) => {
                // Shown under the player; logged too, like every error the window shows.
                diag::note("Playing a sound", &err);
                info.error = Some(err.to_string());
                self.current = None;
                self.shared.finished.store(true, Ordering::Release);
            }
        }
        self.shared.played_frames.store(0, Ordering::Relaxed);
        *work::lock(&self.shared.info) = info;
    }

    fn seek(&mut self, seconds: f64) {
        let Some((stream, resampler)) = &mut self.current else { return };
        let result = stream.seek(seconds);
        *resampler = Resampler::default();
        let mut info = work::lock(&self.shared.info);
        match result {
            Ok(at) => info.start = at,
            Err(err) => {
                diag::note("Seeking in a sound", &err);
                info.error = Some(err.to_string());
            }
        }
        self.shared.played_frames.store(0, Ordering::Relaxed);
    }

    fn stop(&mut self) {
        self.current = None;
        self.shared.played_frames.store(0, Ordering::Relaxed);
        *work::lock(&self.shared.info) = Info::default();
    }

    fn has_pending(&self) -> bool {
        self.pending_at < self.pending.len()
    }

    /// Moves what fits of the converted samples into the buffer. The first time for a sound, says
    /// how long it took to be ready.
    fn push_pending(&mut self) {
        let (pushed, _) = self.buffer.push_partial_slice(&self.pending[self.pending_at..]);
        self.pending_at += pushed.len();
        if !pushed.is_empty()
            && let Some(requested) = self.awaiting_first.take()
        {
            work::lock(&self.shared.info).ready_ms = Some(requested.elapsed().as_secs_f32() * 1000.0);
            (self.on_change)();
        }
    }

    /// Decodes and converts the next packet; at the end, starts over when looping, or is done.
    fn decode_packet(&mut self) {
        let Some((stream, resampler)) = self.current.as_mut() else { return };
        match stream.next() {
            Ok(Some(source)) => {
                resampler.process(&stream.samples, source, self.device, &mut self.pending);
                self.pending_at = 0;
            }
            Ok(None) if self.shared.looping.load(Ordering::Relaxed) && stream.seek(0.0).is_ok() => {
                // Carry on from the start without flushing, so the loop has no gap.
                *resampler = Resampler::default();
            }
            Ok(None) => self.end(),
            Err(err) => {
                diag::note("Playing a sound", &err);
                work::lock(&self.shared.info).error = Some(err.to_string());
                self.end();
                (self.on_change)();
            }
        }
    }

    fn end(&mut self) {
        self.ended = true;
        self.shared.decoder_done.store(true, Ordering::Release);
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use crossbeam_channel::Sender;

    use super::*;
    use crate::audio::Source;

    const DEVICE_RATE: u32 = 48_000;

    /// A mono 16-bit WAV: `frames` frames of a slow ramp at `rate`.
    fn wav(rate: u32, frames: u32) -> Arc<[u8]> {
        let data: Vec<u8> = (0..frames).flat_map(|i| ((i % 1000) as i16 * 20).to_le_bytes()).collect();
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * 2).to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data);
        out.into()
    }

    struct Running {
        shared: Arc<Shared>,
        commands: Sender<Command>,
        buffer: rtrb::Consumer<f32>,
        changes: Arc<AtomicUsize>,
        thread: std::thread::JoinHandle<()>,
    }

    /// The decoder on its own thread, feeding a stereo buffer with room for 2 s, with no device
    /// reading it.
    fn start() -> Running {
        let shared = Arc::new(Shared::default());
        let (producer, buffer) = rtrb::RingBuffer::new(DEVICE_RATE as usize * 2 * 2);
        let (commands, receiver) = crossbeam_channel::unbounded();
        let changes = Arc::new(AtomicUsize::new(0));
        let thread = {
            let (shared, changes) = (shared.clone(), changes.clone());
            std::thread::spawn(move || {
                decoder_loop(&shared, producer, &receiver, Format { rate: DEVICE_RATE, channels: 2 }, &move || {
                    changes.fetch_add(1, Ordering::Relaxed);
                });
            })
        };
        Running { shared, commands, buffer, changes, thread }
    }

    fn wait_for(what: &str, done: impl Fn() -> bool) {
        let until = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < until, "waited too long for {what}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn play(running: &Running, key: i64) {
        let source = Source::Memory(wav(24_000, 2_400));
        let requested = Instant::now();
        running.commands.send(Command::Play { key, source, extension: "wav".to_owned(), requested }).unwrap();
    }

    #[test]
    fn a_sound_is_decoded_into_the_buffer_and_its_end_is_told() {
        let mut running = start();
        play(&running, 7);
        wait_for("the end", || running.shared.decoder_done.load(Ordering::Acquire));
        // 0.1 s of mono at 24 kHz is 0.1 s of stereo at 48 kHz, but for the last frame, which
        // the interpolation has no next frame for.
        let samples = running.buffer.slots();
        assert!((9_500..=9_600).contains(&samples), "{samples} samples");
        let first = running.buffer.read_chunk(4).unwrap();
        let (left, right) = (first.as_slices().0[0], first.as_slices().0[1]);
        assert!(left.abs() < 1e-3 && (left - right).abs() < 1e-6, "mono goes to both speakers");
        first.commit_all();
        let info = work::lock(&running.shared.info).clone();
        assert_eq!(info.key, Some(7));
        assert!(info.duration.is_some_and(|duration| (duration - 0.1).abs() < 1e-6), "{:?}", info.duration);
        assert!(info.ready_ms.is_some() && info.error.is_none());
        assert!(running.changes.load(Ordering::Relaxed) >= 2, "told on starting and once audio was ready");
        assert!(!running.shared.finished.load(Ordering::Acquire), "finished is the device's to say");

        drop(running.commands);
        running.thread.join().unwrap();
    }

    #[test]
    fn seeking_starts_again_from_there() {
        let mut running = start();
        play(&running, 1);
        wait_for("the end", || running.shared.decoder_done.load(Ordering::Acquire));
        // Played, as a device would have, so the seek has nothing buffered to drop.
        let played = running.buffer.slots();
        running.buffer.read_chunk(played).unwrap().commit_all();
        running.commands.send(Command::Seek(0.05)).unwrap();
        wait_for("the seek", || {
            work::lock(&running.shared.info).start > 0.0 && running.shared.decoder_done.load(Ordering::Acquire)
        });
        // It lands on the start of the packet that holds 0.05 s (WAV packets are 1,152 frames).
        let start = work::lock(&running.shared.info).start;
        assert!((0.04..=0.05).contains(&start), "at or just before: {start}");
        // From there to the end: 1,248 of the 2,400 frames, twice over at 48 kHz, in stereo.
        let samples = running.buffer.slots();
        assert!((4_900..=5_050).contains(&samples), "{samples} samples");
        assert_eq!(running.shared.played_frames.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn stopping_forgets_the_sound_and_a_bad_file_is_an_error() {
        let running = start();
        play(&running, 3);
        wait_for("the end", || running.shared.decoder_done.load(Ordering::Acquire));
        running.commands.send(Command::Stop).unwrap();
        wait_for("the stop", || work::lock(&running.shared.info).key.is_none());

        let source = Source::Memory(Arc::from(&b"not a sound"[..]));
        let requested = Instant::now();
        running.commands.send(Command::Play { key: 4, source, extension: "wav".to_owned(), requested }).unwrap();
        wait_for("the error", || work::lock(&running.shared.info).error.is_some());
        assert_eq!(work::lock(&running.shared.info).key, Some(4));
        assert!(running.shared.finished.load(Ordering::Acquire), "nothing will play, so it's over");
    }
}
