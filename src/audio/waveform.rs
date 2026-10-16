//! A sound's shape for drawing, measured by decoding the whole file once.

use std::path::Path;
use std::sync::Arc;

use super::stream::SoundStream;
use super::{Format, SoundError, Source};

/// Points stored per waveform. Enough for a wide player; tiles use fewer.
const WAVEFORM_POINTS: usize = 512;

/// A sound's shape, for drawing: the lowest and highest sample in each slice of time.
pub struct Waveform {
    /// Per point: min and max sample over all channels, scaled to -127..=127.
    pub peaks: Vec<[i8; 2]>,
    pub duration: f32,
    pub sample_rate: u32,
    pub channels: u16,
}

/// Decodes a whole sound file and measures its shape.
pub fn waveform(path: &Path) -> Result<Waveform, SoundError> {
    // One sequential read is far cheaper than many small ones on a spinning disk.
    let bytes: Arc<[u8]> = std::fs::read(path)?.into();
    let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
    let mut stream = SoundStream::open(Source::Memory(bytes), extension)?;

    // Min/max per block of frames first, then grouped into the stored points.
    const BLOCK: usize = 256;
    let mut blocks: Vec<[f32; 2]> = Vec::new();
    let mut current = [f32::MAX, f32::MIN];
    let (mut in_block, mut frames) = (0usize, 0u64);
    let (mut sample_rate, mut channels) = (0u32, 0usize);
    while let Some(Format { rate, channels: packet_channels }) = stream.next()? {
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
        return Err(SoundError::NoAudio);
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
