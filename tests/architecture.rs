//! Checks the rules in docs/architecture.md on the source code: every module uses only modules in its own layer or
//! below, no modules depend on each other in a circle, nothing below `ui` uses egui, and views read no files, start
//! no threads and open no databases.
//!
//! What breaks a rule today is listed in the exceptions below. Each must still be true, so once the code is fixed
//! the test fails until its exception is taken out too.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// The layers, lowest first, and the top-level modules in each.
const LAYERS: [(&str, &[&str]); 5] = [
    ("foundation", &["paths", "db", "diag", "platform", "work"]),
    (
        "domain",
        &[
            "asset",
            "library",
            "scan",
            "groups",
            "sheets",
            "model",
            "imaging",
            "preview_key",
            "folder_index",
            "animated",
            "changelog",
        ],
    ),
    (
        "services",
        &["rescan", "thumbs", "audio", "playback", "detail", "deletion", "updates", "watch", "storage", "memory"],
    ),
    ("ui", &["ui"]),
    ("app", &["app"]),
];

/// Views that do what the app should: (file in src/, what it does).
const VIEW_EXCEPTIONS: [(&str, Work); 1] = [("ui/theme.rs", Work::Files)];

/// The crates a module below `ui` must not use.
const UI_CRATES: [&str; 4] = ["egui", "eframe", "egui_phosphor", "wgpu"];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Work {
    Files,
    Threads,
    Databases,
}

impl Work {
    const ALL: [Work; 3] = [Work::Files, Work::Threads, Work::Databases];

    /// The paths that start this kind of work.
    fn paths(self) -> &'static [&'static str] {
        match self {
            Work::Files => &["fs::"],
            Work::Threads => &["thread::", "work::spawn"],
            Work::Databases => &["rusqlite::", "Connection::"],
        }
    }
}

/// One source file of the crate's library: its path under src/, its top-level module, and its code without
/// comments or tests.
struct Source {
    path: String,
    module: String,
    code: String,
}

#[test]
fn every_module_has_a_layer() {
    let declared = declared_modules();
    let layered: BTreeSet<String> =
        LAYERS.iter().flat_map(|(_, modules)| modules.iter()).map(|m| m.to_string()).collect();
    assert_eq!(declared, layered, "src/lib.rs's modules and the layers in this test (and docs/architecture.md)");
}

#[test]
fn modules_use_only_their_own_layer_or_below() {
    let upward: Vec<(String, String)> = module_uses()
        .into_iter()
        .flat_map(|(module, used)| used.into_iter().map(move |target| (module.clone(), target)))
        .filter(|(module, target)| layer(target) > layer(module))
        .collect();
    assert!(upward.is_empty(), "modules using a higher layer (docs/architecture.md): {upward:?}");
}

#[test]
fn modules_do_not_depend_on_each_other_in_a_circle() {
    let circle = find_circle(&module_uses());
    assert!(circle.is_none(), "modules that depend on each other in a circle: {circle:?}");
}

#[test]
fn nothing_below_ui_uses_egui() {
    let ui = layer("ui");
    let found: BTreeSet<String> = sources()
        .into_iter()
        .filter(|source| layer(&source.module) < ui)
        .filter(|source| UI_CRATES.iter().any(|name| uses_path(&source.code, &format!("{name}::"))))
        .map(|source| source.path)
        .collect();
    assert!(found.is_empty(), "files below the ui layer that use egui: {found:?}");
}

#[test]
fn views_read_no_files_start_no_threads_and_open_no_databases() {
    let found: Vec<(String, Work)> =
        view_work().into_iter().filter(|(path, work)| !VIEW_EXCEPTIONS.contains(&(path.as_str(), *work))).collect();
    assert!(found.is_empty(), "views doing work the app should do: {found:?}");
}

#[test]
fn every_exception_is_still_needed() {
    let view_work = view_work();
    let fixed: Vec<String> = VIEW_EXCEPTIONS
        .iter()
        .filter(|(path, work)| !view_work.iter().any(|(found, did)| found == path && did == work))
        .map(|(path, work)| format!("{path} does {work:?}"))
        .collect();
    assert!(fixed.is_empty(), "fixed, so take these out of the exceptions: {fixed:?}");
}

fn layer(module: &str) -> usize {
    LAYERS
        .iter()
        .position(|(_, modules)| modules.contains(&module))
        .unwrap_or_else(|| panic!("{module} isn't in a layer"))
}

fn src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// The top-level modules src/lib.rs declares.
fn declared_modules() -> BTreeSet<String> {
    let lib = without_tests(&fs::read_to_string(src().join("lib.rs")).expect("read src/lib.rs"));
    lib.lines()
        .filter_map(|line| line.split_once("mod "))
        .filter(|(visibility, _)| visibility.trim().is_empty() || visibility.trim().starts_with("pub"))
        .map(|(_, name)| name.trim().trim_end_matches(';').to_owned())
        .collect()
}

