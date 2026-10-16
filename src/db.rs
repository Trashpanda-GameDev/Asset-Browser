//! One SQLite file holds the folder index, the thumbnail, waveform and model caches, and settings.
//!
//! A single file rather than thousands of small thumbnail files: opening many small files is slow
//! on Windows (NTFS plus antivirus scanning on every open). WAL mode lets the thumbnail workers
//! read while the writer thread commits.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};

use crate::asset::AssetKind;
use crate::model::ModelInfo;
use crate::scan::ScannedFile;

pub const FORMAT_FAILED: u8 = 0;
pub const FORMAT_QOI: u8 = 1;
pub const FORMAT_JPEG: u8 = 2;
pub const FORMAT_RAW_RGBA: u8 = 3;

/// A thumbnail as stored: encoded pixels, or the error message when the file couldn't be read.
pub struct StoredThumb {
    pub w: u32,
    pub h: u32,
    pub src_w: u32,
    pub src_h: u32,
    pub format: u8,
    pub data: Vec<u8>,
}

/// A sound's waveform as stored, or the error message when the file couldn't be decoded.
pub struct StoredWaveform {
    pub duration_ms: u32,
    pub sample_rate: u32,
    pub channels: u16,
    /// Min and max per point, as signed bytes.
    pub peaks: Vec<u8>,
    pub error: Option<String>,
}

/// One cache entry for the writer thread to save.
pub enum CacheWrite {
    Thumb(i64, StoredThumb),
    Waveform(i64, StoredWaveform),
    /// A model's picture, kept with the image thumbnails, and what's in the file.
    Model(i64, StoredThumb, ModelInfo),
}

/// Set to a folder, the app keeps everything in it (the cache, the log and the user's own data)
/// instead of in the user's folders. The screenshots tool's demo sets it, so it can't touch the
/// real library.
pub const HOME_VARIABLE: &str = "ASSET_BROWSER_HOME";

fn home(part: &str) -> Option<PathBuf> {
    std::env::var_os(HOME_VARIABLE).filter(|home| !home.is_empty()).map(|home| PathBuf::from(home).join(part))
}

/// The per-user cache location for this OS (`%LOCALAPPDATA%` on Windows, `~/Library/Caches` on
/// macOS, `~/.cache` on Linux).
pub fn default_path() -> PathBuf {
    home("cache")
        .or_else(|| directories::ProjectDirs::from("", "", "AssetBrowser").map(|dirs| dirs.cache_dir().to_path_buf()))
        .unwrap_or_else(|| std::env::temp_dir().join("AssetBrowser"))
        .join("library.sqlite")
}

/// The user's own data, which isn't a cache and must survive one being cleared: favourites,
/// collections, categories (`groups`) and sprite sheet choices (`sheets`). The per-user data
/// folder: `%APPDATA%` on Windows, `~/Library/Application Support` on macOS, `~/.local/share`
/// on Linux.
pub fn user_data_path() -> PathBuf {
    home("data")
        .or_else(|| directories::ProjectDirs::from("", "", "AssetBrowser").map(|dirs| dirs.data_dir().to_path_buf()))
        .unwrap_or_else(|| std::env::temp_dir().join("AssetBrowser"))
        .join("user.sqlite")
}

/// Opens the user data file for the library's folders (the other user data has its own
/// connections: `groups::Groups`, `sheets::SheetStore`).
pub fn open_user(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get::<_, String>(0))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS library_roots(
             position INTEGER PRIMARY KEY,
             path TEXT NOT NULL);",
    )?;
    Ok(conn)
}

/// The folders the library is made of, in the order they were added.
pub fn load_roots(conn: &Connection) -> rusqlite::Result<Vec<PathBuf>> {
    let mut statement = conn.prepare("SELECT path FROM library_roots ORDER BY position")?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    rows.map(|row| row.map(PathBuf::from)).collect()
}

pub fn save_roots(conn: &Connection, roots: &[PathBuf]) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM library_roots", [])?;
    {
        let mut insert = tx.prepare("INSERT INTO library_roots(position, path) VALUES(?1, ?2)")?;
        for (position, root) in roots.iter().enumerate() {
            insert.execute(params![position as i64, root.to_string_lossy()])?;
        }
    }
    tx.commit()
}

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(10))?;
    conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get::<_, String>(0))?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS roots(
             id INTEGER PRIMARY KEY,
             path TEXT NOT NULL UNIQUE,
             scanned_at INTEGER NOT NULL);
         CREATE TABLE IF NOT EXISTS files(
             root INTEGER NOT NULL,
             rel TEXT NOT NULL,
             kind INTEGER NOT NULL,
             size INTEGER NOT NULL,
             mtime INTEGER NOT NULL,
             PRIMARY KEY(root, rel)) WITHOUT ROWID;
         CREATE TABLE IF NOT EXISTS thumbs(
             key INTEGER PRIMARY KEY,
             w INTEGER NOT NULL,
             h INTEGER NOT NULL,
             src_w INTEGER NOT NULL,
             src_h INTEGER NOT NULL,
             format INTEGER NOT NULL,
             data BLOB NOT NULL);
         CREATE TABLE IF NOT EXISTS waveforms(
             key INTEGER PRIMARY KEY,
             duration_ms INTEGER NOT NULL,
             sample_rate INTEGER NOT NULL,
             channels INTEGER NOT NULL,
             peaks BLOB NOT NULL,
             error TEXT);
         CREATE TABLE IF NOT EXISTS models(
             key INTEGER PRIMARY KEY,
             format TEXT NOT NULL,
             vertices INTEGER NOT NULL,
             triangles INTEGER NOT NULL,
             meshes INTEGER NOT NULL,
             materials INTEGER NOT NULL,
             textures INTEGER NOT NULL,
             missing_textures INTEGER NOT NULL,
             size_x REAL NOT NULL,
             size_y REAL NOT NULL,
             size_z REAL NOT NULL,
             animations TEXT NOT NULL,
             geometry INTEGER NOT NULL);
         CREATE TABLE IF NOT EXISTS settings(
             key TEXT PRIMARY KEY,
             value TEXT NOT NULL);",
    )?;
    Ok(conn)
}

