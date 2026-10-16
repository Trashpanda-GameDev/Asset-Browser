//! The preview picture Blender saves inside a .blend file.
//!
//! A .blend file is Blender's memory written out, readable only with the structure definitions of
//! the Blender version that wrote it, so the geometry is out of reach. But near the start, after
//! the render settings, Blender writes a `TEST` block holding a thumbnail of the scene, which is
//! what the grid shows.
//!
//! Layout: a 12-byte header: "BLENDER", the pointer size (`_` 4 bytes, `-` 8 bytes), the byte
//! order (`v` little-endian, `V` big-endian) and three version digits ("279" is 2.79). Then
//! blocks, each a 4-byte code, the data length (i32), the old memory address (pointer size), the
//! SDNA index and count (i32 each), then the data. Blender 5 writes a 17-byte header, "BLENDER",
//! its size ("17"), "-", a format version ("01"), "v" and four version digits ("0500"), and its
//! blocks have 64-bit lengths: code, SDNA index (i32), old address (u64), length (i64), count
//! (i64). The TEST block holds the width and height (i32), then width × height RGBA pixels, rows
//! from the bottom up. Compressed files are gzip (before 3.0) or zstd (3.0 on); only their
//! beginning is decompressed.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;

use super::ModelError;

/// Stops a damaged length from asking for gigabytes; Blender's previews are 128 or 256 square.
const MAX_SIDE: i64 = 2048;

pub struct Preview {
    pub width: u32,
    pub height: u32,
    /// sRGB, straight alpha, top row first.
    pub rgba: Vec<u8>,
    /// "Blender 2.79"
    pub format: String,
}

pub fn read_preview(path: &Path) -> Result<Preview, ModelError> {
    let file = File::open(path)?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let magic: [u8; 4] = match reader.fill_buf()? {
        [a, b, c, d, ..] => [*a, *b, *c, *d],
        _ => return Err("not a Blender file".into()),
    };
    match magic {
        [0x1f, 0x8b, _, _] => read_from(BufReader::new(flate2::read::MultiGzDecoder::new(reader))),
        [0x28, 0xb5, 0x2f, 0xfd] => read_from(BufReader::new(ZstdFrames::new(reader))),
        _ => read_from(reader),
    }
}

fn read_from(mut r: impl Read) -> Result<Preview, ModelError> {
    let mut header = [0u8; 12];
    r.read_exact(&mut header).map_err(|_| "not a Blender file")?;
    if &header[..7] != b"BLENDER" {
        return Err("not a Blender file".into());
    }
    let layout = match header[7] {
        b'_' | b'-' => Layout {
            pointer: if header[7] == b'_' { 4 } else { 8 },
            big_endian: match header[8] {
                b'v' => false,
                b'V' => true,
                _ => return Err("unknown Blender file header".into()),
            },
            large: false,
            version: digits(&header[9..12])?,
        },
        b'0'..=b'9' => {
            let mut rest = [0u8; 5];
            r.read_exact(&mut rest).map_err(|_| "Blender file header cut short")?;
            if &header[7..10] != b"17-" || &header[10..12] != b"01" || rest[0] != b'v' {
                return Err("this Blender file format is newer than this app can read".into());
            }
            Layout { pointer: 8, big_endian: false, large: true, version: digits(&rest[1..5])? }
        }
        _ => return Err("unknown Blender file header".into()),
    };
    let format = format!("Blender {}", version_name(layout.version));

    let mut block = vec![0u8; layout.block_header_len()];
    loop {
        if r.read_exact(&mut block).is_err() {
            return Err("this Blender file has no saved preview".into());
        }
        let (code, len) = layout.parse(&block);
        match &code {
            b"TEST" => {
                let (width, height, rgba) = read_test(&mut r, &layout, len)?;
                return Ok(Preview { width, height, rgba, format });
            }
            // The preview is written before the global settings and the data; past them it isn't
            // coming.
            b"GLOB" | b"DATA" | b"DNA1" | b"ENDB" => return Err("this Blender file has no saved preview".into()),
            _ => {
                let len = u64::try_from(len).map_err(|_| "damaged Blender file")?;
                let skipped = io::copy(&mut (&mut r).take(len), &mut io::sink())?;
                if skipped < len {
                    return Err("this Blender file has no saved preview".into());
                }
            }
        }
    }
}

