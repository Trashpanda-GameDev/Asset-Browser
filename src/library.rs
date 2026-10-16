//! The files of the library's folders (its roots), sorted, with one folder tree over all of them
//! and fast filtering.
//!
//! The tree's top (`dirs[0]`, "All folders") holds every root, and each root's folders are under
//! its own node. A folder is known across rescans by its lowercase absolute path (`DirNode::key`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::asset::{AssetKind, KindSet};
use crate::preview_key::cache_key;
use crate::scan::ScannedFile;

/// One of the folders the library is made of.
pub struct Root {
    pub path: PathBuf,
    /// `root_string(path)`: the root's identity in the index and in every cache key.
    pub key: String,
    /// `path_key(path)`, for finding files by absolute path.
    pub path_key: String,
    /// The folder's own name ("Resources" for F:\Resources).
    pub name: String,
}

pub struct Entry {
    /// Index into `Library::roots`.
    pub root: u16,
    /// Path relative to the root, with `/` separators.
    pub rel: String,
    /// `rel` in lowercase, for sorting and search.
    pub lower: String,
    pub kind: AssetKind,
    pub size: u64,
    pub mtime: i64,
    /// Index into `Library::dirs` of the folder holding this file.
    pub dir: u32,
    /// Identifies this exact file version in the thumbnail cache.
    pub key: i64,
}

impl Entry {
    pub fn name(&self) -> &str {
        self.rel.rsplit('/').next().unwrap_or(&self.rel)
    }

    pub fn extension(&self) -> &str {
        let name = self.name();
        name.rsplit_once('.').map_or("", |(_, ext)| ext)
    }

    /// The folder part of `rel` ("" for files directly in the root).
    pub fn folder(&self) -> &str {
        self.rel.rsplit_once('/').map_or("", |(folder, _)| folder)
    }

    fn lower_name(&self) -> &str {
        self.lower.rsplit('/').next().unwrap_or(&self.lower)
    }

    fn lower_extension(&self) -> &str {
        self.lower_name().rsplit_once('.').map_or("", |(_, ext)| ext)
    }
}

pub struct DirNode {
    pub name: String,
    /// Lowercase path relative to its root ("" for a root itself, and for the top).
    pub lower: String,
    /// The lowercase absolute path ("" for the top): how a folder is found again after a rescan.
    pub key: String,
    /// The root it's in; `None` for the top, which holds them all.
    pub root: Option<u16>,
    pub parent: Option<u32>,
    pub children: Vec<u32>,
    /// Assets of each kind in this folder and everything below it.
    pub totals: [u32; 3],
}

impl DirNode {
    /// Assets of the enabled kinds in this folder and below it.
    pub fn total(&self, kinds: KindSet) -> u32 {
        kinds.count(self.totals)
    }
}

/// The order of the browsed assets. `Folder` is the library's own order: by path, so each
/// folder's files stay together. `Added` is the order assets went into a collection or category
/// (see `groups`); in a folder it's the same as `Folder`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortBy {
    Added,
    Folder,
    Name,
    Type,
    Size,
    Modified,
}

impl SortBy {
    /// The orders a folder offers.
    pub const ALL: [SortBy; 5] = [Self::Folder, Self::Name, Self::Type, Self::Size, Self::Modified];
    /// The orders a collection or category offers.
    pub const IN_GROUPS: [SortBy; 6] = [Self::Added, Self::Folder, Self::Name, Self::Type, Self::Size, Self::Modified];

    pub fn label(self) -> &'static str {
        match self {
            Self::Added => "Added",
            Self::Folder => "Folder",
            Self::Name => "Name",
            Self::Type => "Type",
            Self::Size => "Size",
            Self::Modified => "Modified",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Folder => "folder",
            Self::Name => "name",
            Self::Type => "type",
            Self::Size => "size",
            Self::Modified => "modified",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::IN_GROUPS.into_iter().find(|sort| sort.key() == key)
    }

