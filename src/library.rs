//! The files of one root folder, sorted, with a folder tree and fast filtering.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::asset::AssetKind;
use crate::scan::ScannedFile;
use crate::thumbs::cache_key;

pub struct Entry {
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
    /// Lowercase path relative to the root ("" for the root itself).
    pub lower: String,
    pub parent: Option<u32>,
    pub children: Vec<u32>,
    /// Assets of each kind in this folder and everything below it.
    pub totals: [u32; 3],
}

impl DirNode {
    /// Assets of the enabled kinds in this folder and below it.
    pub fn total(&self, kinds: [bool; 3]) -> u32 {
        self.totals.iter().zip(kinds).filter(|(_, on)| *on).map(|(n, _)| n).sum()
    }
}

/// The order of the browsed assets. `Folder` is the library's own order: by path, so each
/// folder's files stay together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortBy {
    Folder,
    Name,
    Type,
    Size,
    Modified,
}

impl SortBy {
    pub const ALL: [SortBy; 5] = [Self::Folder, Self::Name, Self::Type, Self::Size, Self::Modified];

    pub fn label(self) -> &'static str {
        match self {
            Self::Folder => "Folder",
            Self::Name => "Name",
            Self::Type => "Type",
            Self::Size => "Size",
            Self::Modified => "Modified",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Folder => "folder",
            Self::Name => "name",
            Self::Type => "type",
            Self::Size => "size",
            Self::Modified => "modified",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|sort| sort.key() == key)
    }
}

pub struct Library {
    pub root: PathBuf,
    /// The root as a string, without a trailing separator. Part of every cache key.
    pub root_str: String,
    pub entries: Vec<Entry>,
    /// `dirs[0]` is the root.
    pub dirs: Vec<DirNode>,
    pub kind_totals: [usize; 3],
}

impl Library {
    pub fn build(root: &Path, files: Vec<ScannedFile>) -> Self {
        let root_str = root_string(root);
        let mut entries: Vec<Entry> = files
            .into_iter()
            .map(|file| Entry {
                lower: file.rel.to_lowercase(),
                key: cache_key(&root_str, &file.rel, file.size, file.mtime, file.kind),
                rel: file.rel,
                kind: file.kind,
                size: file.size,
                mtime: file.mtime,
                dir: 0,
            })
            .collect();
        entries.sort_unstable_by(|a, b| a.lower.cmp(&b.lower));

        let mut dirs = vec![DirNode {
            name: String::new(),
            lower: String::new(),
            parent: None,
            children: Vec::new(),
            totals: [0; 3],
        }];
        let mut by_path: HashMap<String, u32> = HashMap::new();
        let mut kind_totals = [0; 3];
        for entry in &mut entries {
            kind_totals[entry.kind.index()] += 1;
            let dir = match entry.rel.rfind('/') {
                None => 0,
                Some(slash) => dir_index(&entry.rel[..slash], &mut dirs, &mut by_path),
            };
            entry.dir = dir;
            let mut node = Some(dir);
            while let Some(index) = node {
                dirs[index as usize].totals[entry.kind.index()] += 1;
                node = dirs[index as usize].parent;
            }
        }
        for index in 0..dirs.len() {
            let mut children = std::mem::take(&mut dirs[index].children);
            children.sort_unstable_by(|&a, &b| dirs[a as usize].lower.cmp(&dirs[b as usize].lower));
            dirs[index].children = children;
        }

        Self { root: root.to_path_buf(), root_str, entries, dirs, kind_totals }
    }

    pub fn path_of(&self, entry: &Entry) -> PathBuf {
        self.root.join(&entry.rel)
    }

