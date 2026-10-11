//! Sound: playback, read-ahead and waveforms.
//!
//! One output stream stays open for the app's lifetime. A decoder thread decodes the sound (from
//! memory when it was read ahead, otherwise from disk), converts it to the device's channels and
//! sample rate, and keeps a lock-free ring buffer `BUFFER_SECONDS` ahead of the playhead. The
//! audio callback only copies out of that buffer, so it never waits on a lock, the disk or a
//! decoder, and a slow disk read can't make it stutter.

use std::collections::{HashMap, VecDeque};
use std::io::{Cursor, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as DecodeError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Time, TimeBase};

use crate::diag;

/// Decoded audio kept ready ahead of the playhead: rides out a slow disk read without a gap.
const BUFFER_SECONDS: f32 = 2.0;
/// Points stored per waveform. Enough for a wide player; tiles use fewer.
pub const WAVEFORM_POINTS: usize = 512;
/// Read-ahead: files bigger than this stream from disk instead (long music, mostly).
const READ_AHEAD_MAX_FILE: u64 = 24 * 1024 * 1024;
const READ_AHEAD_BUDGET: usize = 160 * 1024 * 1024;

/// Where a sound's bytes come from.
pub enum Source {
    File(PathBuf),
    Memory(Arc<[u8]>),
}

/// A sound's shape, for drawing: the lowest and highest sample in each slice of time.
pub struct Waveform {
    /// Per point: min and max sample over all channels, scaled to -127..=127.
    pub peaks: Vec<[i8; 2]>,
    pub duration: f32,
    pub sample_rate: u32,
    pub channels: u16,
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

impl Player {
    /// `on_change` runs on the decoder thread whenever what `now` reports changes other than by
    /// the playhead moving (a seek landing, a sound starting or ending), e.g. to redraw the UI.
    pub fn start(on_change: Arc<dyn Fn() + Send + Sync>) -> Result<Self, String> {
        let device = cpal::default_host().default_output_device().ok_or("no audio output device")?;
        let device_name = device.description().map_or_else(|_| "audio output".to_owned(), |d| d.to_string());
        let supported = device.default_output_config().map_err(|err| err.to_string())?;
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
            other => Err(format!("the output's sample format ({other}) isn't supported")),
        }?;
        stream.play().map_err(|err| err.to_string())?;

        let (commands, receiver) = crossbeam_channel::unbounded();
        let decoder_shared = shared.clone();
        std::thread::Builder::new()
            .name("audio-decoder".into())
            .spawn(move || decoder_loop(&decoder_shared, producer, &receiver, rate, channels, &*on_change))
            .map_err(|err| err.to_string())?;
        Ok(Self { shared, commands, device_rate: rate, device_name, _stream: stream })
    }

    /// Plays from the start. `extension` helps the format probe ("wav", "ogg", ...).
    pub fn play(&self, key: i64, source: Source, extension: &str) {
        let _ = self.commands.send(Command::Play {
            key,
            source,
            extension: extension.to_owned(),
            requested: Instant::now(),
        });
    }

    pub fn seek(&self, seconds: f64) {
        let _ = self.commands.send(Command::Seek(seconds));
    }

    pub fn stop(&self) {
        let _ = self.commands.send(Command::Stop);
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
        let info = self.shared.info.lock().unwrap().clone();
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

fn output_stream<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut buffer: rtrb::Consumer<f32>,
    shared: Arc<Shared>,
    channels: usize,
) -> Result<cpal::Stream, String> {
    device
        .build_output_stream::<T, _, _>(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                if shared.flush.load(Ordering::Acquire) {
                    if let Ok(stale) = buffer.read_chunk(buffer.slots()) {
                        stale.commit_all();
                    }
                    shared.flush.store(false, Ordering::Release);
                }
                let mut written = 0;
                if !shared.paused.load(Ordering::Relaxed) {
                    let volume = f32::from_bits(shared.volume.load(Ordering::Relaxed));
                    let available = data.len().min(buffer.slots());
                    let whole_frames = available - available % channels;
                    if let Ok(chunk) = buffer.read_chunk(whole_frames) {
                        let (first, second) = chunk.as_slices();
                        for (out, &sample) in data.iter_mut().zip(first.iter().chain(second)) {
                            *out = T::from_sample(sample * volume);
                        }
                        chunk.commit_all();
                        written = whole_frames;
                    }
                    shared.played_frames.fetch_add((written / channels) as u64, Ordering::Relaxed);
                    if written < data.len() && shared.decoder_done.load(Ordering::Acquire) {
                        shared.finished.store(true, Ordering::Release);
                    }
                }
                for out in &mut data[written..] {
                    *out = T::EQUILIBRIUM;
                }
            },
            |err| diag::report("Audio output", err),
            None,
        )
        .map_err(|err| err.to_string())
}

