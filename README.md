# AssetBrowser

A fast, native browser for folders of game assets: add your asset folders and browse their images, sounds and models
together, with previews. Written in Rust with [egui](https://github.com/emilk/egui) on wgpu. Windows first; written to
build on macOS and Linux too, but not yet tried there.

## Run

```
cargo run --release                  # opens the library: the folders you added
cargo run --release -- F:\Resources  # adds this folder to the library (or shows it, if it's in already)
```

## Share it

People get it from the repository's [Releases](https://github.com/Trashpanda-GameDev/Asset-Browser/releases), for
64-bit Windows 10 and 11, in either of two forms. Both update themselves.

- **`TrashpandaAssetBrowser-win-Setup.exe`** installs it for the current user, without admin rights, into
  `%LOCALAPPDATA%\TrashpandaAssetBrowser`, with Start menu and desktop shortcuts, then starts it. It's uninstalled
  from Windows' **Installed apps**.
- **`TrashpandaAssetBrowser-win-Portable.zip`** runs from wherever it's unzipped: start **Asset Browser.exe** in it.

The app checks for a newer release when it starts, every 6 hours, and when asked (**Settings → Updates → Check for
updates**, or a click on its version at the right end of the status bar). It downloads one in the background and
shows **Restart to update**; if you don't, the update is installed when you close the app. The first start after an
update shows **What's new** once; every version's notes are under **Settings → Updates → What's new**. The library,
favorites and settings live in each user's own folders (see [Where things are](#where-things-are)), so updating,
uninstalling or deleting the portable folder leaves them alone. A copy built with `cargo build` doesn't update itself.

Until the app is code-signed, Windows warns about the download ("Windows protected your PC": **More info**,
**Run anyway**); the updates it downloads itself aren't warned about. Builds for macOS and Linux would have to be made
on those systems, and aren't released yet.

The icon is `assets/icon.png` (1024 px); `assets/icon.ico` holds every size Windows uses, made from it.

### Making a release

What's new goes in [`CHANGELOG.md`](CHANGELOG.md) as it's made, under **Unreleased**, written for the people using
the app. Then:

1. In `CHANGELOG.md`, rename `## Unreleased` to the version and date (`## 0.2.0 – 13 Oct 2026`). That section is
   the release's notes and the app's "What's new"; the release stops if the version has none.
2. Set `version` in `Cargo.toml` (say to `0.2.0`) and run `cargo build`, which writes it into `Cargo.lock` too.
   Commit all three: the release builds with `--locked`, so it stops if `Cargo.lock` still has the old version.
3. Merge it into `main`, then tag `main` and push the tag:

   ```
   git switch main
   git pull
   git tag v0.2.0
   git push origin v0.2.0
   ```

Don't make the release on GitHub's website (**Draft a new release**): the workflow makes it, and fails if the tag has
one already.

`.github/workflows/release.yml` builds the app on Windows, packs it with [Velopack](https://velopack.io) into the
setup program, the portable zip and the update packages (with a smaller delta from the last release), and publishes
them as a GitHub release with the version's notes from `CHANGELOG.md`. It stops if the tag doesn't match
`Cargo.toml`. To try the build and packing without publishing, run the workflow by hand (**Actions**, **Release**,
**Run workflow**): the files are kept with the run.

The app id the workflow packs with, `TrashpandaAssetBrowser`, must never change, because installed copies find their
updates by it. Velopack's packer, `vpk`, is installed at the version of the `velopack` crate in `Cargo.lock`; the two
have to match.

### Trying an update before releasing it

With the .NET 8 SDK or later, pack a build twice, as two versions, into one folder:

```
dotnet tool install -g vpk --version 1.2.161
cargo build --release
mkdir target\publish
copy target\release\asset_browser.exe target\publish\
vpk pack -u AssetBrowserTest -v 0.1.0 -p target\publish -e asset_browser.exe -r win-x64 -o target\velopack
```

Unzip `target\velopack\AssetBrowserTest-win-Portable.zip` somewhere and run the `vpk pack` line again with `-v 0.1.1`.
Then start the unzipped copy with `ASSET_BROWSER_UPDATE_FEED` set to that folder, from PowerShell in the repository:

```
$env:ASSET_BROWSER_UPDATE_FEED = "$PWD\target\velopack"
& "<where you unzipped it>\AssetBrowserTest.exe"
```

It looks there instead of on GitHub, finds 0.1.1 and updates to it. The test id keeps it apart from a real install.

## Use it

- **Modes**, the tabs at the top (or **Ctrl+1** to **Ctrl+4**): **All**, **Images**, **Sounds** and **Models**. A mode
  shows only its kind, counts only its kind in the folder tree, and remembers its own layout, tile size, order and
  selection.
- **Grid or list**, top right. The list's columns follow the mode: sizes for images; a waveform, length and format for
  sounds; triangle counts for models. Click a column header to sort, or use the order menu above the assets.
- **The preview pane** on the right shows the selected asset large, with what's known about it and Open, Show in
  folder and Copy path. The button at the top right hides it.
  - **Images:** scroll to zoom at the pointer, drag to pan, double-click for actual pixels. Checkerboard, dark or light
    behind transparency, for the tiles too.
  - **Sprite sheets** play as animations. A sheet with Aseprite or TexturePacker JSON beside it (`hero.json` for
    `hero.png`) plays at once, with each animation to pick: Aseprite's tags, or an atlas's frames grouped by name
    (`run-1`, `run-2`…), with the file's frame timing. So does GameMaker's `name_strip8.png`. For other sheets the
    film-strip button shows how many frames were found between transparent gaps; adjust the grid (across, down,
    border, gap, an animation per row or column, or one) and it's remembered for that file. The overview below marks
    the frame shown: click one to see it. Frames without timing of their own play at 8 fps until you pick a speed.
  - **Animated GIFs** play at once, at their own speed (**File timing**; turn it off to pick one), with every frame
    in the overview. A big one is scaled down evenly so all its frames fit in 64 MB.
  - **Animations zoom and pan like pictures:** scroll, drag, double-click for actual pixels, while they play.
  - **Sounds:** the waveform (click or drag on it to jump), play/pause, stop, loop and volume. Sounds play as soon as
    they're selected; turn off **Auto-play** in the toolbar to stop that.
  - **Models:** a 3D view. Drag to orbit, right-drag to pan, scroll to zoom; buttons for a turntable, the wireframe and
    resetting the view. FBX, OBJ (with its .mtl), glTF/GLB, STL, PLY, 3DS and Collada are read with their materials and
    base-colour textures; Blender files show the preview picture Blender saved in them. The pane lists triangles,
    vertices, meshes, materials, textures (and any that couldn't be found), size in metres and animations.
- **Folders:** add as many as you like with **+** beside Folders, from the folder list at the top left, or by dropping
  a folder on the window. They're browsed together: **All folders** shows everything, and each folder has its own
  tree below it. Right-click one to open it, copy its path or remove it from the library; its files are untouched,
  and its favorites, collections and categories come back if it's added again. A folder that can't be read (an
  unplugged drive, say) keeps what was found there last time, marked with a warning.
- **Favorites, collections and categories**, in the sidebar, gather assets from any folder without moving a file.
  - **Favorites:** the star on a tile or row, the pane's Favorite button, or **F** for the selection.
  - **Collections** are lists for a purpose ("Forest level props"), browsed like a folder.
  - **Categories** are coloured labels ("Footsteps", "UI"): dots on the tiles, chips in the list and the pane, and the
    search box finds assets by them. **Uncategorized** holds everything in no category: put assets in categories from
    there and they leave it, the next one taking the selection, so you can work through it.
  - Make one with **+**, or by dropping assets on "New collection" or "New category"; add to one by dragging assets
    onto it (with the rest of the selection), from the right-click menu, or with the pane's chips. Right-click one to
    rename, recolour or delete it (the files stay where they are). Files moved or deleted since are flagged, with a
    click to take them out.
- **Selecting:** click; **Ctrl+click** adds or removes; **Shift+click** selects a range; **Ctrl+A** selects everything
  shown. With several selected, the pane shows how many, their size and their thumbnails, and Copy path copies them all.
- **Deleting:** **Delete** (or right-click, **Delete…**) asks, then moves the selected files to the Recycle Bin (the
  Trash on macOS and Linux); right-click a folder in the tree for **Delete folder…**, which takes everything in it.
  The asset after them is selected next. They can be put back from the Recycle Bin, favorites, collections and
  categories included: those keep them meanwhile, flagged as missing. While a collection or category is browsed,
  the question also offers to just take them out of it. On a drive without a Recycle Bin (a USB stick, a network
  drive) the question says it can't be undone. The library's own folders are never deleted from here, only removed
  from the library.
- **Keys:** arrows move the selection (Shift extends it), Page Up/Down and Home/End jump, **Enter** opens the file in
  its default app (so does double-click), **Space** plays or pauses, **Esc** stops the sound or else clears the
  selection, **Ctrl+F** searches, **Ctrl+C** copies the selected paths, **F** stars them, **Delete** deletes them
  (after asking). Right-click for Open, Show in folder, Copy path, Favorites, collections and categories, and Delete.
  Every shortcut can be changed, or added (the preview pane, grid, list, show in folder), in Settings.
- **Settings** (the gear at the top right, or **Ctrl+,**):
  - **Updates:** the version, **Check for updates**, **What's new** (every version's notes) and the releases page.
  - **Keyboard:** click a shortcut and press the new keys; a shortcut taken from another action leaves it with none.
  - **Storage:** how big the cache and your data are, part by part, and clearing any of them: thumbnails, waveforms,
    3D model details, the folder index, the settings, and (asked again, since they can't be brought back) favorites,
    collections and categories, sprite sheet choices and the library's folders. The files and the logs are a click
    away.
  - **Memory:** how much memory the app may use, shared out among decoding, thumbnails on the graphics card, sounds
    read ahead, model textures and GIF frames; 1.5 GB keeps what it used before the setting existed.
- The left panel filters by folder (including everything below it). The search box matches every word against the
  file's path in its folder and its categories, so folder names work too.

## How it stays fast

- **The index is cached.** The library opens with the last scan of each folder at once (22 ms for 60k files) and
  rescans them in the background. A scan walks the tree on every core (51 ms for 60k files in 5k folders once the OS has the folders
  cached).
- **Thumbnails are made once.** They're stored in one SQLite file, not thousands of small files, keyed by path, size
  and modification time: an edited file gets a new thumbnail. Served from the cache, 50,000 thumbnails/s.
- **On-screen first.** The grid asks only for the visible rows plus a few around them, every frame; that list replaces
  the work queue, so scrolling past something drops it rather than making it first.
- **The UI thread never touches the disk or decodes anything,** and only draws the rows on screen.
- **PSDs are read from their flattened picture,** and only the rows the thumbnail needs (`src/psd_composite.rs`).
- **Memory stays bounded.** A picture is decoded at full size before it's shrunk, so the thumbnail workers share a
  decoding budget (512 MB): big pictures take turns, small ones never wait. Opaque pictures are shrunk without the
  extra copy transparency needs, and no worker keeps its working copy of a big picture afterwards. At most 1,500
  thumbnails stay on the GPU (about 300 MB), the least recently seen going first.
- **Sound never waits on the disk or a lock.** A decoder thread keeps 2 s of audio ready in a lock-free ring buffer; the
  audio callback only copies from it. The next sounds after the selection are read into memory ahead of time, so
  stepping through a folder with the arrow keys starts each one in under a millisecond.
- **Waveforms are made once,** like thumbnails, and drawn as one mesh per tile.
- **Model thumbnails are rendered on the CPU** by the same workers, then cached like the rest; nothing waits on the GPU.
- **The preview pane loads its large picture or model on its own thread,** newest request first: arrowing through a
  folder never queues up full-size loads, and the thumbnail stands in until the large one is ready.
- **The 3D view draws into a texture on the GPU egui already uses,** and only redraws when the camera, the model or the
  pane's size changes.

## Measure it

```
cargo run --release --bin bench -- <folder> [--thumbs N] [--sounds N] [--models N] [--gifs N] [--only TEXT] [--offset K] [--threads N] [--sheet out.png]
```

Prints scan, index and search timings, then makes thumbnails, waveforms and model thumbnails for an even sample of the
folder's images, sounds and models, twice each: cold (into a throwaway cache) and warm. Then it times how long a sound
takes to start, from disk and from memory, and loads the frames of the GIFs as the preview pane does. `--only .psd`
limits the sample to matching paths, `--offset` picks a different
sample (the OS keeps recently read files in memory, so compare settings on fresh files), and `--sheet` writes the
thumbnails to one image to check by eye.

```
cargo run --release --bin shots -- <folder>
```

Opens the app on your library and saves screenshots of the window, each settings page and "What's new" into the
folder, to check the look by eye; it closes itself after about 17 seconds, without taking the keyboard.

Measured on F:\Resources (60,648 assets, spinning laptop disk, 20 threads), 12 Oct 2026:

| | |
|---|---|
| Scan, 60k files | 51 ms |
| Load cached index / build library / search | 22 / 26 / 1.6 ms |
| Thumbnails, cold, mixed images | 86-100/s: the disk's limit for random files, however many reads are queued |
| Thumbnails, cold, PSDs | 37/s at 100 MB peak (the `psd` crate: 10/s at 2 GB) |
| Thumbnails, from cache | 12,000-50,000/s |
| Waveforms, cold, mixed WAV/MP3/OGG | 37/s at 174 MB peak |
| Waveforms, from cache | 20,000/s |
| Sound start (open + first packet), from disk / read ahead | median 15.7 ms / under 0.1 ms, plus the device's buffer |
| Model thumbnails, all 557 models, cold / files already in memory | 138/s / 1,456/s, 0 failures |
| Model thumbnails, from cache | 21,760/s |
| Model thumbnail, one thread, files in memory: load / render | median 0.4 / 3.2 ms; worst 88 / 20 ms |
| GIF frames for the preview pane, all 559 GIFs (544 animated) | median 0.1 ms, worst 0.8 s (104 frames of 1270 × 698), 0 failures |
| Memory, thumbnails for 30 PNGs of 4400 × 4400 not seen before: peak / after | 0.7 GB / back to 41 MB in 3 s (it was 3.7 GB, then 2.1 GB kept for good) |

## Where things are

| File | What it does |
|---|---|
| `src/scan.rs` | Parallel folder walk; which files count as assets |
| `src/library.rs` | The library's folders: sorted entries, one folder tree over all of them, filtering |
| `src/db.rs` | SQLite: folder index, thumbnail, waveform and model-facts caches, settings |
| `src/thumbs.rs` | Preview worker pool and cache writer; image decoding and resizing |
| `src/audio.rs` | Playback (cpal + symphonia, lock-free ring buffer), read-ahead, waveforms |
| `src/psd_composite.rs` | Reads a PSD/PSB's flattened picture at reduced size |
| `src/model/` | 3D models: the loaders, the CPU thumbnail renderer, and the lighting shared with the 3D view |
| `src/detail.rs` | Loads the preview pane's large picture (or a GIF's frames) or whole model |
| `src/groups.rs` | Favorites, collections and categories, and where their members are in the library |
| `src/sheets.rs` | Sprite sheets: reading Aseprite / TexturePacker data, guessing grids, per-file choices |
| `src/animated.rs` | Animated GIFs as sheets of their own frames, within a memory budget |
| `src/app.rs` | The window: panels, keyboard, modes, settings |
| `src/ui/mode.rs` | The modes, what each remembers, and the selection |
| `src/ui/browse.rs` | The grid and the list (with each mode's columns) |
| `src/ui/inspector.rs` | The preview pane, with a section per kind |
| `src/ui/image_view.rs` | The pane's zoomable picture, which animations are drawn through too |
| `src/ui/sound.rs` | The player: transport, waveforms, now playing |
| `src/ui/viewport.rs` | The pane's 3D view (wgpu) |
| `src/ui/folders.rs` | The folder tree and the list of the library's folders |
| `src/ui/organize.rs` | Favorites, collections and categories in the sidebar, menus, chips, dragging |
| `src/ui/sheet_view.rs` | The pane's player for sprite sheets and GIFs, and the grid |
| `src/ui/previews.rs` | Thumbnails on the GPU, waveforms and model facts the window has |
| `src/ui/theme.rs`, `widgets.rs`, `format.rs` | Colours, fonts and icons; the hand-drawn controls; numbers and dates |
| `src/platform.rs` | Open / show in file manager, the Recycle Bin, the title bar's colours, the process's memory: the only per-OS code |
| `src/deletion.rs` | Deleting to the Recycle Bin on its own thread, and which files actually went |
| `src/diag.rs` | Error log and panic hook (release builds have no console) |
| `src/updates.rs` | Updates through Velopack: checking the releases, downloading, installing on a restart or on closing |
| `src/ui/updates.rs` | The version and the updater's news at the end of the status bar |
| `src/keymap.rs` | The keyboard shortcuts: defaults, changing them, matching key presses |
| `src/memory.rs` | The memory setting, shared out among decoding, GPU thumbnails, read-ahead, textures and GIF frames |
| `src/storage.rs` | Measuring the cache and user data part by part, and clearing parts of the cache |
| `src/changelog.rs`, `CHANGELOG.md` | What's new in each version, built into the app; the release's notes too |
| `src/ui/settings.rs` | The settings window: Updates, Keyboard, Storage and Memory pages |
| `src/ui/whats_new.rs` | "What's new" after an update, and every version's notes |
| `src/bin/bench.rs` | The benchmark |
| `src/bin/shots.rs` | Screenshots of the window and the settings, to check the look |
| `build.rs`, `assets/` | The app icon, and putting it with the app's name into the Windows exe (`src/main.rs` sets the window's) |
| `.cargo/config.toml` | Links the C runtime into Windows builds, so they need no Visual C++ redistributable |
| `.github/workflows/` | `ci.yml` checks every push on Windows, macOS and Linux; `release.yml` publishes a release for a version tag |

The cache and log live in the OS's per-user cache folder (`%LOCALAPPDATA%\AssetBrowser\cache` on Windows). Settings
(mode, layouts, volume and so on) are saved in the same file. What you make yourself is kept apart, so clearing the
cache can't lose it: the library's folders, favorites, collections, categories and sprite sheet choices are in
`%APPDATA%\AssetBrowser\data\user.sqlite`. Failed update checks go in the app's log too; Velopack's own updater logs
to `%LOCALAPPDATA%\velopack\velopack_TrashpandaAssetBrowser.log`.

## Adding to a mode

What belongs to one kind of asset lives in a few known places, so a new tool for it goes next to the others:

- What the mode browses, its name and icon: `Mode` in `src/ui/mode.rs`.
- Its list columns: `columns()` in `src/ui/browse.rs`.
- Its options above the assets: the `match mode` at the end of `BrowserApp::toolbar` in `src/app.rs`.
- Its part of the preview pane: `image_section`, `sound_section` or `model_section`, and the matching
  `*_properties`, in `src/ui/inspector.rs`.
- Anything it should remember: `load_settings` and `save_settings` in `src/app.rs`.

## Not yet

Material previews, SVG, Opus sounds (symphonia has no Opus decoder), dragging files out into other apps, playing a
model's animations, and the geometry inside Blender files (only the picture Blender saved).
