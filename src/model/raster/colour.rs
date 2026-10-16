//! sRGB to linear and back, by table.

use std::sync::OnceLock;

use crate::model::{linear_to_srgb, srgb_to_linear};

pub(super) fn srgb8_to_linear() -> &'static [f32; 256] {
    static LUT: OnceLock<[f32; 256]> = OnceLock::new();
    LUT.get_or_init(|| std::array::from_fn(|i| srgb_to_linear(i as f32 / 255.0)))
}

/// Linear 0..1 in 4,096 steps to 8-bit sRGB: fine enough that no step is visible.
pub(super) fn linear_to_srgb8() -> &'static [u8; 4096] {
    static LUT: OnceLock<[u8; 4096]> = OnceLock::new();
    LUT.get_or_init(|| std::array::from_fn(|i| (linear_to_srgb(i as f32 / 4095.0) * 255.0).round() as u8))
}
