//! One sound being decoded with symphonia, packet by packet.

use std::io::{Cursor, ErrorKind};

use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as DecodeError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Time, TimeBase};

use super::{Format, Source};

/// One sound being decoded, packet by packet.
pub(super) struct SoundStream {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    time_base: Option<TimeBase>,
    pub(super) duration: Option<f64>,
    /// The last decoded packet: interleaved, at the source's rate.
    pub(super) samples: Vec<f32>,
}

impl SoundStream {
    pub(super) fn open(source: Source, extension: &str) -> Result<Self, String> {
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

    /// Decodes the next packet into `samples`. Returns its format, or `None` at the end.
    pub(super) fn next(&mut self) -> Result<Option<Format>, String> {
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
                    let spec = Format { rate: decoded.spec().rate(), channels: decoded.spec().channels().count() };
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
    pub(super) fn seek(&mut self, seconds: f64) -> Result<f64, String> {
        let time = Time::try_from_secs_f64(seconds.max(0.0)).ok_or("can't seek there")?;
        let seeked = self
            .format
            .seek(SeekMode::Accurate, SeekTo::Time { time, track_id: Some(self.track_id) })
            .map_err(|err| err.to_string())?;
        self.decoder.reset();
        Ok(self.time_base.and_then(|base| base.calc_time(seeked.actual_ts)).map_or(seconds, |time| time.as_secs_f64()))
    }
}