    /// Biggest, newest and most recently added first is what people look for.
    pub fn starts_descending(self) -> bool {
        matches!(self, Self::Size | Self::Modified | Self::Added)
    }
}

pub struct Library {
    pub roots: Vec<Root>,
    /// Sorted by root, then lowercase path.
    pub entries: Vec<Entry>,
    /// `dirs[0]` is the top ("All folders"); `dirs[1 + n]` is root `n`.
    pub dirs: Vec<DirNode>,
    pub kind_totals: [usize; 3],
}

/// Where a file is, by its absolute path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Located {
    Found(u32),
    /// Under one of the roots, but not there (moved, renamed or deleted).
    Missing,
    /// In none of the roots.
    Outside,
}

impl Library {
    /// Builds the library from each root's files, in the roots' order.
    pub fn build(roots: Vec<(PathBuf, Vec<ScannedFile>)>) -> Self {
        let mut library_roots = Vec::with_capacity(roots.len());
        let mut entries = Vec::new();
        for (index, (path, files)) in roots.into_iter().enumerate() {
            let key = root_string(&path);
            entries.extend(files.into_iter().map(|file| Entry {
                root: index as u16,
                lower: file.rel.to_lowercase(),
                key: cache_key(&key, &file.rel, file.size, file.mtime, file.kind),
                rel: file.rel,
                kind: file.kind,
                size: file.size,
                mtime: file.mtime,
                dir: 0,
            }));
            library_roots.push(Root { path_key: path_key(&path), name: folder_name(&path), path, key });
        }
        entries.sort_unstable_by(|a, b| (a.root, &a.lower).cmp(&(b.root, &b.lower)));

        let node = |name: String, lower: String, key: String, root: Option<u16>, parent: Option<u32>| DirNode {
            name,
            lower,
            key,
            root,
            parent,
            children: Vec::new(),
            totals: [0; 3],
        };
        let mut dirs = vec![node("All folders".to_owned(), String::new(), String::new(), None, None)];
        for (index, root) in library_roots.iter().enumerate() {
            dirs.push(node(root.name.clone(), String::new(), root.path_key.clone(), Some(index as u16), Some(0)));
            dirs[0].children.push(1 + index as u32);
        }
        let mut by_path: HashMap<(u16, String), u32> = HashMap::new();
        let mut kind_totals = [0; 3];
        for entry in &mut entries {
            kind_totals[entry.kind.index()] += 1;
            let dir = match entry.rel.rfind('/') {
                None => 1 + entry.root as u32,
                Some(slash) => {
                    let root = &library_roots[entry.root as usize];
                    dir_index(entry.root, &root.path_key, &entry.rel[..slash], &mut dirs, &mut by_path)
                }
            };
            entry.dir = dir;
            let mut node = Some(dir);
            while let Some(index) = node {
                dirs[index as usize].totals[entry.kind.index()] += 1;
                node = dirs[index as usize].parent;
            }
        }
        // Folders by name; the roots stay in the order they were added.
        for index in 1..dirs.len() {
            let mut children = std::mem::take(&mut dirs[index].children);
            children.sort_unstable_by(|&a, &b| dirs[a as usize].lower.cmp(&dirs[b as usize].lower));
            dirs[index].children = children;
        }

        Self { roots: library_roots, entries, dirs, kind_totals }
    }

    /// Where the folder tree starts: the top when there are several roots, the root itself when
    /// there's one.
    pub fn home(&self) -> u32 {
        if self.roots.len() == 1 { 1 } else { 0 }
    }

    /// The folder node of root `index`.
    pub fn root_dir(index: u16) -> u32 {
        1 + index as u32
    }

    pub fn path_of(&self, entry: &Entry) -> PathBuf {
        self.roots[entry.root as usize].path.join(&entry.rel)
    }

