//! Favorites, collections and categories: the user's own ways of organizing assets, across
//! folders and without moving a file. All three are named sets of file paths:
//! - **Favorites**, the one set that's always there (the starred assets);
//! - **collections**: lists for a purpose ("Forest level props"), browsed like a folder;
//! - **categories**: labels with a colour ("Footsteps", "UI"), shown on the assets themselves and
//!   matched by the search box.
//!
//! They're the user's work rather than a cache, so they're kept in the user data file
//! (`paths::user_data_file`, in `%APPDATA%\AssetBrowser\data` on Windows), not in the cache
//! database, which can be deleted at any time. Everything is held in memory too, and each change
//! is written through at once, in one transaction.
//!
//! A member is identified by its absolute path, normalised (`path_key`): `/` separators and
//! lowercase, as the library matches paths. So it survives the file being edited (which changes
//! its cache key) and opening a parent folder instead of the one it was added from.

mod membership;
mod store;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::diag;
pub use crate::library::path_key;
pub use membership::Membership;
use store::Store;

pub const FAVORITES: i64 = 0;

/// Not a stored group: every asset in no category, worked out by `Membership`.
pub const UNCATEGORIZED: i64 = -1;

pub const UNCATEGORIZED_NAME: &str = "Uncategorized";

/// How many colours categories cycle through. The palette is the UI's (`theme::CATEGORY_COLORS`), and
/// the compiler holds it to this many.
pub const CATEGORY_COLOR_COUNT: u8 = 8;

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
    /// Where every change is saved before it's made.
    store: Store,
    /// Favorites first, then collections, then categories, each in the order they were made.
    list: Vec<Group>,
}

