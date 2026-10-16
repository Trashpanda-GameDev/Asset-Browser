//! The picture being drawn: samples with their depth and triangle, resolved to pixels at the end.

use super::SUPERSAMPLE;
use super::colour::{self, linear_to_srgb8};

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
                *pixel = colour::pixel(lut, sum.map(|c| c / count as f32), count);
            }
        }
        out
    }
}