/// The files found by the last scan of `root`, or `None` if it was never scanned.
pub fn load_index(conn: &Connection, root: &str) -> rusqlite::Result<Option<Vec<ScannedFile>>> {
    let id: Option<i64> =
        conn.query_row("SELECT id FROM roots WHERE path = ?1", [root], |row| row.get(0)).optional()?;
    let Some(id) = id else { return Ok(None) };
    let mut statement = conn.prepare("SELECT rel, kind, size, mtime FROM files WHERE root = ?1")?;
    let rows = statement.query_map([id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?, row.get::<_, i64>(3)?))
    })?;
    let mut files = Vec::new();
    for row in rows {
        let (rel, kind, size, mtime) = row?;
        if let Some(kind) = AssetKind::from_db(kind) {
            files.push(ScannedFile { rel, kind, size: size as u64, mtime });
        }
    }
    Ok(Some(files))
}

pub fn save_index(conn: &mut Connection, root: &str, files: &[ScannedFile]) -> rusqlite::Result<()> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO roots(path, scanned_at) VALUES(?1, ?2)
         ON CONFLICT(path) DO UPDATE SET scanned_at = excluded.scanned_at",
        params![root, now],
    )?;
    let id: i64 = tx.query_row("SELECT id FROM roots WHERE path = ?1", [root], |row| row.get(0))?;
    tx.execute("DELETE FROM files WHERE root = ?1", [id])?;
    {
        let mut insert = tx.prepare("INSERT INTO files(root, rel, kind, size, mtime) VALUES(?1, ?2, ?3, ?4, ?5)")?;
        for file in files {
            insert.execute(params![id, file.rel, file.kind.to_db(), file.size as i64, file.mtime])?;
        }
    }
    tx.commit()
}

pub fn get_thumb(conn: &Connection, key: i64) -> rusqlite::Result<Option<StoredThumb>> {
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

pub fn get_waveform(conn: &Connection, key: i64) -> rusqlite::Result<Option<StoredWaveform>> {
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

pub fn get_model(conn: &Connection, key: i64) -> rusqlite::Result<Option<ModelInfo>> {
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
pub fn put_cache(conn: &mut Connection, batch: &[CacheWrite]) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    {
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
    }
    tx.commit()
}

pub fn get_setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| row.get(0)).optional().ok().flatten()
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    set_settings(conn, &[(key, value)])
}

pub fn remove_setting(conn: &Connection, key: &str) -> rusqlite::Result<()> {
    remove_settings(conn, &[key])
}

/// Removes several settings in one transaction.
pub fn remove_settings(conn: &Connection, keys: &[&str]) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    {
        let mut statement = tx.prepare_cached("DELETE FROM settings WHERE key = ?1")?;
        for key in keys {
            statement.execute([key])?;
        }
    }
    tx.commit()
}

/// Saves several settings in one transaction.
pub fn set_settings(conn: &Connection, settings: &[(&str, &str)]) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    {
        let mut statement = tx.prepare_cached(
            "INSERT INTO settings(key, value) VALUES(?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )?;
        for (key, value) in settings {
            statement.execute(params![key, value])?;
        }
    }
    tx.commit()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    /// The app's first launch: the UI opens a brand-new database, then the scan thread, the
    /// thumbnail writer and every worker open it at the same moment.
    #[test]
    fn first_launch_opens_from_many_threads() {
        let dir = TempDir::new("first_launch");
        let path = dir.path().join("library.sqlite");
        let ui = open(&path).expect("UI connection");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(21));
        let threads: Vec<_> = (0..21)
            .map(|i| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    let mut conn = open(&path).map_err(|err| format!("open {i}: {err}"))?;
                    let thumb =
                        StoredThumb { w: 1, h: 1, src_w: 1, src_h: 1, format: FORMAT_RAW_RGBA, data: vec![0; 4] };
                    put_cache(&mut conn, &[CacheWrite::Thumb(i, thumb)]).map_err(|err| format!("write {i}: {err}"))
                })
            })
            .collect();
        set_setting(&ui, "last_root", "X").expect("setting");
        let errors: Vec<String> = threads.into_iter().filter_map(|t| t.join().unwrap().err()).collect();
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(get_setting(&ui, "last_root").as_deref(), Some("X"));
        remove_setting(&ui, "last_root").expect("remove setting");
        assert_eq!(get_setting(&ui, "last_root"), None);
    }

    #[test]
    fn library_folders_keep_their_order() {
        let dir = TempDir::new("roots");
        let path = dir.path().join("user.sqlite");
        let conn = open_user(&path).expect("user data");
        assert!(load_roots(&conn).unwrap().is_empty());
        let roots = [PathBuf::from("F:\\Resources"), PathBuf::from("D:\\Audio"), PathBuf::from("C:\\Art")];
        save_roots(&conn, &roots).unwrap();
        save_roots(&conn, &roots[1..]).unwrap();
        drop(conn);
        let conn = open_user(&path).expect("user data again");
        assert_eq!(load_roots(&conn).unwrap(), roots[1..], "saving replaces the list");
    }
}