/// Every file of the library's modules (not the programs, and not the modules only tests have), with comments and
/// tests taken out.
fn sources() -> Vec<Source> {
    let declared = declared_modules();
    let mut files = Vec::new();
    collect_rs_files(&src(), &mut files);
    files
        .into_iter()
        .filter_map(|file| {
            let path = file.strip_prefix(src()).ok()?.to_string_lossy().replace('\\', "/");
            let module = path.split('/').next()?.trim_end_matches(".rs").to_owned();
            if !declared.contains(&module) {
                return None;
            }
            let code = without_tests(&fs::read_to_string(&file).ok()?);
            Some(Source { path, module, code })
        })
        .collect()
}

fn collect_rs_files(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("list src/").flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
}

/// The text without comments and without `#[cfg(test)]` items.
fn without_tests(text: &str) -> String {
    let mut code = String::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if line.trim() == "#[cfg(test)]" {
            skip_item(&mut lines);
        } else {
            code.push_str(without_comment(line));
            code.push('\n');
        }
    }
    code
}

/// Skips the lines of an item: to its closing brace, or to its `;` when it has no body.
fn skip_item<'a>(lines: &mut impl Iterator<Item = &'a str>) {
    let mut depth = 0;
    for line in lines {
        let code = without_comment(line);
        depth += code.matches('{').count() as i64 - code.matches('}').count() as i64;
        if depth <= 0 && (code.contains('}') || code.trim_end().ends_with(';')) {
            return;
        }
    }
}

fn without_comment(line: &str) -> &str {
    line.find("//").map_or(line, |at| &line[..at])
}

/// Which top-level modules each module uses, through `crate::` paths.
fn module_uses() -> BTreeMap<String, BTreeSet<String>> {
    let mut uses: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for source in sources() {
        let used = uses.entry(source.module.clone()).or_default();
        used.extend(crate_paths(&source.code).into_iter().filter(|target| *target != source.module));
    }
    uses
}

/// The first part of every `crate::` path: `crate::a::b` and `crate::{a, b::c}` name `a` and `b`.
fn crate_paths(code: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for (at, _) in code.match_indices("crate::") {
        if !starts_word(code, at) {
            continue;
        }
        let rest = &code[at + "crate::".len()..];
        if let Some(group) = rest.strip_prefix('{') {
            names.extend(top_level_items(group).map(|item| first_segment(item).to_owned()));
        } else {
            names.insert(first_segment(rest).to_owned());
        }
    }
    names.retain(|name| !name.is_empty() && name != "self");
    names
}

/// The comma-separated items of a `{...}` group, whose opening brace is already taken off.
fn top_level_items(group: &str) -> impl Iterator<Item = &str> {
    let mut depth = 0;
    let end = group
        .char_indices()
        .find(|&(_, c)| {
            depth += match c {
                '{' => 1,
                '}' => -1,
                _ => 0,
            };
            depth < 0
        })
        .map_or(group.len(), |(at, _)| at);
    let mut items = Vec::new();
    let (mut start, mut depth) = (0, 0);
    for (at, c) in group[..end].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 0 => {
                items.push(&group[start..at]);
                start = at + 1;
            }
            _ => {}
        }
    }
    items.push(&group[start..end]);
    items.into_iter().map(str::trim)
}

fn first_segment(path: &str) -> &str {
    let end = path.find(|c: char| !is_word(c)).unwrap_or(path.len());
    &path[..end]
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Whether the text at `at` starts a word, rather than continuing one (`my_crate::` isn't `crate::`).
fn starts_word(code: &str, at: usize) -> bool {
    !code[..at].chars().next_back().is_some_and(is_word)
}

/// Whether the code names a path starting with `prefix`, such as `fs::` or `egui::`.
fn uses_path(code: &str, prefix: &str) -> bool {
    code.match_indices(prefix).any(|(at, _)| starts_word(code, at))
}

/// The work each view does: (file in src/, what it does).
fn view_work() -> Vec<(String, Work)> {
    sources()
        .into_iter()
        .filter(|source| source.module == "ui")
        .flat_map(|source| {
            let done: Vec<Work> = Work::ALL
                .into_iter()
                .filter(|work| work.paths().iter().any(|path| uses_path(&source.code, path)))
                .collect();
            done.into_iter().map(move |work| (source.path.clone(), work))
        })
        .collect()
}

/// A circle of modules each using the next, if there is one.
fn find_circle(uses: &BTreeMap<String, BTreeSet<String>>) -> Option<Vec<String>> {
    let mut done = BTreeSet::new();
    uses.keys().find_map(|start| walk(uses, start, &mut Vec::new(), &mut done))
}

/// Depth first from `module`; `path` is how it was reached.
fn walk(
    uses: &BTreeMap<String, BTreeSet<String>>,
    module: &str,
    path: &mut Vec<String>,
    done: &mut BTreeSet<String>,
) -> Option<Vec<String>> {
    if let Some(at) = path.iter().position(|seen| seen == module) {
        return Some(path[at..].to_vec());
    }
    if !done.insert(module.to_owned()) {
        return None;
    }
    path.push(module.to_owned());
    let circle = uses.get(module).into_iter().flatten().find_map(|next| walk(uses, next, path, done));
    path.pop();
    circle
}
