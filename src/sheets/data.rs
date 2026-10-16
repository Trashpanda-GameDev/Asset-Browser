//! Sprite sheet data beside a picture, as Aseprite and TexturePacker write it.

use std::collections::BTreeMap;
use std::path::Path;

use super::{Animation, Frame, Sheet, Source};

/// Reads `name.json` (or `name.png.json`) beside the picture, if it's sprite sheet data.
pub(super) fn from_data_file(path: &Path) -> Option<Sheet> {
    let stem = path.file_stem()?.to_string_lossy().into_owned();
    let file_name = path.file_name()?.to_string_lossy().into_owned();
    let dir = path.parent()?;
    let candidates = [dir.join(format!("{stem}.json")), dir.join(format!("{file_name}.json"))];
    let (data_path, text) = candidates
        .iter()
        .find_map(|candidate| std::fs::read_to_string(candidate).ok().map(|text| (candidate, text)))?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let data_name = data_path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    parse_data(&json, &data_name)
}

/// Aseprite / TexturePacker JSON: `frames` as an array or an object of named frames, and
/// optional `meta.frameTags`.
pub(super) fn parse_data(json: &serde_json::Value, data_name: &str) -> Option<Sheet> {
    let entries: Vec<(String, &serde_json::Value)> = match json.get("frames")? {
        serde_json::Value::Array(list) => list
            .iter()
            .map(|frame| (frame.get("filename").and_then(|v| v.as_str()).unwrap_or("").to_owned(), frame))
            .collect(),
        serde_json::Value::Object(map) => map.iter().map(|(name, frame)| (name.clone(), frame)).collect(),
        _ => return None,
    };
    let number = |value: Option<&serde_json::Value>, key: &str| -> Option<u32> {
        value?.get(key)?.as_f64().map(|n| n.max(0.0).round() as u32)
    };
    let mut frames = Vec::new();
    let mut names = Vec::new();
    for (name, frame) in entries {
        let rect = frame.get("frame");
        let [x, y, w, h] = ["x", "y", "w", "h"].map(|key| number(rect, key));
        let (Some(x), Some(y), Some(w), Some(h)) = (x, y, w, h) else { continue };
        if w == 0 || h == 0 {
            continue;
        }
        let trimmed = frame.get("spriteSourceSize");
        let source = frame.get("sourceSize");
        let offset = [number(trimmed, "x").unwrap_or(0), number(trimmed, "y").unwrap_or(0)];
        let size = [number(source, "w").unwrap_or(w).max(w), number(source, "h").unwrap_or(h).max(h)];
        let rotated = frame.get("rotated").and_then(|v| v.as_bool()).unwrap_or(false);
        let duration_ms = frame.get("duration").and_then(|v| v.as_f64()).map(|d| d.max(1.0).round() as u32);
        frames.push(Frame { rect: [x, y, w, h], rotated, offset, size, duration_ms });
        names.push(name);
    }
    if frames.is_empty() {
        return None;
    }

    let mut animations = Vec::new();
    if let Some(tags) = json.get("meta").and_then(|meta| meta.get("frameTags")).and_then(|tags| tags.as_array()) {
        for tag in tags {
            let name = tag.get("name").and_then(|v| v.as_str()).unwrap_or("").to_owned();
            let (Some(from), Some(to)) = (number(Some(tag), "from"), number(Some(tag), "to")) else { continue };
            let last = frames.len() as u32 - 1;
            let (from, to) = (from.min(last) as usize, to.min(last) as usize);
            let forward: Vec<usize> = (from.min(to)..=from.max(to)).collect();
            let order = match tag.get("direction").and_then(|v| v.as_str()).unwrap_or("forward") {
                "reverse" => forward.into_iter().rev().collect(),
                "pingpong" if forward.len() > 2 => {
                    let back: Vec<usize> = forward[1..forward.len() - 1].iter().rev().copied().collect();
                    forward.into_iter().chain(back).collect()
                }
                _ => forward,
            };
            animations.push(Animation { name, frames: order });
        }
    }
    if animations.is_empty() {
        animations = animations_by_name(&names);
    }
    if animations.is_empty() {
        animations = vec![Animation { name: "All frames".to_owned(), frames: (0..frames.len()).collect() }];
    }
    Some(Sheet { frames, animations, source: Source::Data(data_name.to_owned()) })
}

/// Atlas frames named `run-1`, `run-2`, `bat/fly_01`…: grouped by the name without its
/// number, in number order. Only worth it when some name has more than one frame.
pub(super) fn animations_by_name(names: &[String]) -> Vec<Animation> {
    let mut groups: BTreeMap<String, Vec<(u64, usize)>> = BTreeMap::new();
    for (index, name) in names.iter().enumerate() {
        let base = name.rsplit_once('.').filter(|(_, ext)| ext.len() <= 4).map_or(name.as_str(), |(base, _)| base);
        let digits = base.chars().rev().take_while(char::is_ascii_digit).count();
        let (stem, number) = base.split_at(base.len() - digits);
        let stem = stem.trim_end_matches(['-', '_', ' ', '.']);
        groups.entry(stem.to_owned()).or_default().push((number.parse().unwrap_or(0), index));
    }
    if !groups.values().any(|frames| frames.len() > 1) {
        return Vec::new();
    }
    groups
        .into_iter()
        .map(|(stem, mut frames)| {
            frames.sort_unstable();
            let name = stem.rsplit('/').next().unwrap_or(&stem).to_owned();
            let name = if name.is_empty() { "Frames".to_owned() } else { name };
            Animation { name, frames: frames.into_iter().map(|(_, index)| index).collect() }
        })
        .collect()
}