fn decoder_loop(
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
                            info.error = Some(err);
                            current = None;
                            shared.finished.store(true, Ordering::Release);
                        }
                    }
                    shared.played_frames.store(0, Ordering::Relaxed);
                    *shared.info.lock().unwrap() = info;
                }
                Command::Seek(seconds) => {
                    if let Some((stream, resampler)) = &mut current {
                        let result = stream.seek(seconds);
                        *resampler = Resampler::default();
                        let mut info = shared.info.lock().unwrap();
                        match result {
                            Ok(at) => info.start = at,
                            Err(err) => info.error = Some(err),
                        }
                        shared.played_frames.store(0, Ordering::Relaxed);
                    }
                }
                Command::Stop => {
                    current = None;
                    shared.played_frames.store(0, Ordering::Relaxed);
                    *shared.info.lock().unwrap() = Info::default();
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
                shared.info.lock().unwrap().ready_ms = Some(requested.elapsed().as_secs_f32() * 1000.0);
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
                shared.info.lock().unwrap().error = Some(err);
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

/// One sound being decoded, packet by packet.
struct SoundStream {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    time_base: Option<TimeBase>,
    duration: Option<f64>,
    /// The last decoded packet: interleaved, at the source's rate.
    samples: Vec<f32>,
}

impl SoundStream {
    fn open(source: Source, extension: &str) -> Result<Self, String> {
        let media: Box<dyn MediaSource> = match source {
            Source::File(path) => Box::new(std::fs::File::open(path).map_err(|err| err.to_string())?),
            Source::Memory(bytes) => Box::new(Cursor::new(bytes)),
        };
        let media = MediaSourceStream::new(media, MediaSourceStreamOptions::default());
        let mut hint = Hint::new();
        hint.with_extension(extension);
        let format = symphonia::default::get_probe()
            .probe(&hint, media, FormatOptions::default(), MetadataOptions::default())
            .map_err(|err| err.to_string())?;
        let track = format.default_track(TrackType::Audio).ok_or("no audio track")?;
        let params = track.codec_params.as_ref().and_then(|params| params.audio()).ok_or("no audio track")?;
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(params, &AudioDecoderOptions::default())
            .map_err(|err| err.to_string())?;
        let duration = match (track.num_frames, params.sample_rate) {
            (Some(frames), Some(rate)) if rate > 0 => Some(frames as f64 / rate as f64),
            _ => None,
        };
        let (track_id, time_base) = (track.id, track.time_base);
        Ok(Self { format, decoder, track_id, time_base, duration, samples: Vec::new() })
    }

    /// Decodes the next packet into `samples`. Returns its (sample rate, channels), or `None`
    /// at the end.
    fn next(&mut self) -> Result<Option<(u32, usize)>, String> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) => return Ok(None),
                // Many files end without a clean end-of-stream marker.
                Err(DecodeError::IoError(err)) if err.kind() == ErrorKind::UnexpectedEof => return Ok(None),
                Err(err) => return Err(err.to_string()),
            };
            if packet.track_id != self.track_id {
                continue;
            }
            match self.decoder.decode(&packet) {
                Ok(decoded) => {
                    if decoded.frames() == 0 {
                        continue;
                    }
                    let spec = (decoded.spec().rate(), decoded.spec().channels().count());
                    decoded.copy_to_vec_interleaved(&mut self.samples);
                    return Ok(Some(spec));
                }
                // A damaged packet: skip it, as other players do.
                Err(DecodeError::DecodeError(_)) => continue,
                Err(err) => return Err(err.to_string()),
            }
        }
    }

    /// Returns where it actually landed, in seconds (at or just before `seconds`).
    fn seek(&mut self, seconds: f64) -> Result<f64, String> {
        let time = Time::try_from_secs_f64(seconds.max(0.0)).ok_or("can't seek there")?;
        let seeked = self
            .format
            .seek(SeekMode::Accurate, SeekTo::Time { time, track_id: Some(self.track_id) })
            .map_err(|err| err.to_string())?;
        self.decoder.reset();
        Ok(self.time_base.and_then(|base| base.calc_time(seeked.actual_ts)).map_or(seconds, |time| time.as_secs_f64()))
    }
}