impl Groups {
    pub fn open(path: &Path) -> Self {
        let (store, list) = Store::open(path);
        Self { store, list }
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
        let color = (self.of_kind(GroupKind::Category).count() % CATEGORY_COLOR_COUNT as usize) as u8;
        let id = match saved(self.store.create(kind, name, color), "Saving a new group")? {
            Some(id) => id,
            // Not saved anywhere: an id for this session.
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
        if saved(self.store.rename(id, name), "Renaming a group").is_none() {
            return false;
        }
        let Some(group) = self.get_mut(id) else { return false };
        group.name = name.to_owned();
        group.lower = name.to_lowercase();
        true
    }

    pub fn set_color(&mut self, id: i64, color: u8) {
        let color = color % CATEGORY_COLOR_COUNT;
        if saved(self.store.set_color(id, color), "Saving a category's colour").is_none() {
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
        if saved(self.store.delete(id), "Deleting a group").is_none() {
            return false;
        }
        self.list.retain(|group| group.id != id);
        true
    }

    /// Deletes every collection and category and empties Favorites (Settings → Storage). The files
    /// are untouched.
    pub fn clear(&mut self) -> bool {
        if saved(self.store.clear(), "Clearing favorites, collections and categories").is_none() {
            return false;
        }
        self.list = vec![favorites_group()];
        true
    }

    /// Adds the files that aren't in the group yet; returns how many were added.
    pub fn add(&mut self, id: i64, paths: &[PathBuf]) -> usize {
        let Some(group) = self.get(id) else { return 0 };
        let mut keys = HashSet::new();
        let added: Vec<(Member, i64)> = paths
            .iter()
            .filter_map(|path| {
                let key = path_key(path);
                (!group.contains(&key) && keys.insert(key.clone()))
                    .then(|| Member { key, path: path.to_string_lossy().into_owned() })
            })
            .zip(group.next_seq..)
            .collect();
        if added.is_empty() || saved(self.store.add(id, &added), "Saving to a group").is_none() {
            return 0;
        }
        let Some(group) = self.get_mut(id) else { return 0 };
        let count = added.len();
        group.next_seq += count as i64;
        group.keys.extend(keys);
        group.members.extend(added.into_iter().map(|(member, _)| member));
        count
    }

    /// Takes files out of the group (the files themselves are untouched); returns how many.
    pub fn remove(&mut self, id: i64, keys: &[String]) -> usize {
        let Some(group) = self.get(id) else { return 0 };
        let gone: HashSet<String> = keys.iter().filter(|key| group.contains(key)).cloned().collect();
        if gone.is_empty() || saved(self.store.remove(id, &gone), "Removing from a group").is_none() {
            return 0;
        }
        let Some(group) = self.get_mut(id) else { return 0 };
        group.members.retain(|member| !gone.contains(&member.key));
        group.keys.retain(|key| !gone.contains(key));
        gone.len()
    }
}

/// What a store change gave, or `None` once its failure is reported.
fn saved<T>(result: rusqlite::Result<T>, doing: &str) -> Option<T> {
    result.map_err(|err| diag::report(doing, err)).ok()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::{AssetKind, KindSet};
    use crate::library::Library;
    use crate::scan::ScannedFile;
    use crate::test_support::TempDir;

    /// The user data file, in a folder of its own.
    fn user_data(name: &str) -> (TempDir, PathBuf) {
        let dir = TempDir::new(name);
        let path = dir.path().join("user.sqlite");
        (dir, path)
    }

    /// Saved in the user data file: changing a number would turn people's collections into
    /// categories.
    #[test]
    fn stored_kinds_never_change() {
        let kinds = [GroupKind::Favorites, GroupKind::Collection, GroupKind::Category];
        assert_eq!(kinds.map(GroupKind::to_db), [0, 1, 2]);
        for kind in kinds {
            assert_eq!(GroupKind::from_db(kind.to_db()), Some(kind));
        }
        assert_eq!(GroupKind::from_db(3), None);
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

    /// What the file refuses isn't done in memory either, so what's shown is what's saved.
    #[test]
    fn a_change_that_cant_be_saved_isnt_made() {
        let (_dir, db) = user_data("groups_refused");
        let mut groups = Groups::open(&db);
        let props = groups.create(GroupKind::Collection, "Props").unwrap();
        assert_eq!(groups.add(props, &[PathBuf::from("/x/1.png")]), 1);
        // Another connection takes the members away under it.
        rusqlite::Connection::open(&db).unwrap().execute_batch("DROP TABLE members").unwrap();
        assert_eq!(groups.add(props, &[PathBuf::from("/x/2.png")]), 0);
        assert!(!groups.contains(props, "/x/2.png"), "not saved, so not added");
        assert_eq!(groups.remove(props, &["/x/1.png".to_owned()]), 0);
        assert!(groups.contains(props, "/x/1.png"), "not saved, so not taken out");
        assert!(!groups.delete(props) && groups.get(props).is_some(), "not saved, so not deleted");
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
        let path = |entry: usize| library.path_of(&library.entries()[entry]);
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
        assert_eq!(membership.total(FAVORITES, KindSet::only(AssetKind::Audio)), 1);
        assert!(membership.is_favorite(icon) && !membership.is_favorite(library.find(0, "rock.fbx").unwrap()));
        assert_eq!(membership.groups_of(step), [FAVORITES, footsteps]);
        assert!(membership.category_matches(&groups, step, "foot"));
        assert!(!membership.category_matches(&groups, icon, "foot"));
        // Search finds the sound by its category although "footsteps" isn't in its path.
        let found = library
            .filter_with(0, KindSet::ALL, "footsteps", |index, word| membership.category_matches(&groups, index, word));
        assert_eq!(found, [step]);
        // Uncategorized: everything in no category (Favorites isn't one), in library order.
        let rock = library.find(0, "rock.fbx").unwrap();
        let mut uncategorized = vec![icon, rock];
        uncategorized.sort_unstable();
        assert_eq!(membership.entries[&UNCATEGORIZED], uncategorized);
        assert_eq!(membership.totals(UNCATEGORIZED), [1, 0, 1]);
    }
}
