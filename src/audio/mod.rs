//! Sound: playback, read-ahead and waveforms.
//!
//! One output stream stays open for the app's lifetime. A decoder thread decodes the sound (from
//! memory when it was read ahead, otherwise from disk), converts it to the device's channels and
//! sample rate, and keeps a lock-free ring buffer `BUFFER_SECONDS` ahead of the playhead. The
//! audio callback only copies out of that buffer, so it never waits on a lock, the disk or a
//! decoder, and a slow disk read can't make it stutter.

mod decoder;
mod output;
mod read_ahead;
mod resample;
mod stream;
mod waveform;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::SampleFormat;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::Sender;

use crate::work::{self, Wake};
use decoder::decoder_loop;
use output::output_stream;
use stream::SoundStream;

pub use read_ahead::{ReadAhead, set_read_ahead_budget};
pub use waveform::{Waveform, waveform};

/// Decoded audio kept ready ahead of the playhead: rides out a slow disk read without a gap.
const BUFFER_SECONDS: f32 = 2.0;

/// How samples come: frames a second, and samples per frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Format {
    rate: u32,
    channels: usize,
}

/// Where a sound's bytes come from.
pub enum Source {
    File(PathBuf),
    Memory(Arc<[u8]>),
}

/// What's playing, as of this frame.
pub struct NowPlaying {
    pub key: i64,
    pub position: f64,
    pub duration: Option<f64>,
    pub paused: bool,
    /// Reached the end (and isn't looping), or failed.
    pub finished: bool,
    /// From the play request to the first audio handed to the device.
    pub ready_ms: Option<f32>,
    pub error: Option<String>,
}

impl NowPlaying {
    /// How far through the sound playback is, from 0 to 1; `length` stands in until the player
    /// knows the sound's own.
    pub fn progress(&self, length: f64) -> f32 {
        (self.position / self.duration.unwrap_or(length).max(f64::EPSILON)) as f32
    }
}

pub struct Player {
    shared: Arc<Shared>,
    commands: Sender<Command>,
    device_rate: u32,
    pub device_name: String,
    _stream: cpal::Stream,
}

enum Command {
    Play { key: i64, source: Source, extension: String, requested: Instant },
    Seek(f64),
    Stop,
}

#[derive(Default)]
struct Shared {
    /// Set by the decoder thread to have the callback drop everything buffered; the callback
    /// clears it once done.
    flush: AtomicBool,
    paused: AtomicBool,
    looping: AtomicBool,
    volume: AtomicU32,
    /// Device frames played since `Info::start`.
    played_frames: AtomicU64,
    decoder_done: AtomicBool,
    finished: AtomicBool,
    info: Mutex<Info>,
}

#[derive(Clone, Default)]
struct Info {
    key: Option<i64>,
    duration: Option<f64>,
    /// Position in seconds where `played_frames` started counting.
    start: f64,
    ready_ms: Option<f32>,
    error: Option<String>,
}

/// Why there's no sound output.
#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    #[error("no audio output device")]
    NoDevice,
    /// What the device said. (cpal's error isn't a standard one, so it's taken in by hand.)
    #[error("{0}")]
    Device(cpal::Error),
    #[error("the output's sample format ({0}) isn't supported")]
    SampleFormat(SampleFormat),
    #[error("the sound decoder couldn't start")]
    NoDecoder,
}

impl From<cpal::Error> for OutputError {
    fn from(err: cpal::Error) -> Self {
        Self::Device(err)
    }
}