/// Converts decoded audio to the device's channel count and sample rate. Linear interpolation:
/// plenty for auditioning, and cheap enough to never hold up playback.
#[derive(Default)]
struct Resampler {
    /// Position of the next output frame, in source frames from the start of the next packet
    /// (-1 is the last frame of the previous packet).
    position: f64,
    previous: Vec<f32>,
    mixed: Vec<f32>,
}

impl Resampler {
    fn process(
        &mut self,
        input: &[f32],
        in_channels: usize,
        in_rate: u32,
        out_channels: usize,
        out_rate: u32,
        out: &mut Vec<f32>,
    ) {
        out.clear();
        if in_channels == 0 {
            return;
        }
        let frames = input.len() / in_channels;
        if frames == 0 {
            return;
        }
        if self.previous.len() != in_channels {
            // First packet (or the channel count changed): nothing to blend with yet.
            self.previous.clear();
            self.position = self.position.max(0.0);
        }
        if in_rate == out_rate {
            for source in input.chunks_exact(in_channels) {
                map_channels(source, out_channels, out);
            }
        } else {
            let step = in_rate as f64 / out_rate as f64;
            let last = (frames - 1) as f64;
            let mut position = self.position;
            while position < last {
                let index = position.floor();
                let t = (position - index) as f32;
                let index = index as isize;
                let a =
                    if index < 0 { &self.previous[..] } else { &input[index as usize * in_channels..][..in_channels] };
                let b = &input[(index + 1) as usize * in_channels..][..in_channels];
                self.mixed.clear();
                self.mixed.extend(a.iter().zip(b).map(|(a, b)| a + (b - a) * t));
                map_channels(&self.mixed, out_channels, out);
                position += step;
            }
            self.position = position - frames as f64;
        }
        self.previous.clear();
        self.previous.extend_from_slice(&input[(frames - 1) * in_channels..]);
    }
}

/// Mono goes to every speaker, anything goes to mono as an average, and otherwise channels
/// map in order (front left and right first), with extra outputs left silent.
fn map_channels(frame: &[f32], out_channels: usize, out: &mut Vec<f32>) {
    let in_channels = frame.len();
    if in_channels == out_channels {
        out.extend_from_slice(frame);
    } else if in_channels == 1 {
        out.extend(std::iter::repeat_n(frame[0], out_channels));
    } else if out_channels == 1 {
        out.push(frame.iter().sum::<f32>() / in_channels as f32);
    } else {
        out.extend((0..out_channels).map(|channel| frame.get(channel).copied().unwrap_or(0.0)));
    }
}

/// Opens a sound and decodes its first packet: the work playback waits for before the first audio
/// can reach the sound card. For the benchmark.
pub fn time_to_first_audio(source: Source, extension: &str) -> Result<Duration, String> {
    let start = Instant::now();
    let mut stream = SoundStream::open(source, extension)?;
    stream.next()?;
    Ok(start.elapsed())
}

