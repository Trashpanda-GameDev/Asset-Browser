//! How previews are kept in the cache file: thumbnails as JPEG or QOI, waveforms as peaks,
//! models' facts beside their pictures, and failures as their message, so a broken file isn't read
//! again on every visit.

use std::sync::Arc;

use image::{ExtendedColorType, ImageEncoder, ImageFormat};
use rusqlite::{Connection, OptionalExtension, params};

use super::{Preview, PreviewError, ThumbJob};
use crate::asset::AssetKind;
use crate::audio::Waveform;
use crate::db::in_transaction;
use crate::diag;
use crate::imaging::Pixels;
use crate::model::ModelInfo;

const FORMAT_FAILED: u8 = 0;
const FORMAT_QOI: u8 = 1;
const FORMAT_JPEG: u8 = 2;
const FORMAT_RAW_RGBA: u8 = 3;

/// A thumbnail as stored: encoded pixels, or the error message when the file couldn't be read.
pub(super) struct StoredThumb {
    w: u32,
    h: u32,
    src_w: u32,
    src_h: u32,
    format: u8,
    data: Vec<u8>,
}

/// A sound's waveform as stored, or the error message when the file couldn't be decoded.
pub(super) struct StoredWaveform {
    duration_ms: u32,
    sample_rate: u32,
    channels: u16,
    /// Min and max per point, as signed bytes.
    peaks: Vec<u8>,
    error: Option<String>,
}

/// One cache entry for the writer thread to save.
pub(super) enum CacheWrite {
    Thumb(i64, StoredThumb),
    Waveform(i64, StoredWaveform),
    /// A model's picture, kept with the image thumbnails, and what's in the file.
    Model(i64, StoredThumb, ModelInfo),
}

fn get_thumb(conn: &Connection, key: i64) -> rusqlite::Result<Option<StoredThumb>> {
    conn.prepare_cached("SELECT w, h, src_w, src_h, format, data FROM thumbs WHERE key = ?1")?
        .query_row([key], |row| {
            Ok(StoredThumb {
                w: row.get(0)?,
                h: row.get(1)?,
                src_w: row.get(2)?,
                src_h: row.get(3)?,
                format: row.get(4)?,
                data: row.get(5)?,
            })
        })
        .optional()
}

fn get_waveform(conn: &Connection, key: i64) -> rusqlite::Result<Option<StoredWaveform>> {
    conn.prepare_cached("SELECT duration_ms, sample_rate, channels, peaks, error FROM waveforms WHERE key = ?1")?
        .query_row([key], |row| {
            Ok(StoredWaveform {
                duration_ms: row.get(0)?,
                sample_rate: row.get(1)?,
                channels: row.get(2)?,
                peaks: row.get(3)?,
                error: row.get(4)?,
            })
        })
        .optional()
}

fn get_model(conn: &Connection, key: i64) -> rusqlite::Result<Option<ModelInfo>> {
    conn.prepare_cached(
        "SELECT format, vertices, triangles, meshes, materials, textures, missing_textures,
                size_x, size_y, size_z, animations, geometry
         FROM models WHERE key = ?1",
    )?
    .query_row([key], |row| {
        Ok(ModelInfo {
            format: row.get(0)?,
            vertices: row.get::<_, i64>(1)? as u64,
            triangles: row.get::<_, i64>(2)? as u64,
            meshes: row.get(3)?,
            materials: row.get(4)?,
            textures: row.get(5)?,
            missing_textures: row.get(6)?,
            size: [row.get::<_, f64>(7)? as f32, row.get::<_, f64>(8)? as f32, row.get::<_, f64>(9)? as f32],
            animations: decode_animations(&row.get::<_, String>(10)?),
            geometry: row.get(11)?,
        })
    })
    .optional()
}

/// One `name<TAB>seconds` line per animation.
fn encode_animations(animations: &[(String, f32)]) -> String {
    animations.iter().map(|(name, seconds)| format!("{}\t{seconds}\n", name.replace(['\t', '\n', '\r'], " "))).collect()
}

fn decode_animations(text: &str) -> Vec<(String, f32)> {
    text.lines()
        .filter_map(|line| {
            let (name, seconds) = line.rsplit_once('\t')?;
            Some((name.to_owned(), seconds.parse().ok()?))
        })
        .collect()
}

