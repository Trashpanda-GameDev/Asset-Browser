//! Decoded audio to the device's channels and sample rate.

use super::Format;

/// Converts decoded audio to the device's channel count and sample rate. Linear interpolation:
/// plenty for auditioning, and cheap enough to never hold up playback.
#[derive(Default)]
pub(super) struct Resampler {
    /// Position of the next output frame, in source frames from the start of the next packet
    /// (-1 is the last frame of the previous packet).
    position: f64,
    previous: Vec<f32>,
    mixed: Vec<f32>,
}

impl Resampler {
    /// Converts `input`, in `from`'s format, to `to`'s, into `out`.
    pub(super) fn process(&mut self, input: &[f32], from: Format, to: Format, out: &mut Vec<f32>) {
        let (in_channels, in_rate) = (from.channels, from.rate);
        let (out_channels, out_rate) = (to.channels, to.rate);
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