/// Blender's preview is its viewport through the camera: often a small object in the middle of a
/// clear picture, inside the camera's dashed frame. With a clear background, the frame is wiped
/// and the picture cropped to a square around what's drawn (zooming in at most 3×), so the object
/// fills the thumbnail as the renders do. A picture with a background is left whole.
pub fn crop_to_content(preview: &Preview) -> (u32, u32, Vec<u8>) {
    /// Alpha below this is a clear pixel.
    const CLEAR: u8 = 16;
    /// The camera's frame: within this many pixels of the edges, a faint dark band (alpha below
    /// `FRAME_ALPHA`) and a dashed line of pure black pixels.
    const FRAME: usize = 3;
    const FRAME_ALPHA: u8 = 160;
    /// Room left around what's drawn, as a share of its size.
    const MARGIN: f32 = 1.12;
    /// The crop is at least this share of the picture's side: zooming in at most this much.
    const MAX_ZOOM: usize = 3;

    let (w, h) = (preview.width as usize, preview.height as usize);
    let whole = (preview.width, preview.height, preview.rgba.clone());
    let alpha = |x: usize, y: usize| preview.rgba[(y * w + x) * 4 + 3];
    let clear = preview.rgba.as_chunks::<4>().0.iter().filter(|p| p[3] < CLEAR).count();
    if w < 8 || h < 8 || clear * 4 < w * h {
        return whole;
    }
    let frame = |x: usize, y: usize| {
        let at = (y * w + x) * 4;
        let edge = x < FRAME || y < FRAME || x >= w - FRAME || y >= h - FRAME;
        edge && (alpha(x, y) < FRAME_ALPHA || preview.rgba[at..at + 3] == [0, 0, 0])
    };
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for y in 0..h {
        for x in 0..w {
            if alpha(x, y) >= CLEAR && !frame(x, y) {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            }
        }
    }
    if x0 > x1 {
        return whole;
    }
    let side = (((x1 - x0 + 1).max(y1 - y0 + 1) as f32 * MARGIN).ceil() as usize).max(w.min(h) / MAX_ZOOM);
    let (cx, cy) = ((x0 + x1 + 1) as isize, (y0 + y1 + 1) as isize);
    let (left, top) = ((cx - side as isize) / 2, (cy - side as isize) / 2);
    let mut rgba = vec![0u8; side * side * 4];
    for (i, pixel) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (x, y) = (left + (i % side) as isize, top + (i / side) as isize);
        if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && !frame(x as usize, y as usize) {
            let at = (y as usize * w + x as usize) * 4;
            pixel.copy_from_slice(&preview.rgba[at..at + 4]);
        }
    }
    (side as u32, side as u32, rgba)
}

fn read_test(r: &mut impl Read, layout: &Layout, len: i64) -> Result<(u32, u32, Vec<u8>), ModelError> {
    let mut size = [0u8; 8];
    r.read_exact(&mut size).map_err(|_| "the saved preview is cut short")?;
    let width = layout.i32(&size[..4]) as i64;
    let height = layout.i32(&size[4..]) as i64;
    if !(1..=MAX_SIDE).contains(&width) || !(1..=MAX_SIDE).contains(&height) || len < 8 + width * height * 4 {
        return Err("the saved preview is damaged".into());
    }
    let row = width as usize * 4;
    let mut pixels = vec![0u8; row * height as usize];
    r.read_exact(&mut pixels).map_err(|_| "the saved preview is cut short")?;
    // Stored bottom row first.
    let rgba = pixels.chunks_exact(row).rev().flatten().copied().collect();
    Ok((width as u32, height as u32, rgba))
}

fn digits(bytes: &[u8]) -> Result<u32, ModelError> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return Err("unknown Blender file header".into());
    }
    Ok(bytes.iter().fold(0, |n, &digit| n * 10 + u32::from(digit - b'0')))
}

/// 279 → "2.79", 402 → "4.2", 500 → "5.0": 2.x versions had two-digit minor numbers.
fn version_name(version: u32) -> String {
    let (major, minor) = (version / 100, version % 100);
    if major < 3 { format!("{major}.{minor:02}") } else { format!("{major}.{minor}") }
}

