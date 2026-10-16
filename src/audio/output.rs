//! The audio callback: copies what the decoder thread buffered to the device, and does nothing
//! else, so it never waits on a lock, the disk or a decoder.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use cpal::traits::DeviceTrait;
use cpal::{FromSample, SizedSample};

use super::{OutputError, Shared};
use crate::diag;

pub(super) fn output_stream<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut buffer: rtrb::Consumer<f32>,
    shared: Arc<Shared>,
    channels: usize,
) -> Result<cpal::Stream, OutputError> {
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
        .map_err(OutputError::from)
}