    /// Entries in `dir` (and below it) of the enabled kinds whose path contains every word of
    /// `query` (lowercase, whitespace separated).
    pub fn filter(&self, dir: u32, kinds: KindSet, query: &str) -> Vec<u32> {
        self.filter_with(dir, kinds, query, |_, _| false)
    }

    /// `filter`, where a word missing from an entry's path can still match through `also`
    /// (entry index, word): e.g. the names of the categories the entry is in.
    pub fn filter_with(&self, dir: u32, kinds: KindSet, query: &str, also: impl Fn(u32, &str) -> bool) -> Vec<u32> {
        let (root, prefix) = match self.dirs.get(dir as usize) {
            Some(node) if !node.lower.is_empty() => (node.root, format!("{}/", node.lower)),
            Some(node) => (node.root, String::new()),
            None => (None, String::new()),
        };
        let words: Vec<&str> = query.split_whitespace().collect();
        self.entries
            .iter()
            .enumerate()
            .filter(|(index, entry)| {
                root.is_none_or(|root| entry.root == root)
                    && entry.lower.starts_with(&prefix)
                    && self.matches(*index as u32, kinds, &words, &also)
            })
            .map(|(index, _)| index as u32)
            .collect()
    }

    /// Whether the entry is of an enabled kind and has every word in its path (or in `also`).
    pub fn matches(&self, index: u32, kinds: KindSet, words: &[&str], also: &impl Fn(u32, &str) -> bool) -> bool {
        let entry = &self.entries[index as usize];
        kinds.contains(entry.kind) && words.iter().all(|word| entry.lower.contains(word) || also(index, word))
    }

    /// The entry of root `root` whose lowercase relative path is `lower`.
    pub fn find(&self, root: u16, lower: &str) -> Option<u32> {
        self.entries
            .binary_search_by(|entry| (entry.root, entry.lower.as_str()).cmp(&(root, lower)))
            .ok()
            .map(|index| index as u32)
    }

    /// Where the file with this `path_key` is.
    pub fn locate(&self, key: &str) -> Located {
        for (index, root) in self.roots.iter().enumerate() {
            let rel = key.strip_prefix(root.path_key.as_str()).and_then(|rest| rest.strip_prefix('/'));
            if let Some(rel) = rel {
                return match self.find(index as u16, rel) {
                    Some(found) => Located::Found(found),
                    None => Located::Missing,
                };
            }
        }
        Located::Outside
    }

    /// Puts `view` (indices into `entries`, in library order) in the given order. Ties keep the
    /// library order, whichever the direction.
    pub fn sort(&self, view: &mut [u32], by: SortBy, descending: bool) {
        let entry = |index: &u32| &self.entries[*index as usize];
        view.sort_unstable_by(|a, b| {
            let (x, y) = (entry(a), entry(b));
            let order = match by {
                SortBy::Folder | SortBy::Added => std::cmp::Ordering::Equal,
                SortBy::Name => x.lower_name().cmp(y.lower_name()),
                SortBy::Type => {
                    x.lower_extension().cmp(y.lower_extension()).then_with(|| x.lower_name().cmp(y.lower_name()))
                }
                SortBy::Size => x.size.cmp(&y.size),
                SortBy::Modified => x.mtime.cmp(&y.mtime),
            };
            let order = if descending { order.reverse() } else { order };
            // Library order breaks ties (and is the whole order for `Folder`).
            let whole = matches!(by, SortBy::Folder | SortBy::Added);
            order.then(if whole && descending { b.cmp(a) } else { a.cmp(b) })
        });
    }

    /// Same folders and the same files in them, at the same versions: a rescan found nothing new.
    pub fn same_files_as(&self, other: &Library) -> bool {
        self.roots.len() == other.roots.len()
            && self.roots.iter().zip(&other.roots).all(|(a, b)| a.key == b.key)
            && self.entries.len() == other.entries.len()
            && self.entries.iter().zip(&other.entries).all(|(a, b)| a.key == b.key)
    }