struct Layout {
    pointer: usize,
    big_endian: bool,
    /// Blender 5's blocks, with 64-bit lengths.
    large: bool,
    version: u32,
}

impl Layout {
    fn block_header_len(&self) -> usize {
        if self.large { 32 } else { 12 + self.pointer + 4 }
    }

    /// (code, data length)
    fn parse(&self, block: &[u8]) -> ([u8; 4], i64) {
        let code = [block[0], block[1], block[2], block[3]];
        if self.large {
            let mut len = [0u8; 8];
            len.copy_from_slice(&block[16..24]);
            (code, i64::from_le_bytes(len))
        } else {
            (code, self.i32(&block[4..8]) as i64)
        }
    }

    fn i32(&self, bytes: &[u8]) -> i32 {
        let bytes = [bytes[0], bytes[1], bytes[2], bytes[3]];
        if self.big_endian { i32::from_be_bytes(bytes) } else { i32::from_le_bytes(bytes) }
    }
}

/// Reads zstd frames one after another: Blender compresses in many frames, followed by a seek
/// table in a skippable frame, where reading ends.
struct ZstdFrames<R: Read> {
    decoder: Option<ruzstd::decoding::StreamingDecoder<R, ruzstd::decoding::FrameDecoder>>,
    source: Option<R>,
}

impl<R: Read> ZstdFrames<R> {
    fn new(source: R) -> Self {
        ZstdFrames { decoder: None, source: Some(source) }
    }
}

