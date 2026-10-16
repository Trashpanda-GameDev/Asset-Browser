//! The groups in the user data file. Every change is saved here first, and `Groups` makes it in
//! memory only once it's saved, so what's shown is what's in the file.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, Params, Transaction, params};

use super::{CATEGORY_COLOR_COUNT, FAVORITES, Group, GroupKind, Member, favorites_group};
use crate::{db, diag};

/// The user data file, or nothing when it couldn't be opened: the groups then work for the
/// session without being saved.
pub(super) struct Store {
    conn: Option<Connection>,
}

impl Store {
    /// Opens the file and reads every group. When it can't, that's reported and there's Favorites.
    pub(super) fn open(path: &Path) -> (Self, Vec<Group>) {
        match open_db(path).and_then(|conn| load(&conn).map(|list| (conn, list))) {
            Ok((conn, list)) => (Self { conn: Some(conn) }, list),
            Err(err) => {
                diag::report(&format!("Favorites and collections unavailable ({})", path.display()), err);
                (Self { conn: None }, vec![favorites_group()])
            }
        }
    }

    /// Saves a new group: its id, or `None` when there's no file to save it in.
    pub(super) fn create(&self, kind: GroupKind, name: &str, color: u8) -> rusqlite::Result<Option<i64>> {
        let Some(conn) = &self.conn else { return Ok(None) };
        conn.execute(
            "INSERT INTO groups(kind, name, color, created) VALUES(?1, ?2, ?3, ?4)",
            params![kind.to_db(), name, color, now_ms()],
        )?;
        Ok(Some(conn.last_insert_rowid()))
    }

    pub(super) fn rename(&self, id: i64, name: &str) -> rusqlite::Result<()> {
        self.execute("UPDATE groups SET name = ?1 WHERE id = ?2", params![name, id])
    }

    pub(super) fn set_color(&self, id: i64, color: u8) -> rusqlite::Result<()> {
        self.execute("UPDATE groups SET color = ?1 WHERE id = ?2", params![color, id])
    }

    pub(super) fn delete(&self, id: i64) -> rusqlite::Result<()> {
        self.in_transaction(|tx| {
            tx.execute("DELETE FROM members WHERE grp = ?1", [id])?;
            tx.execute("DELETE FROM groups WHERE id = ?1", [id])?;
            Ok(())
        })
    }

    /// Deletes every collection and category, and empties Favorites.
    pub(super) fn clear(&self) -> rusqlite::Result<()> {
        self.in_transaction(|tx| {
            tx.execute("DELETE FROM members", [])?;
            tx.execute("DELETE FROM groups WHERE id <> ?1", [FAVORITES])?;
            Ok(())
        })
    }

    /// Adds members to group `id`, each with its place in the group's order.
    pub(super) fn add(&self, id: i64, members: &[(Member, i64)]) -> rusqlite::Result<()> {
        let now = now_ms();
        self.in_transaction(|tx| {
            let mut insert = tx.prepare_cached(
                "INSERT OR IGNORE INTO members(grp, key, path, seq, added) VALUES(?1, ?2, ?3, ?4, ?5)",
            )?;
            for (member, seq) in members {
                insert.execute(params![id, member.key, member.path, seq, now])?;
            }
            Ok(())
        })
    }

    pub(super) fn remove(&self, id: i64, keys: &HashSet<String>) -> rusqlite::Result<()> {
        self.in_transaction(|tx| {
            let mut delete = tx.prepare_cached("DELETE FROM members WHERE grp = ?1 AND key = ?2")?;
            for key in keys {
                delete.execute(params![id, key])?;
            }
            Ok(())
        })
    }

    /// One statement; nothing to do without a file.
    fn execute(&self, sql: &str, params: impl Params) -> rusqlite::Result<()> {
        if let Some(conn) = &self.conn {
            conn.execute(sql, params)?;
        }
        Ok(())
    }

    /// Several statements as one; nothing to do without a file.
    fn in_transaction(&self, work: impl FnOnce(&Transaction<'_>) -> rusqlite::Result<()>) -> rusqlite::Result<()> {
        self.conn.as_ref().map_or(Ok(()), |conn| db::in_transaction(conn, work))
    }
}

fn open_db(path: &Path) -> rusqlite::Result<Connection> {
    let conn = db::open_user_data(path)?;
    // Favorites is always there, as group 0.
    conn.execute("INSERT OR IGNORE INTO groups(id, kind, name, color, created) VALUES(0, 0, 'Favorites', 0, 0)", [])?;
    Ok(conn)
}

fn load(conn: &Connection) -> rusqlite::Result<Vec<Group>> {
    let mut statement = conn.prepare("SELECT id, kind, name, color FROM groups ORDER BY kind, id")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?, row.get::<_, i64>(3)?))
    })?;
    let mut list = Vec::new();
    for row in rows {
        let (id, kind, name, color) = row?;
        let Some(kind) = GroupKind::from_db(kind) else { continue };
        list.push(Group {
            id,
            kind,
            lower: name.to_lowercase(),
            name,
            color: (color.rem_euclid(CATEGORY_COLOR_COUNT as i64)) as u8,
            members: Vec::new(),
            keys: HashSet::new(),
            next_seq: 0,
        });
    }
    let mut statement = conn.prepare("SELECT grp, key, path, seq FROM members ORDER BY grp, seq")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, i64>(3)?))
    })?;
    let mut by_id: HashMap<i64, usize> = list.iter().enumerate().map(|(index, group)| (group.id, index)).collect();
    if !by_id.contains_key(&FAVORITES) {
        list.insert(0, favorites_group());
        by_id = list.iter().enumerate().map(|(index, group)| (group.id, index)).collect();
    }
    for row in rows {
        let (grp, key, path, seq) = row?;
        let Some(&index) = by_id.get(&grp) else { continue };
        let group = &mut list[index];
        if group.keys.insert(key.clone()) {
            group.members.push(Member { key, path });
            group.next_seq = group.next_seq.max(seq + 1);
        }
    }
    Ok(list)
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}
