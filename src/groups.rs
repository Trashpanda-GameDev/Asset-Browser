//! Favorites, collections and categories: the user's own ways of organizing assets, across
//! folders and without moving a file. All three are named sets of file paths:
//! - **Favorites**, the one set that's always there (the starred assets);
//! - **collections**: lists for a purpose ("Forest level props"), browsed like a folder;
//! - **categories**: labels with a colour ("Footsteps", "UI"), shown on the assets themselves and
//!   matched by the search box.
//!
//! They're the user's work rather than a cache, so they're kept in the user data file
//! (`db::user_data_path`, in `%APPDATA%\AssetBrowser\data` on Windows), not in the cache
//! database, which can be deleted at any time. Everything is held in memory too, and each change
//! is written through at once, in one transaction.
//!
//! A member is identified by its absolute path, normalised (`path_key`): `/` separators and
//! lowercase, as the library matches paths. So it survives the file being edited (which changes
//! its cache key) and opening a parent folder instead of the one it was added from.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};

use crate::diag;
pub use crate::library::path_key;
use crate::library::{Library, Located};

pub const FAVORITES: i64 = 0;
/// Not a stored group: every asset in no category, worked out by `Membership`.
pub const UNCATEGORIZED: i64 = -1;
pub const UNCATEGORIZED_NAME: &str = "Uncategorized";
/// How many colours categories cycle through (the palette is the UI's: `theme::CATEGORY_COLORS`).
pub const CATEGORY_COLORS: u8 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GroupKind {
    Favorites,
    Collection,
    Category,
}

impl GroupKind {
    fn to_db(self) -> i64 {
        match self {
            Self::Favorites => 0,
            Self::Collection => 1,
            Self::Category => 2,
        }
    }

    fn from_db(value: i64) -> Option<Self> {
        match value {
            0 => Some(Self::Favorites),
            1 => Some(Self::Collection),
            2 => Some(Self::Category),
            _ => None,
        }
    }

    /// "collection", "category": for messages.
    pub fn noun(self) -> &'static str {
        match self {
            Self::Favorites => "favorites",
            Self::Collection => "collection",
            Self::Category => "category",
        }
    }
}

pub struct Group {
    pub id: i64,
    pub kind: GroupKind,
    pub name: String,
    /// `name` in lowercase, for search.
    pub lower: String,
    /// Index into the category palette.
    pub color: u8,
    /// In the order they were added.
    pub members: Vec<Member>,
    keys: HashSet<String>,
    next_seq: i64,
}

impl Group {
    pub fn contains(&self, key: &str) -> bool {
        self.keys.contains(key)
    }
}

pub struct Member {
    /// `path_key` of `path`: what membership is decided by.
    pub key: String,
    /// The path as it was added, for showing.
    pub path: String,
}

pub struct Groups {
    /// `None` when the file couldn't be opened: the groups still work, but aren't saved.
    conn: Option<Connection>,
    /// Favorites first, then collections, then categories, each in the order they were made.
    list: Vec<Group>,
}

impl Groups {
    pub fn open(path: &Path) -> Self {
        let opened = open_db(path).and_then(|conn| load(&conn).map(|list| (conn, list)));
        match opened {
            Ok((conn, list)) => Self { conn: Some(conn), list },
            Err(err) => {
                diag::report(&format!("Favorites and collections unavailable ({})", path.display()), err);
                Self { conn: None, list: vec![favorites_group()] }
            }
        }
    }

    pub fn all(&self) -> &[Group] {
        &self.list
    }

    pub fn of_kind(&self, kind: GroupKind) -> impl Iterator<Item = &Group> {
        self.list.iter().filter(move |group| group.kind == kind)
    }

    pub fn get(&self, id: i64) -> Option<&Group> {
        self.list.iter().find(|group| group.id == id)
    }

    fn get_mut(&mut self, id: i64) -> Option<&mut Group> {
        self.list.iter_mut().find(|group| group.id == id)
    }

    pub fn contains(&self, id: i64, key: &str) -> bool {
        self.get(id).is_some_and(|group| group.contains(key))
    }

