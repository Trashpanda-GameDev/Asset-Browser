//! Reads the flattened picture that Photoshop stores at the end of a PSD or PSB, at reduced size.
//!
//! The `psd` crate decodes every layer before it can give the flattened picture. Measured on
//! F:\Resources (`bench --only .psd`, 300 files): 10 thumbnails/s, single files up to 15 s, and a
//! 2 GB memory peak. A thumbnail only needs the flattened picture, and with RLE compression the
//! file lists every row's length up front, so only the rows that survive downsampling are read
//! and decoded. The layers, usually most of the file, are skipped without being read.
//!
//! Layout (Adobe's "Photoshop File Formats" spec): header, colour mode data, image resources,
//! layer and mask information, then the image data: planar channels, raw or PackBits RLE.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use super::Pixels;

/// Photoshop's own limits: 30,000 pixels a side for a PSD, 300,000 for a PSB, and 56 channels. A
/// header past them is damaged, and believing it could ask for gigabytes.
const MAX_SIDE_PSD: u32 = 30_000;
const MAX_SIDE_PSB: u32 = 300_000;
const MAX_CHANNELS: usize = 56;

/// Reads the picture at no less than `2 * max` pixels on its longest side (or its full size if
/// smaller), so the final resize still has detail to average.
pub fn read(path: &Path, max: u32) -> Result<Pixels, String> {
    read_inner(path, max).map_err(|err| format!("PSD: {err}"))
}

fn read_inner(path: &Path, max: u32) -> Result<Pixels, String> {
    let file = File::open(path).map_err(|err| err.to_string())?;
    let len = file.metadata().map_err(|err| err.to_string())?.len();
    let mut r = Reader { inner: BufReader::with_capacity(64 * 1024, file), pos: 0, len };
    let header = Header::read(&mut r)?;
    let palette = read_color_data(&mut r, header.mode)?;
    let resources_len = u64::from(r.u32()?);
    r.skip(resources_len)?;
    let merged_alpha = read_merged_alpha(&mut r, header.big)?;
    let compression = r.u16()?;
    let layout = Layout::new(&header, merged_alpha, max)?;
    let planes = match compression {
        0 => read_raw(&mut r, &header, &layout)?,
        1 => read_rle(&mut r, &header, &layout)?,
        2 | 3 => return Err("ZIP-compressed picture data isn't supported".into()),
        other => return Err(format!("unknown compression {other}")),
    };
    Ok(compose(&header, &layout, &planes, &palette))
}

/// What the file's header says.
struct Header {
    /// PSB, the large document format: some lengths take 8 bytes instead of 4.
    big: bool,
    channels: usize,
    w: u32,
    h: u32,
    depth: u16,
    mode: u16,
}

impl Header {
    fn read(r: &mut Reader) -> Result<Self, String> {
        let header = r.bytes(26)?;
        if &header[0..4] != b"8BPS" {
            return Err("not a Photoshop file".into());
        }
        let big = match be16(&header[4..]) {
            1 => false,
            2 => true,
            version => return Err(format!("unknown version {version}")),
        };
        let channels = be16(&header[12..]) as usize;
        let (h, w) = (be32(&header[14..]), be32(&header[18..]));
        let (depth, mode) = (be16(&header[22..]), be16(&header[24..]));
        if w == 0 || h == 0 || channels == 0 {
            return Err("empty image".into());
        }
        let max_side = if big { MAX_SIDE_PSB } else { MAX_SIDE_PSD };
        if w > max_side || h > max_side || channels > MAX_CHANNELS {
            return Err("its header says it's bigger than Photoshop allows".into());
        }
        if !matches!(depth, 1 | 8 | 16 | 32) {
            return Err(format!("unsupported bit depth {depth}"));
        }
        Ok(Self { big, channels, w, h, depth, mode })
    }

    /// Bytes in one row of one channel, as stored once decoded.
    fn row_bytes(&self) -> usize {
        match self.depth {
            1 => (self.w as usize).div_ceil(8),
            depth => self.w as usize * usize::from(depth / 8),
        }
    }
}

/// Which channels are read, and how much of them.
struct Layout {
    /// The colour channels, then the transparency when there is one.
    needed: Vec<usize>,
    alpha: bool,
    /// Every `step`th pixel of every `step`th row is read.
    step: u32,
    out_w: u32,
    out_h: u32,
}