/// Why a sound can't be played or measured. The message is what the user reads, under the player
/// or in place of a waveform (the thumbnail cache keeps it with a failed file).
#[derive(Debug, thiserror::Error)]
pub enum SoundError {
    #[error("{0}")]
    Read(#[from] std::io::Error),
    #[error("{0}")]
    Decode(#[from] symphonia::core::errors::Error),
    #[error("no audio track")]
    NoTrack,
    #[error("can't seek there")]
    BadSeek,
    #[error("no audio in this file")]
    NoAudio,
}

impl Player {
    /// `on_change` runs on the decoder thread whenever what `now` reports changes other than by
    /// the playhead moving (a seek landing, a sound starting or ending), e.g. to redraw the UI.
    pub fn start(on_change: Wake) -> Result<Self, OutputError> {
        let device = cpal::default_host().default_output_device().ok_or(OutputError::NoDevice)?;
        let device_name = device.description().map_or_else(|_| "audio output".to_owned(), |d| d.to_string());
        let supported = device.default_output_config()?;
        let rate = supported.sample_rate();
        let channels = supported.channels() as usize;
        let (producer, consumer) = rtrb::RingBuffer::new((rate as f32 * BUFFER_SECONDS) as usize * channels);
        let shared = Arc::new(Shared::default());
        shared.volume.store(0.8f32.to_bits(), Ordering::Relaxed);

        let config = supported.config();
        let stream = match supported.sample_format() {
            SampleFormat::F32 => output_stream::<f32>(&device, config, consumer, shared.clone(), channels),
            SampleFormat::I16 => output_stream::<i16>(&device, config, consumer, shared.clone(), channels),
            SampleFormat::U16 => output_stream::<u16>(&device, config, consumer, shared.clone(), channels),
            SampleFormat::I32 => output_stream::<i32>(&device, config, consumer, shared.clone(), channels),
            other => Err(OutputError::SampleFormat(other)),
        }?;
        stream.play()?;

        let (commands, receiver) = crossbeam_channel::unbounded();
        let decoder_shared = shared.clone();
        work::spawn("audio-decoder", move || {
            decoder_loop(&decoder_shared, producer, &receiver, Format { rate, channels }, &*on_change);
        })
        .ok_or(OutputError::NoDecoder)?;
        Ok(Self { shared, commands, device_rate: rate, device_name, _stream: stream })
    }

    /// Plays from the start. `extension` helps the format probe ("wav", "ogg", ...).
    pub fn play(&self, key: i64, source: Source, extension: &str) {
        self.send(Command::Play { key, source, extension: extension.to_owned(), requested: Instant::now() });
    }

    pub fn seek(&self, seconds: f64) {
        self.send(Command::Seek(seconds));
    }

    pub fn stop(&self) {
        self.send(Command::Stop);
    }

    fn send(&self, command: Command) {
        // Fails only when the decoder thread has died, which the panic hook has logged: then
        // there's no sound to play.
        let _ = self.commands.send(command);
    }

    pub fn set_paused(&self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Relaxed);
    }

    pub fn set_looping(&self, looping: bool) {
        self.shared.looping.store(looping, Ordering::Relaxed);
    }

    pub fn set_volume(&self, volume: f32) {
        self.shared.volume.store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn now(&self) -> Option<NowPlaying> {
        let info = work::lock(&self.shared.info).clone();
        let key = info.key?;
        let played = self.shared.played_frames.load(Ordering::Relaxed) as f64 / self.device_rate as f64;
        let mut position = info.start + played;
        if let Some(duration) = info.duration.filter(|d| *d > 0.0) {
            position =
                if self.shared.looping.load(Ordering::Relaxed) { position % duration } else { position.min(duration) };
        }
        Some(NowPlaying {
            key,
            position,
            duration: info.duration,
            paused: self.shared.paused.load(Ordering::Relaxed),
            finished: self.shared.finished.load(Ordering::Acquire),
            ready_ms: info.ready_ms,
            error: info.error,
        })
    }
}

/// Opens a sound and decodes its first packet: the work playback waits for before the first audio
/// can reach the sound card. For the benchmark.
pub fn time_to_first_audio(source: Source, extension: &str) -> Result<Duration, SoundError> {
    let start = Instant::now();
    let mut stream = SoundStream::open(source, extension)?;
    stream.next()?;
    Ok(start.elapsed())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_stay_as_they_are_shown() {
        // The thumbnail cache keeps a waveform's failure with the file.
        assert_eq!(SoundError::NoAudio.to_string(), "no audio in this file");
        assert_eq!(SoundError::NoTrack.to_string(), "no audio track");
        assert_eq!(SoundError::BadSeek.to_string(), "can't seek there");
        assert_eq!(OutputError::NoDevice.to_string(), "no audio output device");
        let format = OutputError::SampleFormat(SampleFormat::U8);
        assert_eq!(format.to_string(), "the output's sample format (u8) isn't supported");
    }
}