    /// "New collection", or "New collection 2" and so on when that's taken.
    pub fn unused_name(&self, kind: GroupKind) -> String {
        let base = match kind {
            GroupKind::Category => "New category",
            _ => "New collection",
        };
        let taken = |name: &str| self.of_kind(kind).any(|group| group.name.eq_ignore_ascii_case(name));
        if !taken(base) {
            return base.to_owned();
        }
        (2..).map(|n| format!("{base} {n}")).find(|name| !taken(name)).unwrap_or_else(|| base.to_owned())
    }

    /// Makes an empty collection or category; returns its id.
    pub fn create(&mut self, kind: GroupKind, name: &str) -> Option<i64> {
        let name = name.trim();
        if kind == GroupKind::Favorites || name.is_empty() {
            return None;
        }
        let color = (self.of_kind(GroupKind::Category).count() % CATEGORY_COLORS as usize) as u8;
        let created = now_ms();
        let id = match &self.conn {
            Some(conn) => match conn.execute(
                "INSERT INTO groups(kind, name, color, created) VALUES(?1, ?2, ?3, ?4)",
                params![kind.to_db(), name, color, created],
            ) {
                Ok(_) => conn.last_insert_rowid(),
                Err(err) => {
                    diag::report("Saving a new group", err);
                    return None;
                }
            },
            None => self.list.iter().map(|group| group.id).max().unwrap_or(0) + 1,
        };
        let group = Group {
            id,
            kind,
            name: name.to_owned(),
            lower: name.to_lowercase(),
            color,
            members: Vec::new(),
            keys: HashSet::new(),
            next_seq: 0,
        };
        // Keep the list grouped by kind: after the last group of the same kind (or the kinds before it).
        let at =
            self.list.iter().rposition(|other| other.kind.to_db() <= kind.to_db()).map_or(self.list.len(), |i| i + 1);
        self.list.insert(at, group);
        Some(id)
    }

    pub fn rename(&mut self, id: i64, name: &str) -> bool {
        let name = name.trim();
        if id == FAVORITES || name.is_empty() {
            return false;
        }
        if let Some(conn) = &self.conn
            && let Err(err) = conn.execute("UPDATE groups SET name = ?1 WHERE id = ?2", params![name, id])
        {
            diag::report("Renaming a group", err);
            return false;
        }
        let Some(group) = self.get_mut(id) else { return false };
        group.name = name.to_owned();
        group.lower = name.to_lowercase();
        true
    }

    pub fn set_color(&mut self, id: i64, color: u8) {
        let color = color % CATEGORY_COLORS;
        if let Some(conn) = &self.conn
            && let Err(err) = conn.execute("UPDATE groups SET color = ?1 WHERE id = ?2", params![color, id])
        {
            diag::report("Saving a category's colour", err);
            return;
        }
        if let Some(group) = self.get_mut(id) {
            group.color = color;
        }
    }

    /// Deletes a collection or category. The files are untouched; Favorites can't be deleted.
    pub fn delete(&mut self, id: i64) -> bool {
        if id == FAVORITES || self.get(id).is_none() {
            return false;
        }
        if let Some(conn) = &self.conn {
            let deleted = conn.unchecked_transaction().and_then(|tx| {
                tx.execute("DELETE FROM members WHERE grp = ?1", [id])?;
                tx.execute("DELETE FROM groups WHERE id = ?1", [id])?;
                tx.commit()
            });
            if let Err(err) = deleted {
                diag::report("Deleting a group", err);
                return false;
            }
        }
        self.list.retain(|group| group.id != id);
        true
    }

    /// Deletes every collection and category and empties Favorites (Settings → Storage). The files
    /// are untouched.
    pub fn clear(&mut self) -> bool {
        if let Some(conn) = &self.conn {
            let cleared = conn.unchecked_transaction().and_then(|tx| {
                tx.execute("DELETE FROM members", [])?;
                tx.execute("DELETE FROM groups WHERE id <> ?1", [FAVORITES])?;
                tx.commit()
            });
            if let Err(err) = cleared {
                diag::report("Clearing favorites, collections and categories", err);
                return false;
            }
        }
        self.list = vec![favorites_group()];
        true
    }