impl Layout {
    fn new(header: &Header, merged_alpha: bool, max: u32) -> Result<Self, String> {
        let (color_channels, alpha) = match header.mode {
            0 | 1 | 7 | 8 => (1, merged_alpha && header.channels >= 2), // bitmap, grey, multichannel, duotone
            2 => (1, false),                                            // indexed
            3 => (3, merged_alpha && header.channels >= 4),             // RGB
            4 => (4, merged_alpha && header.channels >= 5),             // CMYK
            mode => return Err(format!("unsupported colour mode {mode}")),
        };
        if header.channels < color_channels {
            return Err("too few channels for its colour mode".into());
        }
        let needed = (0..color_channels).chain(alpha.then_some(color_channels)).collect();
        let step = (header.w.max(header.h) / (2 * max.max(1))).max(1);
        Ok(Self { needed, alpha, step, out_w: header.w.div_ceil(step), out_h: header.h.div_ceil(step) })
    }

    fn plane_len(&self) -> usize {
        self.out_w as usize * self.out_h as usize
    }

    /// Where row `out_y` of a plane goes.
    fn plane_row<'a>(&self, plane: &'a mut [u8], out_y: usize) -> &'a mut [u8] {
        &mut plane[out_y * self.out_w as usize..][..self.out_w as usize]
    }
}

/// The palette of an indexed picture: 256 reds, then 256 greens, then 256 blues. Other modes'
/// colour data is skipped.
fn read_color_data(r: &mut Reader, mode: u16) -> Result<Vec<u8>, String> {
    let len = u64::from(r.u32()?);
    if mode == 2 {
        r.bytes(len)
    } else {
        r.skip(len)?;
        Ok(Vec::new())
    }
}

/// Whether the flattened picture's first extra channel is its transparency: the layer count is
/// negative when it is; otherwise extra channels are saved selections. Leaves `r` past the layers.
fn read_merged_alpha(r: &mut Reader, big: bool) -> Result<bool, String> {
    let layers_len = if big { r.u64()? } else { u64::from(r.u32()?) };
    let layers_end = r.pos.saturating_add(layers_len);
    let mut merged_alpha = false;
    if layers_len > 0 {
        let info_len = if big { r.u64()? } else { u64::from(r.u32()?) };
        if info_len >= 2 {
            merged_alpha = (r.u16()? as i16) < 0;
        }
    }
    r.seek_to(layers_end)?;
    Ok(merged_alpha)
}

/// Uncompressed: each channel's rows one after another, so each row wanted is read where it is.
fn read_raw(r: &mut Reader, header: &Header, layout: &Layout) -> Result<Vec<Vec<u8>>, String> {
    let row_bytes = header.row_bytes();
    let mut planes = vec![vec![0u8; layout.plane_len()]; layout.needed.len()];
    let mut row = vec![0u8; row_bytes];
    let data_start = r.pos;
    for (plane, &channel) in planes.iter_mut().zip(&layout.needed) {
        for out_y in 0..layout.out_h as usize {
            let y = out_y as u64 * u64::from(layout.step);
            r.seek_to(data_start + (channel as u64 * u64::from(header.h) + y) * row_bytes as u64)?;
            r.read_exact(&mut row)?;
            sample_row(&row, header.depth, layout.step, layout.plane_row(plane, out_y));
        }
    }
    Ok(planes)
}

/// PackBits: every row's packed length first, for all the channels, then the rows.
fn read_rle(r: &mut Reader, header: &Header, layout: &Layout) -> Result<Vec<Vec<u8>>, String> {
    let offsets = read_row_table(r, header)?;
    let mut planes = vec![vec![0u8; layout.plane_len()]; layout.needed.len()];
    let mut row = vec![0u8; header.row_bytes()];
    let mut packed = Vec::new();
    let data_start = r.pos;
    for (plane, &channel) in planes.iter_mut().zip(&layout.needed) {
        for out_y in 0..layout.out_h as usize {
            let index = channel * header.h as usize + out_y * layout.step as usize;
            let (start, end) = (offsets[index], offsets[index + 1]);
            r.seek_to(data_start + start)?;
            r.read_to(&mut packed, end - start)?;
            unpack_bits(&packed, &mut row)?;
            sample_row(&row, header.depth, layout.step, layout.plane_row(plane, out_y));
        }
    }
    Ok(planes)
}

/// Where each row's packed data starts, from the first row's, and where the last one ends.
fn read_row_table(r: &mut Reader, header: &Header) -> Result<Vec<u64>, String> {
    let rows = header.channels * header.h as usize;
    let width = if header.big { 4 } else { 2 };
    let table = r.bytes((rows * width) as u64)?;
    let mut offsets = Vec::with_capacity(rows + 1);
    let mut total = 0u64;
    offsets.push(0);
    for entry in table.chunks_exact(width) {
        total += if header.big { u64::from(be32(entry)) } else { u64::from(be16(entry)) };
        offsets.push(total);
    }
    Ok(offsets)
}