/// Saves a batch in one transaction.
pub(super) fn put_cache(conn: &Connection, batch: &[CacheWrite]) -> rusqlite::Result<()> {
    in_transaction(conn, |tx| {
        let mut thumb = tx.prepare_cached(
            "INSERT OR REPLACE INTO thumbs(key, w, h, src_w, src_h, format, data)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        let mut waveform = tx.prepare_cached(
            "INSERT OR REPLACE INTO waveforms(key, duration_ms, sample_rate, channels, peaks, error)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        let mut model = tx.prepare_cached(
            "INSERT OR REPLACE INTO models(key, format, vertices, triangles, meshes, materials, textures,
                                           missing_textures, size_x, size_y, size_z, animations, geometry)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        )?;
        for write in batch {
            match write {
                CacheWrite::Thumb(key, t) => {
                    thumb.execute(params![key, t.w, t.h, t.src_w, t.src_h, t.format, t.data])?;
                }
                CacheWrite::Waveform(key, w) => {
                    waveform.execute(params![key, w.duration_ms, w.sample_rate, w.channels, w.peaks, w.error])?;
                }
                CacheWrite::Model(key, t, info) => {
                    thumb.execute(params![key, t.w, t.h, t.src_w, t.src_h, t.format, t.data])?;
                    let [x, y, z] = info.size.map(f64::from);
                    model.execute(params![
                        key,
                        info.format,
                        info.vertices as i64,
                        info.triangles as i64,
                        info.meshes,
                        info.materials,
                        info.textures,
                        info.missing_textures,
                        x,
                        y,
                        z,
                        encode_animations(&info.animations),
                        info.geometry,
                    ])?;
                }
            }
        }
        Ok(())
    })
}

pub(super) fn cached_preview(conn: &rusqlite::Connection, job: &ThumbJob) -> Option<Result<Preview, PreviewError>> {
    let report = |err: rusqlite::Error| diag::report("Reading the preview cache", err);
    match job.kind {
        AssetKind::Image => get_thumb(conn, job.key)
            .map_err(report)
            .ok()
            .flatten()
            .map(|stored| decode_stored(&stored).map(Preview::Image)),
        AssetKind::Audio => get_waveform(conn, job.key).map_err(report).ok().flatten().map(decode_waveform),
        AssetKind::Model => {
            let stored = get_thumb(conn, job.key).map_err(report).ok().flatten()?;
            if stored.format == FORMAT_FAILED {
                return Some(Err(PreviewError::Stored(String::from_utf8_lossy(&stored.data).into_owned())));
            }
            // A picture without its info is made again.
            let info = get_model(conn, job.key).map_err(report).ok().flatten()?;
            Some(decode_stored(&stored).map(|pixels| Preview::Model(pixels, Arc::new(info))))
        }
    }
}

pub(super) fn to_cache(job: &ThumbJob, result: &Result<Preview, PreviewError>) -> CacheWrite {
    match (job.kind, result) {
        (_, Ok(Preview::Image(pixels))) => CacheWrite::Thumb(job.key, encode_stored(pixels)),
        (_, Ok(Preview::Sound(waveform))) => CacheWrite::Waveform(job.key, encode_waveform(waveform)),
        (_, Ok(Preview::Model(pixels, info))) => CacheWrite::Model(job.key, encode_stored(pixels), (**info).clone()),
        (AssetKind::Audio, Err(error)) => CacheWrite::Waveform(
            job.key,
            StoredWaveform {
                duration_ms: 0,
                sample_rate: 0,
                channels: 0,
                peaks: Vec::new(),
                error: Some(error.to_string()),
            },
        ),
        (_, Err(error)) => CacheWrite::Thumb(
            job.key,
            StoredThumb { w: 0, h: 0, src_w: 0, src_h: 0, format: FORMAT_FAILED, data: error.to_string().into_bytes() },
        ),
    }
}

fn encode_waveform(waveform: &Waveform) -> StoredWaveform {
    StoredWaveform {
        duration_ms: (waveform.duration * 1000.0).round() as u32,
        sample_rate: waveform.sample_rate,
        channels: waveform.channels,
        peaks: waveform.peaks.iter().flat_map(|[low, high]| [*low as u8, *high as u8]).collect(),
        error: None,
    }
}

fn decode_waveform(stored: StoredWaveform) -> Result<Preview, PreviewError> {
    if let Some(error) = stored.error {
        return Err(PreviewError::Stored(error));
    }
    Ok(Preview::Sound(Arc::new(Waveform {
        peaks: stored.peaks.as_chunks::<2>().0.iter().map(|pair| [pair[0] as i8, pair[1] as i8]).collect(),
        duration: stored.duration_ms as f32 / 1000.0,
        sample_rate: stored.sample_rate,
        channels: stored.channels,
    })))
}