    /// Adds the files that aren't in the group yet; returns how many were added.
    pub fn add(&mut self, id: i64, paths: &[PathBuf]) -> usize {
        let Some(group) = self.list.iter_mut().find(|group| group.id == id) else { return 0 };
        let mut added = Vec::new();
        for path in paths {
            let key = path_key(path);
            if group.keys.insert(key.clone()) {
                let member = Member { key, path: path.to_string_lossy().into_owned() };
                added.push((member, group.next_seq));
                group.next_seq += 1;
            }
        }
        if added.is_empty() {
            return 0;
        }
        if let Some(conn) = &self.conn {
            let now = now_ms();
            let saved = conn.unchecked_transaction().and_then(|tx| {
                {
                    let mut insert = tx.prepare_cached(
                        "INSERT OR IGNORE INTO members(grp, key, path, seq, added) VALUES(?1, ?2, ?3, ?4, ?5)",
                    )?;
                    for (member, seq) in &added {
                        insert.execute(params![id, member.key, member.path, seq, now])?;
                    }
                }
                tx.commit()
            });
            if let Err(err) = saved {
                diag::report("Saving to a group", err);
            }
        }
        let count = added.len();
        group.members.extend(added.into_iter().map(|(member, _)| member));
        count
    }

    /// Takes files out of the group (the files themselves are untouched); returns how many.
    pub fn remove(&mut self, id: i64, keys: &[String]) -> usize {
        let Some(group) = self.list.iter_mut().find(|group| group.id == id) else { return 0 };
        let gone: HashSet<&String> = keys.iter().filter(|key| group.keys.contains(*key)).collect();
        if gone.is_empty() {
            return 0;
        }
        group.members.retain(|member| !gone.contains(&member.key));
        for key in &gone {
            group.keys.remove(*key);
        }
        if let Some(conn) = &self.conn {
            let saved = conn.unchecked_transaction().and_then(|tx| {
                {
                    let mut delete = tx.prepare_cached("DELETE FROM members WHERE grp = ?1 AND key = ?2")?;
                    for key in &gone {
                        delete.execute(params![id, key])?;
                    }
                }
                tx.commit()
            });
            if let Err(err) = saved {
                diag::report("Removing from a group", err);
            }
        }
        gone.len()
    }
}

fn favorites_group() -> Group {
    Group {
        id: FAVORITES,
        kind: GroupKind::Favorites,
        name: "Favorites".to_owned(),
        lower: "favorites".to_owned(),
        color: 0,
        members: Vec::new(),
        keys: HashSet::new(),
        next_seq: 0,
    }
}

fn open_db(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let conn = Connection::open(path)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get::<_, String>(0))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS groups(
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
         INSERT OR IGNORE INTO groups(id, kind, name, color, created) VALUES(0, 0, 'Favorites', 0, 0);",
    )?;
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
            color: (color.rem_euclid(CATEGORY_COLORS as i64)) as u8,
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

/// Where each group's members are in the open library. Rebuilt whenever the library or a group
/// changes; cheap, since each member costs one binary search.
#[derive(Default)]
pub struct Membership {
    /// Per group: its members in the library, as entry indices, in the group's order.
    pub entries: HashMap<i64, Vec<u32>>,
    /// Per entry that's in any group: those groups, in `Groups::all` order.
    of_entry: HashMap<u32, Vec<i64>>,
    /// Per group: how many members of each kind are in the library.
    totals: HashMap<i64, [u32; 3]>,
    /// Per group: members under the library's folder that aren't there any more (moved,
    /// renamed or deleted), by key.
    pub missing: HashMap<i64, Vec<String>>,
    /// Per group: how many members are outside the library's folder.
    pub elsewhere: HashMap<i64, usize>,
}