impl<R: Read> Read for ZstdFrames<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if let Some(decoder) = &mut self.decoder {
                let n = decoder.read(buf)?;
                if n > 0 || buf.is_empty() {
                    return Ok(n);
                }
                // This frame is done; the next one starts where it ended.
                self.source = self.decoder.take().map(|decoder| decoder.into_inner());
            }
            let Some(source) = self.source.take() else { return Ok(0) };
            match ruzstd::decoding::StreamingDecoder::new(source) {
                Ok(decoder) => self.decoder = Some(decoder),
                Err(_) => return Ok(0),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use std::io::Write;

    /// A legacy .blend: header, a REND block, the TEST preview (2 × 2, bottom row first), GLOB.
    fn legacy(pointer: usize, big_endian: bool) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend(b"BLENDER");
        out.push(if pointer == 4 { b'_' } else { b'-' });
        out.push(if big_endian { b'V' } else { b'v' });
        out.extend(b"279");
        let int = |n: i32| if big_endian { n.to_be_bytes() } else { n.to_le_bytes() };
        let block = |out: &mut Vec<u8>, code: &[u8; 4], data: &[u8]| {
            out.extend(code);
            out.extend(int(data.len() as i32));
            out.extend(vec![0xAB; pointer]);
            out.extend(int(0));
            out.extend(int(1));
            out.extend(data);
        };
        block(&mut out, b"REND", &[7; 72]);
        let mut test = Vec::new();
        test.extend(int(2));
        test.extend(int(2));
        test.extend([0, 0, 255, 255, 0, 0, 255, 255]); // bottom row: blue
        test.extend([255, 0, 0, 255, 255, 0, 0, 128]); // top row: red
        block(&mut out, b"TEST", &test);
        block(&mut out, b"GLOB", &[0; 16]);
        out
    }

    fn check(preview: &Preview) {
        assert_eq!((preview.width, preview.height), (2, 2));
        assert_eq!(&preview.rgba[..8], &[255, 0, 0, 255, 255, 0, 0, 128], "top row first");
        assert_eq!(&preview.rgba[8..12], &[0, 0, 255, 255]);
        assert_eq!(preview.format, "Blender 2.79");
    }

    #[test]
    fn reads_the_saved_preview() {
        for (pointer, big_endian) in [(4, false), (8, false), (8, true), (4, true)] {
            check(&read_from(&legacy(pointer, big_endian)[..]).unwrap());
        }
    }

    #[test]
    fn reads_compressed_files() {
        let dir = TempDir::new("blend");
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gzip.write_all(&legacy(8, false)).unwrap();
        let gz = dir.write("gz.blend", &gzip.finish().unwrap());
        check(&read_preview(&gz).unwrap());

        // Split over two frames, as Blender writes it.
        let data = legacy(8, false);
        let mut zstd = ruzstd::encoding::compress_to_vec(&data[..40], ruzstd::encoding::CompressionLevel::Fastest);
        zstd.extend(ruzstd::encoding::compress_to_vec(&data[40..], ruzstd::encoding::CompressionLevel::Fastest));
        let zst = dir.write("zstd.blend", &zstd);
        check(&read_preview(&zst).unwrap());

        let plain = dir.write("plain.blend", &legacy(4, false));
        check(&read_preview(&plain).unwrap());
    }

    #[test]
    fn reads_blender_5_header() {
        let mut out = Vec::new();
        out.extend(b"BLENDER17-01v0500");
        let mut test = Vec::new();
        test.extend(1i32.to_le_bytes());
        test.extend(1i32.to_le_bytes());
        test.extend([1, 2, 3, 4]);
        out.extend(b"TEST");
        out.extend(0i32.to_le_bytes());
        out.extend(0u64.to_le_bytes());
        out.extend((test.len() as i64).to_le_bytes());
        out.extend(1i64.to_le_bytes());
        out.extend(&test);
        let preview = read_from(&out[..]).unwrap();
        assert_eq!((preview.width, preview.height, &preview.rgba[..]), (1, 1, &[1u8, 2, 3, 4][..]));
        assert_eq!(preview.format, "Blender 5.0");
        // A format version we don't know is a clear error, not garbage.
        out[10..12].copy_from_slice(b"02");
        assert!(read_from(&out[..]).err().unwrap().to_string().contains("newer"));
    }

    #[test]
    fn crops_a_small_object_out_of_a_clear_preview() {
        let side = 64;
        let mut rgba = vec![0u8; side * side * 4];
        for (i, pixel) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let (x, y) = (i % side, i / side);
            if x == 0 || y == 0 || x == side - 1 || y == side - 1 {
                *pixel = [0, 0, 0, 96]; // the camera frame: a faint band...
            } else if (x == 2 || y == 1 || x == side - 3 || y == side - 2) && (x + y) % 2 == 0 {
                *pixel = [0, 0, 0, 255]; // ...and a dashed line
            } else if (40..48).contains(&x) && (10..18).contains(&y) {
                *pixel = [200, 50, 50, 255];
            }
        }
        let preview = Preview { width: 64, height: 64, rgba, format: String::new() };
        let (w, h, cropped) = crop_to_content(&preview);
        // At most 3× zoom: a third of the picture.
        assert_eq!((w, h), (21, 21));
        let pixels = cropped.as_chunks::<4>().0;
        assert_eq!(pixels.iter().filter(|p| p[3] == 255).count(), 64, "the whole object");
        assert!(pixels.iter().all(|p| p[3] == 0 || p[3] == 255), "no frame left");
        let center = pixels[10 * 21 + 10];
        assert_eq!(center, [200, 50, 50, 255]);

        // A preview with a background is kept whole.
        let opaque = Preview { width: 16, height: 16, rgba: vec![90; 16 * 16 * 4], format: String::new() };
        assert_eq!(crop_to_content(&opaque).0, 16);
    }

    #[test]
    fn damaged_files_never_panic() {
        for original in [legacy(4, false), legacy(8, true)] {
            for bytes in crate::test_support::damaged(&original, 300) {
                if let Ok(preview) = read_from(&bytes[..]) {
                    let _ = crop_to_content(&preview);
                }
            }
        }
    }

    #[test]
    fn no_preview_or_not_blender() {
        let mut data = legacy(8, false);
        // Rename the TEST block: no preview before GLOB.
        let at = data.windows(4).position(|w| w == b"TEST").unwrap();
        data[at..at + 4].copy_from_slice(b"XXXX");
        assert_eq!(read_from(&data[..]).err().unwrap().to_string(), "this Blender file has no saved preview");
        assert!(read_from(&b"PK\x03\x04 not blender"[..]).is_err());
        assert!(read_from(&data[..20]).is_err());
        assert_eq!(version_name(402), "4.2");
    }
}