/// The channels as straight RGBA: CMYK inked, indexed colours looked up, and the white Photoshop
/// blends into semi-transparent pixels taken out again.
fn compose(header: &Header, layout: &Layout, planes: &[Vec<u8>], palette: &[u8]) -> Pixels {
    let mut rgba = vec![0u8; layout.plane_len() * 4];
    for (i, px) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (red, green, blue) = match header.mode {
            3 => (planes[0][i], planes[1][i], planes[2][i]),
            // Stored inverted: 255 is no ink.
            4 => {
                let k = planes[3][i] as u32;
                let ink = |c: u8| (c as u32 * k / 255) as u8;
                (ink(planes[0][i]), ink(planes[1][i]), ink(planes[2][i]))
            }
            2 => {
                let index = planes[0][i] as usize;
                let at = |offset: usize| palette.get(offset + index).copied().unwrap_or(0);
                (at(0), at(256), at(512))
            }
            _ => (planes[0][i], planes[0][i], planes[0][i]),
        };
        let a = if layout.alpha { planes[layout.needed.len() - 1][i] } else { 255 };
        let unmatte = |c: u8| -> u8 {
            if a == 0 || a == 255 { c } else { ((c as i32 - (255 - a as i32)) * 255 / a as i32).clamp(0, 255) as u8 }
        };
        px.copy_from_slice(&[unmatte(red), unmatte(green), unmatte(blue), a]);
    }
    Pixels { w: layout.out_w, h: layout.out_h, src_w: header.w, src_h: header.h, rgba }
}

/// Picks every `step`th sample of a decoded row, converted to 8 bits.
fn sample_row(row: &[u8], depth: u16, step: u32, out: &mut [u8]) {
    for (out_x, value) in out.iter_mut().enumerate() {
        let x = out_x * step as usize;
        *value = match depth {
            1 => {
                // Bitmap mode: a set bit is black.
                if (row[x / 8] >> (7 - x % 8)) & 1 == 1 { 0 } else { 255 }
            }
            8 => row[x],
            16 => row[x * 2], // big-endian: the high byte comes first
            _ => {
                // 32-bit documents are linear floating point; convert to sRGB-ish for display.
                let f = f32::from_be_bytes([row[x * 4], row[x * 4 + 1], row[x * 4 + 2], row[x * 4 + 3]]);
                (f.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0 + 0.5) as u8
            }
        };
    }
}

/// PackBits: a signed count byte, then either count+1 literal bytes or one byte repeated
/// 1-count times.
fn unpack_bits(src: &[u8], dst: &mut [u8]) -> Result<(), String> {
    let (mut i, mut o) = (0, 0);
    while o < dst.len() {
        let Some(&count) = src.get(i) else { return Err("row data ends early".into()) };
        i += 1;
        let count = count as i8;
        if count >= 0 {
            let n = count as usize + 1;
            let literal = src.get(i..i + n).ok_or("row data ends early")?;
            let end = (o + n).min(dst.len());
            dst[o..end].copy_from_slice(&literal[..end - o]);
            i += n;
            o += n;
        } else if count != -128 {
            let n = (1 - count as isize) as usize;
            let value = *src.get(i).ok_or("row data ends early")?;
            i += 1;
            let end = (o + n).min(dst.len());
            dst[o..end].fill(value);
            o += n;
        }
    }
    Ok(())
}

/// A buffered reader that knows its position, so forward jumps within the buffer don't discard
/// it (`seek_relative`).
struct Reader {
    inner: BufReader<File>,
    pos: u64,
    /// The file's length: nothing is read, or made room for, past it.
    len: u64,
}

impl Reader {
    fn bytes(&mut self, n: u64) -> Result<Vec<u8>, String> {
        let mut buf = Vec::new();
        self.read_to(&mut buf, n)?;
        Ok(buf)
    }

    /// Reads `n` bytes into `buf`, once it's known the file has them.
    fn read_to(&mut self, buf: &mut Vec<u8>, n: u64) -> Result<(), String> {
        if self.pos.saturating_add(n) > self.len {
            return Err("the file ends before its data does".into());
        }
        buf.resize(n as usize, 0);
        self.read_exact(buf)
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), String> {
        self.inner.read_exact(buf).map_err(|err| err.to_string())?;
        self.pos += buf.len() as u64;
        Ok(())
    }

    fn u16(&mut self) -> Result<u16, String> {
        Ok(be16(&self.bytes(2)?))
    }

    fn u32(&mut self) -> Result<u32, String> {
        Ok(be32(&self.bytes(4)?))
    }

    fn u64(&mut self) -> Result<u64, String> {
        let b = self.bytes(8)?;
        Ok(u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
    }

    fn skip(&mut self, n: u64) -> Result<(), String> {
        self.seek_to(self.pos.saturating_add(n))
    }

    fn seek_to(&mut self, target: u64) -> Result<(), String> {
        if target == self.pos {
            return Ok(());
        }
        let delta = target as i128 - self.pos as i128;
        if let Ok(delta) = i64::try_from(delta) {
            self.inner.seek_relative(delta).map_err(|err| err.to_string())?;
        } else {
            self.inner.seek(SeekFrom::Start(target)).map_err(|err| err.to_string())?;
        }
        self.pos = target;
        Ok(())
    }
}

fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    /// The colour of pixel (x, y) in the test pictures.
    fn color(x: u32, y: u32) -> [u8; 3] {
        [(x * 40) as u8, (y * 60) as u8, 128]
    }