impl Membership {
    pub fn build(library: &Library, groups: &Groups) -> Self {
        let mut membership = Membership::default();
        let mut categorized = vec![false; library.entries.len()];
        for group in groups.all() {
            let mut entries = Vec::new();
            let mut totals = [0u32; 3];
            for member in &group.members {
                match library.locate(&member.key) {
                    Located::Found(index) => {
                        entries.push(index);
                        totals[library.entries[index as usize].kind.index()] += 1;
                        membership.of_entry.entry(index).or_default().push(group.id);
                        if group.kind == GroupKind::Category {
                            categorized[index as usize] = true;
                        }
                    }
                    Located::Missing => membership.missing.entry(group.id).or_default().push(member.key.clone()),
                    Located::Outside => *membership.elsewhere.entry(group.id).or_default() += 1,
                }
            }
            membership.entries.insert(group.id, entries);
            membership.totals.insert(group.id, totals);
        }
        // Uncategorized: what no category holds, in library order.
        let mut totals = [0u32; 3];
        let uncategorized: Vec<u32> = (0..library.entries.len() as u32)
            .filter(|&index| !categorized[index as usize])
            .inspect(|&index| totals[library.entries[index as usize].kind.index()] += 1)
            .collect();
        membership.entries.insert(UNCATEGORIZED, uncategorized);
        membership.totals.insert(UNCATEGORIZED, totals);
        membership
    }

    /// The groups the entry is in.
    pub fn groups_of(&self, index: u32) -> &[i64] {
        self.of_entry.get(&index).map_or(&[], Vec::as_slice)
    }

    pub fn is_favorite(&self, index: u32) -> bool {
        self.groups_of(index).contains(&FAVORITES)
    }

    pub fn totals(&self, id: i64) -> [u32; 3] {
        self.totals.get(&id).copied().unwrap_or_default()
    }

    /// Members of the enabled kinds in the library.
    pub fn total(&self, id: i64, kinds: [bool; 3]) -> u32 {
        self.totals(id).iter().zip(kinds).filter(|(_, on)| *on).map(|(n, _)| n).sum()
    }