/// Decodes a whole sound file and measures its shape.
pub fn waveform(path: &Path) -> Result<Waveform, String> {
    // One sequential read is far cheaper than many small ones on a spinning disk.
    let bytes: Arc<[u8]> = std::fs::read(path).map_err(|err| err.to_string())?.into();
    let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
    let mut stream = SoundStream::open(Source::Memory(bytes), extension)?;

    // Min/max per block of frames first, then grouped into the stored points.
    const BLOCK: usize = 256;
    let mut blocks: Vec<[f32; 2]> = Vec::new();
    let mut current = [f32::MAX, f32::MIN];
    let (mut in_block, mut frames) = (0usize, 0u64);
    let (mut sample_rate, mut channels) = (0u32, 0usize);
    while let Some((rate, packet_channels)) = stream.next()? {
        (sample_rate, channels) = (rate, packet_channels);
        for frame in stream.samples.chunks_exact(packet_channels.max(1)) {
            for &sample in frame {
                current[0] = current[0].min(sample);
                current[1] = current[1].max(sample);
            }
            in_block += 1;
            if in_block == BLOCK {
                blocks.push(current);
                current = [f32::MAX, f32::MIN];
                in_block = 0;
            }
        }
        frames += (stream.samples.len() / packet_channels.max(1)) as u64;
    }
    if in_block > 0 {
        blocks.push(current);
    }
    if frames == 0 || sample_rate == 0 {
        return Err("no audio in this file".into());
    }

    let points = blocks.len().min(WAVEFORM_POINTS);
    let scale = |value: f32| (value.clamp(-1.0, 1.0) * 127.0).round() as i8;
    let peaks = (0..points)
        .map(|point| {
            let start = point * blocks.len() / points;
            let end = ((point + 1) * blocks.len() / points).max(start + 1);
            let (low, high) = blocks[start..end]
                .iter()
                .fold((f32::MAX, f32::MIN), |(low, high), block| (low.min(block[0]), high.max(block[1])));
            [scale(low), scale(high)]
        })
        .collect();
    Ok(Waveform { peaks, duration: frames as f32 / sample_rate as f32, sample_rate, channels: channels as u16 })
}

/// Reads the sounds next to the selection into memory, so moving to one starts it without
/// waiting on the disk.
pub struct ReadAhead {
    shared: Arc<ReadAheadShared>,
}

#[derive(Default)]
struct ReadAheadShared {
    wanted: Mutex<VecDeque<(i64, PathBuf)>>,
    wake: Condvar,
    cache: Mutex<ByteCache>,
    shutdown: AtomicBool,
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
        let spawned = std::thread::Builder::new().name("read-ahead".into()).spawn(move || read_ahead_loop(&worker));
        if let Err(err) = spawned {
            diag::report("Starting sound read-ahead", err);
        }
        Self { shared }
    }

    /// Replaces the list of files to read: (key, path, size). Big files are skipped.
    pub fn want(&self, files: Vec<(i64, PathBuf, u64)>) {
        let mut wanted = self.shared.wanted.lock().unwrap();
        wanted.clear();
        wanted.extend(
            files.into_iter().filter(|(_, _, size)| *size <= READ_AHEAD_MAX_FILE).map(|(key, path, _)| (key, path)),
        );
        drop(wanted);
        self.shared.wake.notify_one();
    }

    pub fn get(&self, key: i64) -> Option<Arc<[u8]>> {
        self.shared.cache.lock().unwrap().files.get(&key).cloned()
    }
}

impl Drop for ReadAhead {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::Relaxed);
        self.shared.wake.notify_one();
    }
}

fn read_ahead_loop(shared: &ReadAheadShared) {
    loop {
        let (key, path) = {
            let mut wanted = shared.wanted.lock().unwrap();
            loop {
                if shared.shutdown.load(Ordering::Relaxed) {
                    return;
                }
                if let Some(next) = wanted.pop_front() {
                    break next;
                }
                wanted = shared.wake.wait(wanted).unwrap();
            }
        };
        if shared.cache.lock().unwrap().files.contains_key(&key) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else { continue };
        let mut cache = shared.cache.lock().unwrap();
        cache.bytes += bytes.len();
        cache.files.insert(key, bytes.into());
        cache.order.push_back(key);
        while cache.bytes > READ_AHEAD_BUDGET {
            let Some(oldest) = cache.order.pop_front() else { break };
            if let Some(evicted) = cache.files.remove(&oldest) {
                cache.bytes -= evicted.len();
            }
        }
    }
}
