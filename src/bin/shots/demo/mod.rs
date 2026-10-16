//! A small library of made-up game assets, drawn and synthesised here, so the README's pictures
//! show what the app does without anyone's own files: item icons, tiles, backgrounds, interface
//! parts, sprite sheets and GIFs, sound effects and music, and 3D models, with a few favorites,
//! collections and categories.

use std::fs;
use std::io;
use std::path::Path;

use asset_browser::groups::{FAVORITES, GroupKind, Groups};
use asset_browser::paths;

mod backgrounds;
mod characters;
mod interface;
mod items;
mod models;
mod paint;
mod sounds;
mod tiles;

use backgrounds::backgrounds;
use characters::characters;
use interface::interface;
use items::{gem, items, potion};
use models::models;
use paint::hex;
use sounds::sounds;
use tiles::tiles;

/// Left in a folder holding the library, so it can be written there again.
const MARK: &str = ".made-up-assets";

/// Writes the library into `root`: an empty folder, or one written before. Anything else in it
/// would be in the pictures, and nothing is deleted here but what a run before added (`INCOMING`).
pub fn write(root: &Path) -> io::Result<()> {
    let taken = fs::read_dir(root).is_ok_and(|mut entries| entries.next().is_some());
    if taken && !root.join(MARK).exists() {
        let message =
            format!("{} has other things in it, which the pictures would show: give an empty folder", root.display());
        return Err(io::Error::other(message));
    }
    fs::create_dir_all(root)?;
    fs::write(
        root.join(MARK),
        "Made up for the README's pictures by src/bin/shots/demo.rs, which writes them again here.\n",
    )?;
    // What arrived during the last run arrives again.
    for file in INCOMING {
        let _ = fs::remove_file(root.join(file));
    }
    items(&root.join("Items"))?;
    tiles(&root.join("Environment/Tiles"))?;
    backgrounds(&root.join("Environment/Backgrounds"))?;
    interface(&root.join("UI"))?;
    characters(&root.join("Characters"))?;
    sounds(&root.join("Audio"))?;
    models(&root.join("Models"))
}

/// Files that arrive while the app is open, one at a time, for the clip of the library keeping
/// itself current.
pub const INCOMING: [&str; 4] =
    ["Items/gem_amber.png", "Items/gem_amethyst.png", "Items/potion_purple.png", "Items/potion_yellow.png"];

/// Writes the `i`th of `INCOMING` into the library in `root`.
pub fn arrive(root: &Path, i: usize) -> io::Result<()> {
    let canvas = match i {
        0 => gem(hex(0xf29a2e)),
        1 => gem(hex(0x9b5cf0)),
        2 => potion(hex(0xb16cff)),
        _ => potion(hex(0xf5c542)),
    };
    canvas.save(&root.join(INCOMING[i.min(INCOMING.len() - 1)]))
}

/// Favorites, collections and categories over the library in `root`, in the app's user data.
#[expect(clippy::too_many_lines, reason = "refactor: split into named steps")]
pub fn organize(root: &Path) {
    let mut groups = Groups::open(&paths::user_data_file());
    let paths = |files: &[&str]| files.iter().map(|file| root.join(file)).collect::<Vec<_>>();
    groups.add(
        FAVORITES,
        &paths(&[
            "Items/gem_ruby.png",
            "Items/potion_red.png",
            "Characters/slime_bounce.png",
            "Audio/Music/menu_theme.wav",
            "Models/crystal.obj",
        ]),
    );
    let collections: [(&str, &[&str]); 2] = [
        (
            "Forest level",
            &[
                "Environment/Tiles/grass.png",
                "Environment/Backgrounds/forest_day.png",
                "Models/tree.obj",
                "Models/mushroom.obj",
                "Items/mushroom.png",
                "Audio/SFX/footstep_grass.wav",
                "Audio/Music/ambience_forest.wav",
            ],
        ),
        (
            "Boss fight",
            &[
                "Audio/Music/battle_loop.wav",
                "Audio/SFX/explosion.wav",
                "Environment/Tiles/lava.png",
                "Items/bomb.png",
                "Items/sword.png",
                "Items/shield.png",
                "Characters/flame.gif",
            ],
        ),
    ];
    for (name, files) in collections {
        if let Some(id) = groups.create(GroupKind::Collection, name) {
            groups.add(id, &paths(files));
        }
    }
    let categories: [(&str, u8, &[&str]); 4] = [
        (
            "Pickups",
            2,
            &[
                "Items/coin.png",
                "Items/gem_ruby.png",
                "Items/gem_emerald.png",
                "Items/gem_sapphire.png",
                "Items/potion_red.png",
                "Items/potion_blue.png",
                "Items/potion_green.png",
                "Items/heart.png",
                "Items/key.png",
                "Items/star.png",
                "Characters/coin_strip8.png",
                "Audio/SFX/coin.wav",
                "Audio/SFX/powerup.wav",
            ],
        ),
        ("Enemies", 0, &["Characters/slime_bounce.png", "Characters/flame.gif", "Audio/SFX/hit.wav"]),
        (
            "Interface",
            4,
            &[
                "UI/button_play.png",
                "UI/button_pause.png",
                "UI/panel.png",
                "UI/health_bar.png",
                "UI/cursor.png",
                "Audio/SFX/click.wav",
            ],
        ),
        (
            "Magic",
            6,
            &[
                "Models/crystal.obj",
                "Audio/SFX/magic_spell.wav",
                "Items/potion_blue.png",
                "Characters/portal.gif",
                "Items/gem_sapphire.png",
            ],
        ),
    ];
    for (name, color, files) in categories {
        if let Some(id) = groups.create(GroupKind::Category, name) {
            groups.set_color(id, color);
            groups.add(id, &paths(files));
        }
    }
}
