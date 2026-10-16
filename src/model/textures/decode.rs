//! Decoding a texture file and shrinking it to the size asked for.

use std::io::Cursor;
use std::path::Path;

use fast_image_resize as fr;
use image::ImageFormat;

use super::{Alpha, Decoded};

pub(super) fn decode_file(path: &Path, max: u32, alpha: Alpha) -> Option<Decoded> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("psd")) {
        let sampled = crate::imaging::psd::read(path, max).ok()?;
        return finish(&name, sampled.w, sampled.h, sampled.rgba, max, alpha);
    }
    let bytes = std::fs::read(path).ok()?;
    let (w, h, rgba) = decode_image(&bytes, path)?;
    finish(&name, w, h, rgba, max, alpha)
}

/// Trusts the content over the extension; TGA has no signature, so it falls back to the name.
pub(super) fn decode_image(bytes: &[u8], name: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let reader = || -> Option<image::ImageReader<Cursor<&[u8]>>> {
        let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?;
        if reader.format().is_none() {
            reader.set_format(ImageFormat::from_path(name).ok()?);
        }
        Some(reader)
    };
    // A model's textures are decoded at full size too: on the thumbnail workers they take their
    // turn in the decode budget, like pictures.
    let (w, h) = reader()?.into_dimensions().ok()?;
    let _decoding = crate::imaging::reserve_decode(w, h);
    let image = reader()?.decode().ok()?.into_rgba8();
    let (w, h) = image.dimensions();
    Some((w, h, image.into_raw()))
}

/// Decides how alpha is used, then scales the picture to fit `max`.
#[expect(clippy::too_many_arguments, reason = "refactor: take fewer, grouped arguments")]
pub(super) fn finish(name: &str, w: u32, h: u32, mut rgba: Vec<u8>, max: u32, alpha: Alpha) -> Option<Decoded> {
    if w == 0 || h == 0 || rgba.len() != w as usize * h as usize * 4 {
        return None;
    }
    let cutout = alpha == Alpha::Guess && is_cutout_mask(&rgba);
    if alpha == Alpha::Guess && !cutout {
        for pixel in rgba.as_chunks_mut::<4>().0 {
            pixel[3] = 255;
        }
    }
    let (fit_w, fit_h) = crate::imaging::fit_size(w, h, max);
    if (fit_w, fit_h) != (w, h) {
        // Opaque (most textures, and every one whose alpha was just dropped): no premultiplied
        // full-size copy needed.
        let opaque = rgba.as_chunks::<4>().0.iter().all(|px| px[3] == 255);
        let src = fr::images::Image::from_vec_u8(w, h, rgba, fr::PixelType::U8x4).ok()?;
        let mut dst = fr::images::Image::new(fit_w, fit_h, fr::PixelType::U8x4);
        let options = fr::ResizeOptions::new()
            .resize_alg(fr::ResizeAlg::Convolution(fr::FilterType::CatmullRom))
            .use_alpha(!opaque);
        fr::Resizer::new().resize(&src, &mut dst, &options).ok()?;
        rgba = dst.into_vec();
    }
    Some(Decoded { name: name.to_owned(), width: fit_w, height: fit_h, rgba, cutout })
}

/// A mask is mostly fully opaque or fully clear, with some clear pixels: leaves, chain-link,
/// lace. Smooth alpha over most of the picture is packed data, and an alpha that is clear
/// everywhere was never meant to be seen.
pub(super) fn is_cutout_mask(rgba: &[u8]) -> bool {
    let (mut clear, mut partial, mut total) = (0usize, 0usize, 0usize);
    for pixel in rgba.as_chunks::<4>().0 {
        let alpha = pixel[3];
        total += 1;
        if alpha < 128 {
            clear += 1;
        }
        if (8..248).contains(&alpha) {
            partial += 1;
        }
    }
    clear > 0 && clear < total && partial * 4 < total
}
