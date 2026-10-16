//! Building the library from what the scan found: its entries in order, and the folder tree over
//! them with each folder's counts.

use std::collections::HashMap;
use std::path::PathBuf;

use super::{DirNode, Entry, Library, Root, folder_name, path_key, root_string};
use crate::preview_key::cache_key;
use crate::scan::ScannedFile;

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
