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

pub struct Sampled {
    pub w: u32,
    pub h: u32,
    pub src_w: u32,
    pub src_h: u32,
    pub rgba: Vec<u8>,
}

/// Reads the picture at no less than `2 * max` pixels on its longest side (or its full size if
/// smaller), so the final resize still has detail to average.
pub fn read(path: &Path, max: u32) -> Result<Sampled, String> {
    read_inner(path, max).map_err(|err| format!("PSD: {err}"))
}

#[expect(clippy::cognitive_complexity, clippy::too_many_lines, reason = "refactor: split into named steps")]
fn read_inner(path: &Path, max: u32) -> Result<Sampled, String> {
    let file = File::open(path).map_err(|err| err.to_string())?;
    let mut r = Reader { inner: BufReader::with_capacity(64 * 1024, file), pos: 0 };

    let header = r.bytes(26)?;
    if &header[0..4] != b"8BPS" {
        return Err("not a Photoshop file".into());
    }
    let big = match be16(&header[4..]) {
        1 => false,
        2 => true, // PSB, the large document format
        version => return Err(format!("unknown version {version}")),
    };
    let channels = be16(&header[12..]) as usize;
    let (h, w) = (be32(&header[14..]), be32(&header[18..]));
    let depth = be16(&header[22..]);
    let mode = be16(&header[24..]);
    if w == 0 || h == 0 || channels == 0 {
        return Err("empty image".into());
    }
    let bytes_per_sample = match depth {
        1 => 0, // bits, see `sample_row`
        8 => 1,
        16 => 2,
        32 => 4,
        _ => return Err(format!("unsupported bit depth {depth}")),
    };

    let color_data_len = r.u32()? as u64;
    let palette = if mode == 2 {
        r.bytes(color_data_len as usize)?
    } else {
        r.skip(color_data_len)?;
        Vec::new()
    };
    let resources_len = r.u32()? as u64;
    r.skip(resources_len)?;

    // A negative layer count means the first extra channel of the flattened picture is its
    // transparency. Otherwise extra channels are saved selections, not transparency.
    let layers_len = if big { r.u64()? } else { r.u32()? as u64 };
    let layers_end = r.pos + layers_len;
    let mut merged_alpha = false;
    if layers_len > 0 {
        let info_len = if big { r.u64()? } else { r.u32()? as u64 };
        if info_len >= 2 {
            merged_alpha = (r.u16()? as i16) < 0;
        }
    }
    r.seek_to(layers_end)?;
    let compression = r.u16()?;

    let (color_channels, alpha) = match mode {
        0 | 1 | 7 | 8 => (1, merged_alpha && channels >= 2), // bitmap, grey, multichannel, duotone
        2 => (1, false),                                     // indexed
        3 => (3, merged_alpha && channels >= 4),             // RGB
        4 => (4, merged_alpha && channels >= 5),             // CMYK
        _ => return Err(format!("unsupported colour mode {mode}")),
    };
    if channels < color_channels {
        return Err("too few channels for its colour mode".into());
    }
    let needed: Vec<usize> = (0..color_channels).chain(alpha.then_some(color_channels)).collect();

    let step = (w.max(h) / (2 * max.max(1))).max(1);
    let (out_w, out_h) = (w.div_ceil(step), h.div_ceil(step));
    let row_bytes = if depth == 1 { (w as usize).div_ceil(8) } else { w as usize * bytes_per_sample };
    let plane_len = out_w as usize * out_h as usize;
    let mut planes = vec![vec![0u8; plane_len]; needed.len()];
    let mut row = vec![0u8; row_bytes];

    match compression {
        0 => {
            let data_start = r.pos;
            for (plane, &channel) in planes.iter_mut().zip(&needed) {
                for out_y in 0..out_h as usize {
                    let y = out_y as u64 * step as u64;
                    r.seek_to(data_start + (channel as u64 * h as u64 + y) * row_bytes as u64)?;
                    r.inner.read_exact(&mut row).map_err(|err| err.to_string())?;
                    r.pos += row_bytes as u64;
                    sample_row(&row, depth, step, &mut plane[out_y * out_w as usize..][..out_w as usize]);
                }
            }
        }
        1 => {
            // Every row's packed length, for all channels, before any pixel data.
            let rows = channels * h as usize;
            let width = if big { 4 } else { 2 };
            let table = r.bytes(rows * width)?;
            let mut offsets = Vec::with_capacity(rows + 1);
            let mut total = 0u64;
            offsets.push(0);
            for entry in table.chunks_exact(width) {
                total += if big { be32(entry) as u64 } else { be16(entry) as u64 };
                offsets.push(total);
            }
            let data_start = r.pos;
            let mut packed = Vec::new();
            for (plane, &channel) in planes.iter_mut().zip(&needed) {
                for out_y in 0..out_h as usize {
                    let index = channel * h as usize + out_y * step as usize;
                    let (start, end) = (offsets[index], offsets[index + 1]);
                    r.seek_to(data_start + start)?;
                    packed.resize((end - start) as usize, 0);
                    r.inner.read_exact(&mut packed).map_err(|err| err.to_string())?;
                    r.pos += end - start;
                    unpack_bits(&packed, &mut row)?;
                    sample_row(&row, depth, step, &mut plane[out_y * out_w as usize..][..out_w as usize]);
                }
            }
        }
        2 | 3 => return Err("ZIP-compressed picture data isn't supported".into()),
        other => return Err(format!("unknown compression {other}")),
    }

    let mut rgba = vec![0u8; plane_len * 4];
    for (i, px) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (red, green, blue) = match mode {
            3 => (planes[0][i], planes[1][i], planes[2][i]),
            // Stored inverted: 255 is no ink.
            4 => {
                let k = planes[3][i] as u32;
                let ink = |c: u8| (c as u32 * k / 255) as u8;
                (ink(planes[0][i]), ink(planes[1][i]), ink(planes[2][i]))
            }
            // The palette is 256 reds, then 256 greens, then 256 blues.
            2 => {
                let index = planes[0][i] as usize;
                let at = |offset: usize| palette.get(offset + index).copied().unwrap_or(0);
                (at(0), at(256), at(512))
            }
            _ => (planes[0][i], planes[0][i], planes[0][i]),
        };
        let a = if alpha { planes[needed.len() - 1][i] } else { 255 };
        // Photoshop blends the flattened picture's semi-transparent pixels with white; undo it.
        let unmatte = |c: u8| -> u8 {
            if a == 0 || a == 255 { c } else { ((c as i32 - (255 - a as i32)) * 255 / a as i32).clamp(0, 255) as u8 }
        };
        px.copy_from_slice(&[unmatte(red), unmatte(green), unmatte(blue), a]);
    }
    Ok(Sampled { w: out_w, h: out_h, src_w: w, src_h: h, rgba })
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
}

impl Reader {
    fn bytes(&mut self, n: usize) -> Result<Vec<u8>, String> {
        let mut buf = vec![0; n];
        self.inner.read_exact(&mut buf).map_err(|err| err.to_string())?;
        self.pos += n as u64;
        Ok(buf)
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
        self.seek_to(self.pos + n)
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