    /// The files of root `root` as a scan reports them, for building the library again with a
    /// root added, removed or rescanned.
    pub fn files_of(&self, root: u16) -> Vec<ScannedFile> {
        self.files_without(root, &[])
    }

    /// The files of root `root` other than those at or below `gone` (lowercase paths relative to
    /// the root, as `place_of` gives them): what's left after they were deleted.
    pub fn files_without(&self, root: u16, gone: &[String]) -> Vec<ScannedFile> {
        let removed = |lower: &str| {
            gone.iter().any(|gone| {
                lower
                    .strip_prefix(gone.as_str())
                    .is_some_and(|rest| gone.is_empty() || rest.is_empty() || rest.starts_with('/'))
            })
        };
        let start = self.entries.partition_point(|entry| entry.root < root);
        self.entries[start..]
            .iter()
            .take_while(|entry| entry.root == root)
            .filter(|entry| !removed(&entry.lower))
            .map(|entry| ScannedFile { rel: entry.rel.clone(), kind: entry.kind, size: entry.size, mtime: entry.mtime })
            .collect()
    }

    /// Which root `path` is in, and its lowercase path relative to that root ("" for the root
    /// itself).
    pub fn place_of(&self, path: &Path) -> Option<(u16, String)> {
        let key = path_key(path);
        self.roots.iter().enumerate().find_map(|(index, root)| {
            let rest = key.strip_prefix(root.path_key.as_str())?;
            let rel = if rest.is_empty() { "" } else { rest.strip_prefix('/')? };
            Some((index as u16, rel.to_owned()))
        })
    }

    /// The root `path` is, if it is one (by `root_string`).
    pub fn root_index(&self, path: &Path) -> Option<u16> {
        let key = root_string(path);
        self.roots.iter().position(|root| root.key == key).map(|index| index as u16)
    }

    /// The entry's folder for showing in a list: relative to its root, after the root's name
    /// when the library has several.
    pub fn folder_label(&self, entry: &Entry) -> String {
        let folder = entry.folder();
        if self.roots.len() < 2 {
            return folder.to_owned();
        }
        let root = &self.roots[entry.root as usize].name;
        if folder.is_empty() { root.clone() } else { format!("{root}/{folder}") }
    }

    /// The folder whose `DirNode::key` (lowercase absolute path, "" for the top) is `key`.
    pub fn find_dir(&self, key: &str) -> Option<u32> {
        self.dirs.iter().position(|node| node.key == key).map(|index| index as u32)
    }

    /// The folder's path on disk (`None` for the top, which is all the roots).
    pub fn dir_path(&self, dir: u32) -> Option<PathBuf> {
        let chain = self.dir_chain(dir);
        let root = self.dirs.get(dir as usize)?.root?;
        // The chain is the top, the root, then the folders down to `dir`, each with its own name.
        let mut path = self.roots[root as usize].path.clone();
        for &index in chain.iter().skip(2) {
            path.push(&self.dirs[index as usize].name);
        }
        Some(path)
    }

    /// `dir` and its parents, root first.
    pub fn dir_chain(&self, dir: u32) -> Vec<u32> {
        let mut chain = Vec::new();
        let mut node = Some(dir);
        while let Some(index) = node {
            chain.push(index);
            node = self.dirs.get(index as usize).and_then(|dir| dir.parent);
        }
        chain.reverse();
        chain
    }

    /// Index of each entry by its cache key, for carrying selections over to a rescanned library.
    pub fn index_by_key(&self) -> HashMap<i64, u32> {
        self.entries.iter().enumerate().map(|(index, entry)| (entry.key, index as u32)).collect()
    }
}

