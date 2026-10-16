//! What the app keeps on disk, how big each part is, and clearing parts of it (Settings → Storage).
//!
//! The cache (`paths::cache_file`) is all made again as it's needed: thumbnails and model pictures,
//! waveforms, model facts, the folder index, and the window's settings. The user data file
//! (`paths::user_data_file`) holds what can't be: favorites, collections and categories, sprite sheet
//! choices and the library's folders.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;

use crate::diag;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Part {
    Thumbnails,
    Waveforms,
    ModelFacts,
    Index,
    Settings,
    Groups,
    SheetChoices,
    LibraryFolders,
}

impl Part {
    /// In the cache file: made again as needed.
    pub const CACHE: [Part; 5] = [Part::Thumbnails, Part::Waveforms, Part::ModelFacts, Part::Index, Part::Settings];
    /// In the user data file: gone for good once cleared.
    pub const USER: [Part; 3] = [Part::Groups, Part::SheetChoices, Part::LibraryFolders];

    pub fn label(self) -> &'static str {
        match self {
            Part::Thumbnails => "Thumbnails and model pictures",
            Part::Waveforms => "Sound waveforms",
            Part::ModelFacts => "3D model details",
            Part::Index => "Folder index",
            Part::Settings => "Settings",
            Part::Groups => "Favorites, collections and categories",
            Part::SheetChoices => "Sprite sheet choices",
            Part::LibraryFolders => "Library folders",
        }
    }

    /// What clearing it does.
    pub fn note(self) -> &'static str {
        match self {
            Part::Thumbnails => "Made again as they're shown, which takes a while for big folders",
            Part::Waveforms => "Made again as sounds are shown",
            Part::ModelFacts => "Read again from the files, with their pictures",
            Part::Index => "Every folder is scanned again, now and at the next start",
            Part::Settings => "Modes, views, volume, shortcuts and memory go back to how they started",
            Part::Groups => "Every collection and category, and what's in Favorites",
            Part::SheetChoices => "Grids, speeds and which pictures play as animations",
            Part::LibraryFolders => "The library is emptied; the folders themselves are untouched",
        }
    }

    pub fn in_user_data(self) -> bool {
        Part::USER.contains(&self)
    }

    /// Its tables, in whichever file it's in.
    fn tables(self) -> &'static [&'static str] {
        match self {
            Part::Thumbnails => &["thumbs"],
            Part::Waveforms => &["waveforms"],
            Part::ModelFacts => &["models"],
            Part::Index => &["roots", "files"],
            Part::Settings => &["settings"],
            Part::Groups => &["groups", "members"],
            Part::SheetChoices => &["sheets"],
            Part::LibraryFolders => &["library_roots"],
        }
    }

    /// The table whose rows are counted for it.
    fn counted(self) -> &'static str {
        match self {
            Part::Index => "files",
            Part::Groups => "members",
            part => part.tables()[0],
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PartUsage {
    /// Thumbnails, sounds, files indexed, assets in groups, pictures, folders.
    pub count: u64,
    /// `None` when the file's page statistics couldn't be read (that's logged).
    pub bytes: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct Usage {
    /// Each file's size on disk, with its write-ahead log.
    pub cache_file: u64,
    pub user_file: u64,
    pub parts: HashMap<Part, PartUsage>,
    /// Collections and categories (Favorites isn't counted).
    pub groups: u64,
}

/// Measures both files. Reads every page of the cache, so it's for a background thread. Every
/// failure is logged with the file and the step. Sizes that can't be read are left unknown, with
/// the counts still given; only a file that can't be opened or counted fails.
pub fn measure(cache: &Path, user: &Path) -> Result<Usage, String> {
    let mut usage = Usage { cache_file: file_size(cache), user_file: file_size(user), ..Usage::default() };
    for (path, parts) in [(cache, &Part::CACHE[..]), (user, &Part::USER[..])] {
        if !path.exists() {
            continue;
        }
        if let Err(err) = measure_file(path, parts, &mut usage) {
            let message = format!("{}: {err}", path.display());
            diag::report("Measuring the files", &message);
            return Err(message);
        }
    }
    Ok(usage)
}

fn measure_file(path: &Path, parts: &[Part], usage: &mut Usage) -> Result<(), String> {
    // Opened as the app's other connections are.
    let conn = Connection::open(path).map_err(|err| format!("opening it: {err}"))?;
    conn.busy_timeout(Duration::from_secs(10)).map_err(|err| format!("opening it: {err}"))?;
    // The space each table takes, from SQLite's page statistics, tried three times.
    let mut bytes = None;
    for attempt in 1..=3 {
        match table_bytes(&conn) {
            Ok(measured) => {
                bytes = Some(measured);
                break;
            }
            Err(err) if attempt < 3 => {
                diag::note(&format!("Measuring {} (try {attempt} of 3)", path.display()), err);
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(err) => diag::report(&format!("Measuring {}: sizes left unknown", path.display()), err),
        }
    }
    for &part in parts {
        let count = rows(&conn, part.counted(), "")?;
        let bytes =
            bytes.as_ref().map(|bytes| part.tables().iter().map(|table| bytes.get(*table).copied().unwrap_or(0)).sum());
        usage.parts.insert(part, PartUsage { count, bytes });
    }
    if parts.contains(&Part::Groups) {
        usage.groups = rows(&conn, "groups", " WHERE kind <> 0")?;
    }
    Ok(())
}

/// A table's rows (those matching `filter`); none if the table hasn't been made yet.
fn rows(conn: &Connection, table: &str, filter: &str) -> Result<u64, String> {
    let count = |sql: &str| conn.query_row(sql, [], |row| row.get::<_, i64>(0)).map(|n| n as u64);
    let exists = count(&format!("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = '{table}'"))
        .map_err(|err| format!("looking for {table}: {err}"))?;
    if exists == 0 {
        return Ok(0);
    }
    count(&format!("SELECT COUNT(*) FROM {table}{filter}")).map_err(|err| format!("counting {table}: {err}"))
}

fn file_size(path: &Path) -> u64 {
    let size = |path: &Path| std::fs::metadata(path).map_or(0, |meta| meta.len());
    size(path) + size(&path.with_extension("sqlite-wal"))
}

/// Bytes in the file per table, its indexes included (SQLite's `dbstat`, built in).
fn table_bytes(conn: &Connection) -> rusqlite::Result<HashMap<String, u64>> {
    let mut of_table: HashMap<String, String> = HashMap::new();
    let mut statement = conn.prepare("SELECT name, tbl_name FROM sqlite_master WHERE type IN ('table', 'index')")?;
    for row in statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))? {
        let (name, table) = row?;
        of_table.insert(name, table);
    }
    let mut bytes = HashMap::new();
    let mut statement = conn.prepare("SELECT name, SUM(pgsize) FROM dbstat GROUP BY name")?;
    for row in statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))? {
        let (name, size) = row?;
        let table = of_table.get(&name).cloned().unwrap_or(name);
        *bytes.entry(table).or_insert(0) += size as u64;
    }
    Ok(bytes)
}