    /// Whether `word` (lowercase) is in the name of a category the entry is in: how the search
    /// box finds assets by category.
    pub fn category_matches(&self, groups: &Groups, index: u32, word: &str) -> bool {
        self.groups_of(index).iter().any(|&id| {
            groups.get(id).is_some_and(|group| group.kind == GroupKind::Category && group.lower.contains(word))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::AssetKind;
    use crate::scan::ScannedFile;
    use crate::test_support::TempDir;

    /// The user data file, in a folder of its own.
    fn user_data(name: &str) -> (TempDir, PathBuf) {
        let dir = TempDir::new(name);
        let path = dir.path().join("user.sqlite");
        (dir, path)
    }

    #[test]
    fn keys_ignore_separators_case_and_trailing_slashes() {
        assert_eq!(path_key(Path::new("F:\\Resources\\UI/Icon.PNG")), "f:/resources/ui/icon.png");
        assert_eq!(path_key(Path::new("C:\\")), "c:");
        assert_eq!(path_key(Path::new("/home/me/assets/")), "/home/me/assets");
    }

    #[test]
    fn groups_persist_and_favorites_stay() {
        let (_dir, db) = user_data("groups_persist");
        let (collection, category) = {
            let mut groups = Groups::open(&db);
            assert_eq!(groups.all().len(), 1, "a new file has just Favorites");
            let collection = groups.create(GroupKind::Collection, "Forest props").unwrap();
            let category = groups.create(GroupKind::Category, "Footsteps").unwrap();
            let paths = [PathBuf::from("F:\\A\\one.wav"), PathBuf::from("F:\\A\\two.wav")];
            assert_eq!(groups.add(category, &paths), 2);
            assert_eq!(groups.add(category, &paths[..1]), 0, "adding twice changes nothing");
            assert_eq!(groups.add(FAVORITES, &paths[1..]), 1);
            assert_eq!(groups.add(collection, &paths), 2);
            assert_eq!(groups.remove(collection, &["f:/a/one.wav".to_owned()]), 1);
            assert!(!groups.delete(FAVORITES) && !groups.rename(FAVORITES, "Mine"));
            assert!(groups.rename(collection, "  Forest level  "));
            groups.set_color(category, 5);
            (collection, category)
        };
        let groups = Groups::open(&db);
        let kinds: Vec<GroupKind> = groups.all().iter().map(|group| group.kind).collect();
        assert_eq!(kinds, [GroupKind::Favorites, GroupKind::Collection, GroupKind::Category]);
        assert_eq!(groups.get(collection).unwrap().name, "Forest level");
        let category = groups.get(category).unwrap();
        assert_eq!(category.color, 5);
        let members: Vec<&str> = category.members.iter().map(|member| member.key.as_str()).collect();
        assert_eq!(members, ["f:/a/one.wav", "f:/a/two.wav"], "kept in the order they were added");
        assert!(groups.contains(FAVORITES, "f:/a/two.wav"));
        assert_eq!(groups.get(collection).unwrap().members.len(), 1);
    }

    #[test]
    fn deleting_a_group_leaves_the_others() {
        let (_dir, db) = user_data("groups_delete");
        let mut groups = Groups::open(&db);
        let a = groups.create(GroupKind::Collection, "A").unwrap();
        let b = groups.create(GroupKind::Collection, "B").unwrap();
        groups.add(a, &[PathBuf::from("/x/1.png")]);
        groups.add(b, &[PathBuf::from("/x/1.png")]);
        assert!(groups.delete(a));
        drop(groups);
        let groups = Groups::open(&db);
        assert!(groups.get(a).is_none());
        assert!(groups.contains(b, "/x/1.png"));
        assert_eq!(groups.unused_name(GroupKind::Collection), "New collection");
    }

    #[test]
    fn membership_finds_members_in_the_library() {
        let (_dir, db) = user_data("groups_membership");
        let file = |rel: &str, kind| ScannedFile { rel: rel.to_owned(), kind, size: 1, mtime: 1 };
        let root = std::env::temp_dir().join("asset_browser_membership_root");
        let library = Library::build(vec![(
            root.clone(),
            vec![
                file("ui/Icon.png", AssetKind::Image),
                file("sfx/step.wav", AssetKind::Audio),
                file("rock.fbx", AssetKind::Model),
            ],
        )]);
        let mut groups = Groups::open(&db);
        let footsteps = groups.create(GroupKind::Category, "Footsteps").unwrap();
        let path = |entry: usize| library.path_of(&library.entries[entry]);
        let step = library.find(0, "sfx/step.wav").unwrap();
        let icon = library.find(0, "ui/icon.png").unwrap();
        groups.add(footsteps, &[path(step as usize), root.join("gone.wav"), PathBuf::from("/elsewhere/x.wav")]);
        groups.add(FAVORITES, &[path(icon as usize), path(step as usize)]);

        let membership = Membership::build(&library, &groups);
        assert_eq!(membership.entries[&footsteps], [step]);
        assert_eq!(membership.entries[&FAVORITES], [icon, step], "in the order they were added");
        assert_eq!(membership.missing[&footsteps].len(), 1);
        assert_eq!(membership.elsewhere[&footsteps], 1);
        assert_eq!(membership.totals(FAVORITES), [1, 1, 0]);
        assert_eq!(membership.total(FAVORITES, [false, true, false]), 1);
        assert!(membership.is_favorite(icon) && !membership.is_favorite(library.find(0, "rock.fbx").unwrap()));
        assert_eq!(membership.groups_of(step), [FAVORITES, footsteps]);
        assert!(membership.category_matches(&groups, step, "foot"));
        assert!(!membership.category_matches(&groups, icon, "foot"));
        // Search finds the sound by its category although "footsteps" isn't in its path.
        let found = library
            .filter_with(0, [true; 3], "footsteps", |index, word| membership.category_matches(&groups, index, word));
        assert_eq!(found, [step]);
        // Uncategorized: everything in no category (Favorites isn't one), in library order.
        let rock = library.find(0, "rock.fbx").unwrap();
        let mut uncategorized = vec![icon, rock];
        uncategorized.sort_unstable();
        assert_eq!(membership.entries[&UNCATEGORIZED], uncategorized);
        assert_eq!(membership.totals(UNCATEGORIZED), [1, 0, 1]);
    }
}