    /// A `w` × `h` RGB PSD at 8 bits, its pixels `color(x, y)`: raw, or PackBits-compressed with
    /// every row one literal run.
    fn psd(w: u32, h: u32, rle: bool) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"8BPS");
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&[0; 6]);
        out.extend_from_slice(&3u16.to_be_bytes());
        out.extend_from_slice(&h.to_be_bytes());
        out.extend_from_slice(&w.to_be_bytes());
        out.extend_from_slice(&8u16.to_be_bytes());
        out.extend_from_slice(&3u16.to_be_bytes());
        // No colour mode data, image resources or layers.
        out.extend_from_slice(&[0; 12]);
        out.extend_from_slice(&u16::from(rle).to_be_bytes());
        let row = |channel: usize, y: u32| (0..w).map(move |x| color(x, y)[channel]);
        if rle {
            for _ in 0..3 * h {
                out.extend_from_slice(&(w as u16 + 1).to_be_bytes());
            }
            for channel in 0..3 {
                for y in 0..h {
                    out.push(w as u8 - 1);
                    out.extend(row(channel, y));
                }
            }
        } else {
            for channel in 0..3 {
                for y in 0..h {
                    out.extend(row(channel, y));
                }
            }
        }
        out
    }

    #[test]
    fn reads_the_flattened_picture_raw_or_packed() {
        let dir = TempDir::new("psd");
        for rle in [false, true] {
            let path = dir.write(&format!("picture_{rle}.psd"), &psd(5, 4, rle));
            let pixels = read(&path, 64).unwrap();
            assert_eq!((pixels.w, pixels.h, pixels.src_w, pixels.src_h), (5, 4, 5, 4));
            for (i, px) in pixels.rgba.as_chunks::<4>().0.iter().enumerate() {
                let (x, y) = (i as u32 % 5, i as u32 / 5);
                assert_eq!(px[..3], color(x, y), "pixel {x}, {y} (rle {rle})");
                assert_eq!(px[3], 255, "no transparency without merged alpha");
            }
        }
    }

    #[test]
    fn a_big_picture_is_read_at_every_nth_pixel() {
        let dir = TempDir::new("psd_shrunk");
        // At most 2 × `max` pixels wide: every 2nd pixel of 6 is 3.
        let pixels = read(&dir.write("big.psd", &psd(6, 4, true)), 1).unwrap();
        assert_eq!((pixels.w, pixels.h, pixels.src_w, pixels.src_h), (2, 2, 6, 4));
        let first_row: Vec<[u8; 3]> =
            pixels.rgba.as_chunks::<4>().0[..2].iter().map(|px| [px[0], px[1], px[2]]).collect();
        assert_eq!(first_row, [color(0, 0), color(3, 0)]);
    }

    #[test]
    fn damaged_files_never_panic() {
        let dir = TempDir::new("psd_damaged");
        for rle in [false, true] {
            for bytes in crate::test_support::damaged(&psd(9, 7, rle), 100) {
                let _ = read(&dir.write("damaged.psd", &bytes), 4);
            }
        }
    }

    /// A damaged header could otherwise ask for gigabytes, and running out of memory ends the app.
    #[test]
    fn a_header_bigger_than_photoshop_allows_is_refused() {
        let dir = TempDir::new("psd_huge");
        let mut huge = psd(2, 2, false);
        huge[14..22].copy_from_slice(&[0xff; 8]);
        let Err(error) = read(&dir.write("huge.psd", &huge), 64) else { panic!("read a huge header") };
        assert!(error.contains("bigger than Photoshop allows"), "{error}");
        let mut palette = psd(2, 2, false);
        palette[25] = 2;
        palette[26..30].copy_from_slice(&[0xff; 4]);
        let Err(error) = read(&dir.write("palette.psd", &palette), 64) else { panic!("read a huge palette") };
        assert!(error.contains("ends before its data"), "{error}");
    }

    #[test]
    fn other_files_are_errors() {
        let dir = TempDir::new("psd_wrong");
        let not_psd = dir.write("picture.psd", b"\x89PNG\r\n\x1a\n not a photoshop file");
        assert_eq!(read(&not_psd, 64).err().as_deref(), Some("PSD: not a Photoshop file"));
        let mut zip = psd(2, 2, false);
        zip[38] = 0;
        zip[39] = 2;
        let zip = dir.write("zip.psd", &zip);
        assert!(read(&zip, 64).err().is_some_and(|err| err.contains("ZIP")));
    }
}
