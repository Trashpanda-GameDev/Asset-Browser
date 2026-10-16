//! One SQLite file holds the folder index, the thumbnail, waveform and model caches, and settings.
//!
//! A single file rather than thousands of small thumbnail files: opening many small files is slow
//! on Windows (NTFS plus antivirus scanning on every open). WAL mode lets the thumbnail workers
//! read while the writer thread commits.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::asset::AssetKind;
use crate::scan::ScannedFile;

/// How much a file's last writes are worth after a crash or a power cut.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Durability {
    /// Everything in the cache can be made again, so a write returns before it's on disk
    /// (`synchronous = NORMAL`): a power cut can lose the last moments, never the file.
    Cache,
    /// The user's own data: a write is on disk before it returns.
    UserData,
}

/// Opens a SQLite file the way the app opens every one: its folder made first, in WAL mode so
/// readers carry on while one connection writes, waiting up to 10 s for another connection's write.
pub fn connect(path: &Path, durability: Durability) -> rusqlite::Result<Connection> {
    if let Some(dir) = path.parent() {
        // Not reported here: opening the file fails right after if its folder isn't there.
        let _ = std::fs::create_dir_all(dir);
    }
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(10))?;
    conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get::<_, String>(0))?;
    if durability == Durability::Cache {
        conn.pragma_update(None, "synchronous", "NORMAL")?;
    }
    Ok(conn)
}

/// Runs `work` as one transaction: everything it writes is saved, or none of it is.
pub fn in_transaction<T>(
    conn: &Connection,
    work: impl FnOnce(&Transaction<'_>) -> rusqlite::Result<T>,
) -> rusqlite::Result<T> {
    let tx = conn.unchecked_transaction()?;
    let value = work(&tx)?;
    tx.commit()?;
    Ok(value)
}

/// The user data file's tables: the library's folders, favorites, collections and categories
/// (`groups`), and sprite sheet choices (`sheets`). They're all made here, with the file's
/// migrations, so its version (`user_version`) covers all of them whichever part opens it first.
const USER_DATA_TABLES: &str = "
    CREATE TABLE IF NOT EXISTS library_roots(
        position INTEGER PRIMARY KEY,
        path TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS groups(
        id INTEGER PRIMARY KEY,
        kind INTEGER NOT NULL,
        name TEXT NOT NULL,
        color INTEGER NOT NULL,
        created INTEGER NOT NULL);
    CREATE TABLE IF NOT EXISTS members(
        grp INTEGER NOT NULL,
        key TEXT NOT NULL,
        path TEXT NOT NULL,
        seq INTEGER NOT NULL,
        added INTEGER NOT NULL,
        PRIMARY KEY(grp, key)) WITHOUT ROWID;
    CREATE TABLE IF NOT EXISTS sheets(
        key TEXT PRIMARY KEY,
        animate INTEGER NOT NULL,
        columns INTEGER,
        rows INTEGER,
        margin INTEGER,
        spacing INTEGER,
        split INTEGER,
        fps REAL NOT NULL) WITHOUT ROWID;";

/// Opens the user's own data file with all its tables, brought up to the current version. The
/// app keeps a connection to it for the library's folders; `groups::Groups` and
/// `sheets::SheetStore` keep their own.
pub fn open_user_data(path: &Path) -> rusqlite::Result<Connection> {
    let conn = connect(path, Durability::UserData)?;
    conn.execute_batch(USER_DATA_TABLES)?;
    migrate_user_data(&conn)?;
    Ok(conn)
}

/// Version 1 stores a sprite sheet's default speed as 0, so changing the default reaches every
/// picture. Before, the default (12 fps) was saved as a number: those become the default.
fn migrate_user_data(conn: &Connection) -> rusqlite::Result<()> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version < 1 {
        in_transaction(conn, |tx| {
            tx.execute("UPDATE sheets SET fps = 0 WHERE fps = 12", [])?;
            tx.pragma_update(None, "user_version", 1)
        })?;
    }
    Ok(())
}

/// The folders the library is made of, in the order they were added.
pub fn load_roots(conn: &Connection) -> rusqlite::Result<Vec<PathBuf>> {
    let mut statement = conn.prepare("SELECT path FROM library_roots ORDER BY position")?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    rows.map(|row| row.map(PathBuf::from)).collect()
}

pub fn save_roots(conn: &Connection, roots: &[PathBuf]) -> rusqlite::Result<()> {
    in_transaction(conn, |tx| {
        tx.execute("DELETE FROM library_roots", [])?;
        let mut insert = tx.prepare("INSERT INTO library_roots(position, path) VALUES(?1, ?2)")?;
        for (position, root) in roots.iter().enumerate() {
            insert.execute(params![position as i64, root.to_string_lossy()])?;
        }
        Ok(())
    })
}

/// Opens the cache file, making whichever of its tables aren't there yet.
pub fn open_cache(path: &Path) -> rusqlite::Result<Connection> {
    let conn = connect(path, Durability::Cache)?;
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

pub fn save_index(conn: &Connection, root: &str, files: &[ScannedFile]) -> rusqlite::Result<()> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    in_transaction(conn, |tx| {
        tx.execute(
            "INSERT INTO roots(path, scanned_at) VALUES(?1, ?2)
             ON CONFLICT(path) DO UPDATE SET scanned_at = excluded.scanned_at",
            params![root, now],
        )?;
        let id: i64 = tx.query_row("SELECT id FROM roots WHERE path = ?1", [root], |row| row.get(0))?;
        tx.execute("DELETE FROM files WHERE root = ?1", [id])?;
        let mut insert = tx.prepare("INSERT INTO files(root, rel, kind, size, mtime) VALUES(?1, ?2, ?3, ?4, ?5)")?;
        for file in files {
            insert.execute(params![id, file.rel, file.kind.to_db(), file.size as i64, file.mtime])?;
        }
        Ok(())
    })
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
    in_transaction(conn, |tx| {
        let mut statement = tx.prepare_cached("DELETE FROM settings WHERE key = ?1")?;
        for key in keys {
            statement.execute([key])?;
        }
        Ok(())
    })
}

/// Saves several settings in one transaction.
pub fn set_settings(conn: &Connection, settings: &[(&str, &str)]) -> rusqlite::Result<()> {
    in_transaction(conn, |tx| {
        let mut statement = tx.prepare_cached(
            "INSERT INTO settings(key, value) VALUES(?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )?;
        for (key, value) in settings {
            statement.execute(params![key, value])?;
        }
        Ok(())
    })
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
        let ui = open_cache(&path).expect("UI connection");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(21));
        let threads: Vec<_> = (0..21)
            .map(|i| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    let conn = open_cache(&path).map_err(|err| format!("open {i}: {err}"))?;
                    set_setting(&conn, &format!("thread.{i}"), "here").map_err(|err| format!("write {i}: {err}"))
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
        let conn = open_user_data(&path).expect("user data");
        assert!(load_roots(&conn).unwrap().is_empty());
        let roots = [PathBuf::from("F:\\Resources"), PathBuf::from("D:\\Audio"), PathBuf::from("C:\\Art")];
        save_roots(&conn, &roots).unwrap();
        save_roots(&conn, &roots[1..]).unwrap();
        drop(conn);
        let conn = open_user_data(&path).expect("user data again");
        assert_eq!(load_roots(&conn).unwrap(), roots[1..], "saving replaces the list");
    }
}