fn dir_index(
    root: u16,
    root_key: &str,
    rel_dir: &str,
    dirs: &mut Vec<DirNode>,
    by_path: &mut HashMap<(u16, String), u32>,
) -> u32 {
    if let Some(&index) = by_path.get(&(root, rel_dir.to_owned())) {
        return index;
    }
    let (parent, name) = match rel_dir.rfind('/') {
        None => (Library::root_dir(root), rel_dir),
        Some(slash) => (dir_index(root, root_key, &rel_dir[..slash], dirs, by_path), &rel_dir[slash + 1..]),
    };
    let index = dirs.len() as u32;
    let lower = rel_dir.to_lowercase();
    dirs.push(DirNode {
        name: name.to_owned(),
        key: format!("{root_key}/{lower}"),
        lower,
        root: Some(root),
        parent: Some(parent),
        children: Vec::new(),
        totals: [0; 3],
    });
    dirs[parent as usize].children.push(index);
    by_path.insert((root, rel_dir.to_owned()), index);
    index
}

/// The form of a root path used as its identity in the index and cache keys.
pub fn root_string(root: &Path) -> String {
    let text = root.to_string_lossy();
    let trimmed = text.trim_end_matches(['/', '\\']);
    // Keep "C:\" rather than "C:" so a drive root stays a valid path.
    if trimmed.ends_with(':') { text.into_owned() } else { trimmed.to_owned() }
}

/// A file's or folder's identity across the app: its absolute path with `/` separators, in
/// lowercase, without a trailing separator. Favorites, collections and categories hold files by
/// it, and folders are found again by it after a rescan.
pub fn path_key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_lowercase()
}

