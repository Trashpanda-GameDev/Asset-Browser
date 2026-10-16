//! sRGB to linear and back, by table.

use std::sync::OnceLock;

use super::SAMPLES;
use crate::model::{linear_to_srgb, srgb_to_linear};

/// Steps from linear 0 to 1 in the table back to sRGB: fine enough that no step is visible.
const LINEAR_STEPS: usize = 4096;

pub(super) fn srgb8_to_linear() -> &'static [f32; 256] {
    static LUT: OnceLock<[f32; 256]> = OnceLock::new();
    LUT.get_or_init(|| std::array::from_fn(|i| srgb_to_linear(i as f32 / 255.0)))
}

/// Linear 0..1 in `LINEAR_STEPS` steps to 8-bit sRGB.
pub(super) fn linear_to_srgb8() -> &'static [u8; LINEAR_STEPS] {
    static LUT: OnceLock<[u8; LINEAR_STEPS]> = OnceLock::new();
    let last = (LINEAR_STEPS - 1) as f32;
    LUT.get_or_init(|| std::array::from_fn(|i| (linear_to_srgb(i as f32 / last) * 255.0).round() as u8))
}

/// A pixel: its linear colour through `table` (`linear_to_srgb8`), and the share of its samples
/// covered as its alpha.
pub(super) fn pixel(table: &[u8; LINEAR_STEPS], color: [f32; 3], covered: usize) -> [u8; 4] {
    let last = (LINEAR_STEPS - 1) as f32;
    let [r, g, b] = color.map(|c| table[(c * last + 0.5) as usize & (LINEAR_STEPS - 1)]);
    [r, g, b, (covered as f32 / SAMPLES as f32 * 255.0).round() as u8]
}
