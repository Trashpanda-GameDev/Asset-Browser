//! One SQLite file holds the folder index, the thumbnail cache and settings.
//!
//! A single file rather than thousands of small thumbnail files: opening many small files is slow
//! on Windows (NTFS plus antivirus scanning on every open). WAL mode lets the thumbnail workers
//! read while the writer thread commits.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};

use crate::asset::AssetKind;
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

/// The per-user cache location for this OS (`%LOCALAPPDATA%` on Windows, `~/Library/Caches` on
/// macOS, `~/.cache` on Linux).
pub fn default_path() -> PathBuf {
    directories::ProjectDirs::from("", "", "AssetBrowser")
        .map(|dirs| dirs.cache_dir().to_path_buf())
        .unwrap_or_else(|| std::env::temp_dir().join("AssetBrowser"))
        .join("library.sqlite")
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

pub fn put_thumbs(conn: &mut Connection, batch: &[(i64, StoredThumb)]) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    {
        let mut insert = tx.prepare_cached(
            "INSERT OR REPLACE INTO thumbs(key, w, h, src_w, src_h, format, data)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        for (key, thumb) in batch {
            insert.execute(params![key, thumb.w, thumb.h, thumb.src_w, thumb.src_h, thumb.format, thumb.data])?;
        }
    }
    tx.commit()
}

pub fn get_setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| row.get(0)).optional().ok().flatten()
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO settings(key, value) VALUES(?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )
    .map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The app's first launch: the UI opens a brand-new database, then the scan thread, the
    /// thumbnail writer and every worker open it at the same moment.
    #[test]
    fn first_launch_opens_from_many_threads() {
        let path = std::env::temp_dir().join(format!("asset_browser_test_{}.sqlite", std::process::id()));
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
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
                    put_thumbs(&mut conn, &[(i, thumb)]).map_err(|err| format!("write {i}: {err}"))
                })
            })
            .collect();
        set_setting(&ui, "last_root", "X").expect("setting");
        let errors: Vec<String> = threads.into_iter().filter_map(|t| t.join().unwrap().err()).collect();
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(get_setting(&ui, "last_root").as_deref(), Some("X"));
        drop(ui);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }
}