    /// Entries in `dir` (and below it) of the enabled kinds whose path contains every word of
    /// `query` (lowercase, whitespace separated).
    pub fn filter(&self, dir: u32, kinds: [bool; 3], query: &str) -> Vec<u32> {
        let prefix = match self.dirs.get(dir as usize) {
            Some(node) if !node.lower.is_empty() => format!("{}/", node.lower),
            _ => String::new(),
        };
        let words: Vec<&str> = query.split_whitespace().collect();
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                kinds[entry.kind.index()]
                    && entry.lower.starts_with(&prefix)
                    && words.iter().all(|word| entry.lower.contains(word))
            })
            .map(|(index, _)| index as u32)
            .collect()
    }

    /// Puts `view` (indices into `entries`, in library order) in the given order. Ties keep the
    /// library order, whichever the direction.
    pub fn sort(&self, view: &mut [u32], by: SortBy, descending: bool) {
        let entry = |index: &u32| &self.entries[*index as usize];
        view.sort_unstable_by(|a, b| {
            let (x, y) = (entry(a), entry(b));
            let order = match by {
                SortBy::Folder => std::cmp::Ordering::Equal,
                SortBy::Name => x.lower_name().cmp(y.lower_name()),
                SortBy::Type => {
                    x.lower_extension().cmp(y.lower_extension()).then_with(|| x.lower_name().cmp(y.lower_name()))
                }
                SortBy::Size => x.size.cmp(&y.size),
                SortBy::Modified => x.mtime.cmp(&y.mtime),
            };
            let order = if descending { order.reverse() } else { order };
            // Library order breaks ties (and is the whole order for `Folder`).
            order.then(if by == SortBy::Folder && descending { b.cmp(a) } else { a.cmp(b) })
        });
    }

    /// Same set of files with the same versions: a rescan found nothing new.
    pub fn same_files_as(&self, other: &Library) -> bool {
        self.entries.len() == other.entries.len()
            && self.entries.iter().zip(&other.entries).all(|(a, b)| a.key == b.key)
    }

    pub fn find_dir(&self, lower: &str) -> Option<u32> {
        self.dirs.iter().position(|node| node.lower == lower).map(|index| index as u32)
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

fn dir_index(rel_dir: &str, dirs: &mut Vec<DirNode>, by_path: &mut HashMap<String, u32>) -> u32 {
    if let Some(&index) = by_path.get(rel_dir) {
        return index;
    }
    let (parent, name) = match rel_dir.rfind('/') {
        None => (0, rel_dir),
        Some(slash) => (dir_index(&rel_dir[..slash], dirs, by_path), &rel_dir[slash + 1..]),
    };
    let index = dirs.len() as u32;
    dirs.push(DirNode {
        name: name.to_owned(),
        lower: rel_dir.to_lowercase(),
        parent: Some(parent),
        children: Vec::new(),
        totals: [0; 3],
    });
    dirs[parent as usize].children.push(index);
    by_path.insert(rel_dir.to_owned(), index);
    index
}

/// The form of a root path used as its identity in the index and cache keys.
pub fn root_string(root: &Path) -> String {
    let text = root.to_string_lossy();
    let trimmed = text.trim_end_matches(['/', '\\']);
    // Keep "C:\" rather than "C:" so a drive root stays a valid path.
    if trimmed.ends_with(':') { text.into_owned() } else { trimmed.to_owned() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library() -> Library {
        let file = |rel: &str, kind, size, mtime| ScannedFile { rel: rel.to_owned(), kind, size, mtime };
        Library::build(
            Path::new("/assets"),
            vec![
                file("ui/b.png", AssetKind::Image, 30, 3),
                file("ui/A.png", AssetKind::Image, 10, 1),
                file("sfx/hit.wav", AssetKind::Audio, 20, 2),
                file("sfx/deep/boom.ogg", AssetKind::Audio, 20, 5),
                file("rock.fbx", AssetKind::Model, 40, 4),
            ],
        )
    }

    fn names(library: &Library, view: &[u32]) -> Vec<String> {
        view.iter().map(|&index| library.entries[index as usize].name().to_owned()).collect()
    }

    #[test]
    fn sorts_with_ties_in_library_order() {
        let library = library();
        let all = library.filter(0, [true; 3], "");
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
        let sfx = library.find_dir("sfx").unwrap();
        let deep = library.find_dir("sfx/deep").unwrap();
        assert_eq!(library.dirs[sfx as usize].totals, [0, 2, 0]);
        assert_eq!(library.dirs[0].total([true, false, true]), 3);
        assert_eq!(library.dir_chain(deep), [0, sfx, deep]);
        assert_eq!(library.filter(sfx, [false, true, false], "boom").len(), 1);
        let by_key = library.index_by_key();
        assert!(library.entries.iter().enumerate().all(|(index, entry)| by_key[&entry.key] == index as u32));
    }
}