/// The last part of a path ("Resources"), or the whole path for a drive ("F:\").
pub fn folder_name(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

/// Whether `inner` is `outer` or inside it.
pub fn is_within(inner: &Path, outer: &Path) -> bool {
    let (inner, outer) = (path_key(inner), path_key(outer));
    inner == outer || inner.strip_prefix(&outer).is_some_and(|rest| rest.starts_with('/'))
}

/// Adds `path` to a library's folders, keeping each file in one root: a folder that's in the
/// library already (a root, or inside one) isn't added, and `Err` has the root it's in. Otherwise
/// it goes in, in place of any roots inside it, and `Ok` has those.
pub fn add_root(roots: &mut Vec<PathBuf>, path: PathBuf) -> Result<Vec<PathBuf>, usize> {
    if let Some(index) = roots.iter().position(|root| is_within(&path, root)) {
        return Err(index);
    }
    let at = roots.iter().position(|root| is_within(root, &path)).unwrap_or(roots.len());
    let (inside, kept): (Vec<PathBuf>, Vec<PathBuf>) = roots.drain(..).partition(|root| is_within(root, &path));
    *roots = kept;
    roots.insert(at, path);
    Ok(inside)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(rel: &str, kind: AssetKind, size: u64, mtime: i64) -> ScannedFile {
        ScannedFile { rel: rel.to_owned(), kind, size, mtime }
    }

    fn library() -> Library {
        Library::build(vec![(
            PathBuf::from("/assets"),
            vec![
                file("ui/b.png", AssetKind::Image, 30, 3),
                file("ui/A.png", AssetKind::Image, 10, 1),
                file("sfx/hit.wav", AssetKind::Audio, 20, 2),
                file("sfx/deep/boom.ogg", AssetKind::Audio, 20, 5),
                file("rock.fbx", AssetKind::Model, 40, 4),
            ],
        )])
    }

    fn names(library: &Library, view: &[u32]) -> Vec<String> {
        view.iter().map(|&index| library.entries[index as usize].name().to_owned()).collect()
    }

    #[test]
    fn sorts_with_ties_in_library_order() {
        let library = library();
        let all = library.filter(0, KindSet::ALL, "");
        assert_eq!(names(&library, &all), ["rock.fbx", "boom.ogg", "hit.wav", "A.png", "b.png"]);

        let mut view = all.clone();
        library.sort(&mut view, SortBy::Name, false);
        assert_eq!(names(&library, &view), ["A.png", "b.png", "boom.ogg", "hit.wav", "rock.fbx"]);
        library.sort(&mut view, SortBy::Size, true);
        assert_eq!(
            names(&library, &view),
            ["rock.fbx", "b.png", "boom.ogg", "hit.wav", "A.png"],
            "ties stay in path order"
        );
        library.sort(&mut view, SortBy::Type, false);
        assert_eq!(names(&library, &view), ["rock.fbx", "boom.ogg", "A.png", "b.png", "hit.wav"]);
        library.sort(&mut view, SortBy::Modified, true);
        assert_eq!(names(&library, &view), ["boom.ogg", "rock.fbx", "b.png", "hit.wav", "A.png"]);
        library.sort(&mut view, SortBy::Folder, false);
        assert_eq!(view, all);
    }

    #[test]
    fn folders_count_each_kind() {
        let library = library();
        let sfx = library.find_dir("/assets/sfx").unwrap();
        let deep = library.find_dir("/assets/sfx/deep").unwrap();
        assert_eq!(library.dirs[sfx as usize].totals, [0, 2, 0]);
        assert_eq!(library.dirs[0].total(KindSet::of(&[AssetKind::Image, AssetKind::Model])), 3);
        assert_eq!(library.home(), 1, "with one root, the tree starts at it");
        assert_eq!(library.dir_chain(deep), [0, 1, sfx, deep]);
        assert_eq!(library.dir_path(deep), Some(PathBuf::from("/assets/sfx/deep")));
        assert_eq!(library.dir_path(1), Some(PathBuf::from("/assets")));
        assert_eq!(library.dir_path(0), None, "the top is every root");
        assert_eq!(library.filter(sfx, KindSet::only(AssetKind::Audio), "boom").len(), 1);
        let by_key = library.index_by_key();
        assert!(library.entries.iter().enumerate().all(|(index, entry)| by_key[&entry.key] == index as u32));
    }

    #[test]
    #[expect(clippy::cognitive_complexity, reason = "refactor: split into smaller tests")]
    fn several_roots_make_one_library() {
        // Forward slashes, which every OS splits paths on, so the second root is named "Audio" on
        // Linux and macOS too.
        let library = Library::build(vec![
            (PathBuf::from("F:\\Resources"), vec![file("ui/a.png", AssetKind::Image, 1, 1)]),
            (
                PathBuf::from("D:/Audio"),
                vec![file("ui/a.png", AssetKind::Image, 1, 1), file("hit.wav", AssetKind::Audio, 1, 1)],
            ),
        ]);
        assert_eq!(library.home(), 0, "with several roots, the tree starts above them");
        assert_eq!(library.dirs[0].totals, [2, 1, 0]);
        assert_eq!(library.dirs[Library::root_dir(1) as usize].name, "Audio");
        // The same relative path in two roots is two files, in two folders.
        let ui_resources = library.find_dir("f:/resources/ui").unwrap();
        let ui_audio = library.find_dir("d:/audio/ui").unwrap();
        assert_ne!(ui_resources, ui_audio);
        assert_eq!(library.filter(ui_audio, KindSet::ALL, "").len(), 1);
        assert_eq!(library.filter(Library::root_dir(1), KindSet::ALL, "").len(), 2);
        assert_eq!(library.filter(0, KindSet::ALL, "").len(), 3);
        let (a, b) = (library.find(0, "ui/a.png").unwrap(), library.find(1, "ui/a.png").unwrap());
        assert_ne!(
            library.entries[a as usize].key, library.entries[b as usize].key,
            "each root's files have their own cache keys"
        );

        let found = library.locate("d:/audio/hit.wav");
        assert!(matches!(found, Located::Found(index) if library.entries[index as usize].rel == "hit.wav"));
        assert_eq!(library.locate("d:/audio/gone.wav"), Located::Missing);
        assert_eq!(library.locate("c:/elsewhere/x.png"), Located::Outside);
        let hit = &library.entries[library.find(1, "hit.wav").unwrap() as usize];
        assert_eq!(library.path_of(hit), Path::new("D:/Audio").join("hit.wav"));
        assert_eq!(library.folder_label(hit), "Audio");
        assert_eq!(library.root_index(Path::new("D:/Audio/")), Some(1));

        // Taking a root's files back out gives what built it, so the library can be rebuilt
        // without it, or with it rescanned.
        let audio: Vec<String> = library.files_of(1).into_iter().map(|file| file.rel).collect();
        assert_eq!(audio, ["hit.wav", "ui/a.png"]);
        let without = Library::build(vec![(PathBuf::from("F:\\Resources"), library.files_of(0))]);
        assert_eq!(without.entries.len(), 1);
        assert!(!without.same_files_as(&library));
        let again = Library::build(vec![
            (PathBuf::from("F:\\Resources"), library.files_of(0)),
            (PathBuf::from("D:/Audio"), library.files_of(1)),
        ]);
        assert!(again.same_files_as(&library));
    }

    #[test]
    fn deleted_files_and_folders_leave_the_rest() {
        let library = Library::build(vec![(
            PathBuf::from("F:\\Art"),
            vec![
                file("ui/A.png", AssetKind::Image, 1, 1),
                file("ui/icons/b.png", AssetKind::Image, 1, 1),
                file("uix/c.png", AssetKind::Image, 1, 1),
                file("d.png", AssetKind::Image, 1, 1),
            ],
        )]);
        assert_eq!(library.place_of(Path::new("f:\\art\\UI")), Some((0, "ui".to_owned())));
        assert_eq!(library.place_of(Path::new("F:\\Art")), Some((0, String::new())));
        assert_eq!(library.place_of(Path::new("F:\\Artwork\\x.png")), None, "a name starting the same isn't inside");
        let rels = |files: Vec<ScannedFile>| files.into_iter().map(|file| file.rel).collect::<Vec<_>>();
        // A folder takes what's below it, but not a sibling whose name starts the same.
        assert_eq!(rels(library.files_without(0, &["ui".to_owned()])), ["d.png", "uix/c.png"]);
        assert_eq!(
            rels(library.files_without(0, &["ui/a.png".to_owned(), "d.png".to_owned()])),
            ["ui/icons/b.png", "uix/c.png"]
        );
    }

    #[test]
    fn each_file_is_in_one_root() {
        assert!(is_within(Path::new("F:/resources/2D"), Path::new("F:\\Resources")));
        assert!(is_within(Path::new("F:\\Resources\\"), Path::new("f:\\resources")));
        assert!(is_within(Path::new("F:\\Resources"), Path::new("F:\\")));
        assert!(!is_within(Path::new("F:\\Resources2"), Path::new("F:\\Resources")));
        assert!(!is_within(Path::new("F:\\Resources"), Path::new("F:\\Resources\\2D")));
        assert!(!is_within(Path::new("D:\\A"), Path::new("F:\\A")));

        let mut roots = vec![PathBuf::from("F:\\Resources")];
        assert_eq!(add_root(&mut roots, PathBuf::from("D:\\Audio")), Ok(vec![]));
        assert_eq!(add_root(&mut roots, PathBuf::from("f:\\resources")), Err(0), "already a root");
        assert_eq!(add_root(&mut roots, PathBuf::from("F:\\Resources\\2D")), Err(0), "inside a root");
        assert_eq!(roots.len(), 2);
        // A folder holding roots takes their place (where the first of them was).
        assert_eq!(add_root(&mut roots, PathBuf::from("F:\\")), Ok(vec![PathBuf::from("F:\\Resources")]));
        assert_eq!(roots, [PathBuf::from("F:\\"), PathBuf::from("D:\\Audio")]);
    }
}