/// Empties the cache's `parts`, then rewrites the file without the space they took. On the cache
/// writer's thread, so no preview is being saved meanwhile; readers carry on.
pub fn clear_cache(conn: &mut Connection, parts: &[Part]) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    for part in parts.iter().filter(|part| !part.in_user_data()) {
        for table in part.tables() {
            tx.execute(&format!("DELETE FROM {table}"), [])?;
        }
    }
    tx.commit()?;
    conn.execute_batch("VACUUM")?;
    // The log took the whole rewritten file; this hands that space back too.
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{self, CacheWrite, StoredThumb};
    use crate::test_support::TempDir;

    #[test]
    fn measures_and_clears_parts_of_the_cache() {
        let temp = TempDir::new("storage");
        let dir = temp.path();
        let path = dir.join("library.sqlite");
        let mut conn = db::open(&path).unwrap();
        let thumb = || StoredThumb {
            w: 64,
            h: 64,
            src_w: 64,
            src_h: 64,
            format: db::FORMAT_RAW_RGBA,
            data: vec![7; 64 * 64 * 4],
        };
        let writes: Vec<CacheWrite> = (0..200).map(|key| CacheWrite::Thumb(key, thumb())).collect();
        db::put_cache(&mut conn, &writes).unwrap();
        db::set_setting(&conn, "ui.mode", "all").unwrap();

        let usage = measure(&path, &dir.join("user.sqlite")).unwrap();
        let thumbs = usage.parts[&Part::Thumbnails];
        assert_eq!(thumbs.count, 200);
        let bytes = thumbs.bytes.expect("the sizes are read");
        assert!(bytes >= 200 * 64 * 64 * 4, "{thumbs:?}");
        assert_eq!(usage.parts[&Part::Settings].count, 1);
        assert!(usage.cache_file >= bytes);

        clear_cache(&mut conn, &[Part::Thumbnails]).unwrap();
        let after = measure(&path, &dir.join("user.sqlite")).unwrap();
        assert_eq!(after.parts[&Part::Thumbnails].count, 0);
        assert_eq!(after.parts[&Part::Settings].count, 1, "the rest is kept");
        assert!(
            after.cache_file < usage.cache_file / 4,
            "the space is given back: {} → {}",
            usage.cache_file,
            after.cache_file
        );
    }
}
