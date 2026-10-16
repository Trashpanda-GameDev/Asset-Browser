//! The picture being drawn: samples with their depth and triangle, resolved to pixels at the end.

use super::colour::linear_to_srgb8;
use super::{SAMPLES, SUPERSAMPLE};

/// Per sample colour, for the stick figures.
pub(super) struct Canvas {
    pub(super) side: usize,
    /// Linear, 0..1.
    pub(super) color: Vec<[f32; 3]>,
    pub(super) covered: Vec<bool>,
}

impl Canvas {
    pub(super) fn new(side: usize) -> Self {
        Canvas { side, color: vec![[0.0; 3]; side * side], covered: vec![false; side * side] }
    }

    /// Averages each block of samples into one pixel: coverage becomes alpha.
    pub(super) fn downsample(&self, size: usize) -> Vec<u8> {
        let lut = linear_to_srgb8();
        let mut out = vec![0u8; size * size * 4];
        for (index, pixel) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let (px, py) = (index % size, index / size);
            let mut sum = [0.0f32; 3];
            let mut count = 0usize;
            for sy in 0..SUPERSAMPLE {
                let row = (py * SUPERSAMPLE + sy) * self.side + px * SUPERSAMPLE;
                for sample in row..row + SUPERSAMPLE {
                    if self.covered[sample] {
                        let c = self.color[sample];
                        sum = [sum[0] + c[0], sum[1] + c[1], sum[2] + c[2]];
                        count += 1;
                    }
                }
            }
            if count > 0 {
                let rgb = sum.map(|c| lut[((c / count as f32) * 4095.0 + 0.5) as usize & 4095]);
                *pixel = [rgb[0], rgb[1], rgb[2], (count as f32 / SAMPLES as f32 * 255.0).round() as u8];
            }
        }
        out
    }
}
