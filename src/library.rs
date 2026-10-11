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
}

pub struct DirNode {
    pub name: String,
    /// Lowercase path relative to the root ("" for the root itself).
    pub lower: String,
    pub children: Vec<u32>,
    /// Assets in this folder and everything below it.
    pub total: u32,
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
                key: cache_key(&root_str, &file.rel, file.size, file.mtime),
                rel: file.rel,
                kind: file.kind,
                size: file.size,
                mtime: file.mtime,
                dir: 0,
            })
            .collect();
        entries.sort_unstable_by(|a, b| a.lower.cmp(&b.lower));

        let mut dirs = vec![DirNode { name: String::new(), lower: String::new(), children: Vec::new(), total: 0 }];
        let mut parents = vec![None];
        let mut by_path: HashMap<String, u32> = HashMap::new();
        let mut kind_totals = [0; 3];
        for entry in &mut entries {
            kind_totals[entry.kind.index()] += 1;
            let dir = match entry.rel.rfind('/') {
                None => 0,
                Some(slash) => dir_index(&entry.rel[..slash], &mut dirs, &mut parents, &mut by_path),
            };
            entry.dir = dir;
            let mut node = Some(dir);
            while let Some(index) = node {
                dirs[index as usize].total += 1;
                node = parents[index as usize];
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

    /// Same set of files with the same versions: a rescan found nothing new.
    pub fn same_files_as(&self, other: &Library) -> bool {
        self.entries.len() == other.entries.len()
            && self.entries.iter().zip(&other.entries).all(|(a, b)| a.key == b.key)
    }

    pub fn find_dir(&self, lower: &str) -> Option<u32> {
        self.dirs.iter().position(|node| node.lower == lower).map(|index| index as u32)
    }
}

fn dir_index(
    rel_dir: &str,
    dirs: &mut Vec<DirNode>,
    parents: &mut Vec<Option<u32>>,
    by_path: &mut HashMap<String, u32>,
) -> u32 {
    if let Some(&index) = by_path.get(rel_dir) {
        return index;
    }
    let (parent, name) = match rel_dir.rfind('/') {
        None => (0, rel_dir),
        Some(slash) => (dir_index(&rel_dir[..slash], dirs, parents, by_path), &rel_dir[slash + 1..]),
    };
    let index = dirs.len() as u32;
    dirs.push(DirNode { name: name.to_owned(), lower: rel_dir.to_lowercase(), children: Vec::new(), total: 0 });
    parents.push(Some(parent));
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