/// JPEG for opaque thumbnails (small), QOI when there's transparency (lossless and fast).
fn encode_stored(pixels: &Pixels) -> StoredThumb {
    let Pixels { w, h, src_w, src_h, .. } = *pixels;
    let opaque = pixels.rgba.as_chunks::<4>().0.iter().all(|px| px[3] == 255);
    let mut data = Vec::new();
    let encoded = if opaque {
        let rgb: Vec<u8> = pixels.rgba.as_chunks::<4>().0.iter().flat_map(|px| [px[0], px[1], px[2]]).collect();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut data, 85)
            .encode(&rgb, w, h, ExtendedColorType::Rgb8)
            .map(|()| FORMAT_JPEG)
    } else {
        image::codecs::qoi::QoiEncoder::new(&mut data)
            .write_image(&pixels.rgba, w, h, ExtendedColorType::Rgba8)
            .map(|()| FORMAT_QOI)
    };
    match encoded {
        Ok(format) => StoredThumb { w, h, src_w, src_h, format, data },
        Err(_) => StoredThumb { w, h, src_w, src_h, format: FORMAT_RAW_RGBA, data: pixels.rgba.clone() },
    }
}

fn decode_stored(stored: &StoredThumb) -> Result<Pixels, PreviewError> {
    let rgba = match stored.format {
        FORMAT_FAILED => return Err(PreviewError::Stored(String::from_utf8_lossy(&stored.data).into_owned())),
        FORMAT_RAW_RGBA => stored.data.clone(),
        format => {
            let format = if format == FORMAT_QOI { ImageFormat::Qoi } else { ImageFormat::Jpeg };
            image::load_from_memory_with_format(&stored.data, format)?.into_rgba8().into_raw()
        }
    };
    Ok(Pixels { w: stored.w, h: stored.h, src_w: stored.src_w, src_h: stored.src_h, rgba })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::db;
    use crate::model::ModelError;
    use crate::test_support::TempDir;

    #[test]
    fn model_preview_round_trips_through_the_cache() {
        let dir = TempDir::new("model_cache");
        let conn = db::open_cache(&dir.path().join("library.sqlite")).unwrap();
        let info = ModelInfo {
            format: "FBX 7.4 binary".into(),
            vertices: 1234,
            triangles: 2000,
            meshes: 3,
            materials: 2,
            textures: 1,
            missing_textures: 1,
            size: [1.5, 2.0, 0.25],
            animations: vec![("Idle\tloop".into(), 2.5), ("Run".into(), 0.75)],
            geometry: true,
        };
        let mut rgba = vec![0u8; 4 * 4 * 4];
        rgba[0..4].copy_from_slice(&[10, 20, 30, 255]);
        let pixels = Pixels { w: 4, h: 4, src_w: 0, src_h: 0, rgba };
        let job = ThumbJob { key: 7, path: PathBuf::from("a.fbx"), kind: AssetKind::Model };
        let failed = ThumbJob { key: 8, path: PathBuf::from("b.fbx"), kind: AssetKind::Model };
        let writes = [
            to_cache(&job, &Ok(Preview::Model(pixels, Arc::new(info.clone())))),
            to_cache(&failed, &Err(ModelError::NoMeshes.into())),
        ];
        put_cache(&conn, &writes).unwrap();

        let Some(Ok(Preview::Model(back, back_info))) = cached_preview(&conn, &job) else { panic!("cached model") };
        assert_eq!((back.w, back.h, back.src_w, back.src_h), (4, 4, 0, 0));
        assert_eq!(&back.rgba[0..4], &[10, 20, 30, 255], "QOI keeps pixels exact");
        // Tabs can't survive the text column; they become spaces.
        let mut expected = info;
        expected.animations[0].0 = "Idle loop".into();
        assert_eq!(*back_info, expected);
        let Some(Err(PreviewError::Stored(message))) = cached_preview(&conn, &failed) else { panic!("cached failure") };
        assert_eq!(message, "there are no meshes in this file");
        let unknown = ThumbJob { key: 9, path: PathBuf::from("c.fbx"), kind: AssetKind::Model };
        assert!(cached_preview(&conn, &unknown).is_none());
    }
}
