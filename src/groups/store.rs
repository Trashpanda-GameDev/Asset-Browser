//! The groups in the user data file: reading them all at the start, and the time members are added.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;

use super::{CATEGORY_COLOR_COUNT, FAVORITES, Group, GroupKind, Member, favorites_group};
use crate::db;

pub(super) fn open_db(path: &Path) -> rusqlite::Result<Connection> {
    let conn = db::open_user_data(path)?;
    // Favorites is always there, as group 0.
    conn.execute("INSERT OR IGNORE INTO groups(id, kind, name, color, created) VALUES(0, 0, 'Favorites', 0, 0)", [])?;
    Ok(conn)
}

pub(super) fn load(conn: &Connection) -> rusqlite::Result<Vec<Group>> {
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

pub(super) fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}
