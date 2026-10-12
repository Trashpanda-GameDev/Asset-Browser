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

`cargo build --release` makes `target\release\asset_browser.exe`: one file, with its icon and the C runtime built in,
so it runs as it is on 64-bit Windows 10 and 11. Zip it and send it; it keeps its data in each user's own folders
(see below). Until the exe is code-signed, Windows warns about it when it comes from the internet ("Windows protected
your PC": **More info**, **Run anyway**). Builds for macOS and Linux have to be made on those systems.

The icon is `assets/icon.png` (1024 px); `assets/icon.ico` holds every size Windows uses, made from it.

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
    the frame shown: click one to see it.
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
- **Keys:** arrows move the selection (Shift extends it), Page Up/Down and Home/End jump, **Enter** opens the file in
  its default app (so does double-click), **Space** plays or pauses, **Esc** stops the sound or else clears the
  selection, **Ctrl+F** searches, **Ctrl+C** copies the selected paths, **F** stars them. Right-click for Open, Show
  in folder, Copy path, Favorites, collections and categories.
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
cargo run --release --bin bench -- <folder> [--thumbs N] [--sounds N] [--models N] [--only TEXT] [--offset K] [--threads N] [--sheet out.png]
```

Prints scan, index and search timings, then makes thumbnails, waveforms and model thumbnails for an even sample of the
folder's images, sounds and models, twice each: cold (into a throwaway cache) and warm. Then it times how long a sound
takes to start, from disk and from memory. `--only .psd` limits the sample to matching paths, `--offset` picks a different
sample (the OS keeps recently read files in memory, so compare settings on fresh files), and `--sheet` writes the
thumbnails to one image to check by eye.

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
| `src/detail.rs` | Loads the preview pane's large picture or whole model |
| `src/groups.rs` | Favorites, collections and categories, and where their members are in the library |
| `src/sheets.rs` | Sprite sheets: reading Aseprite / TexturePacker data, guessing grids, per-file choices |
| `src/app.rs` | The window: panels, keyboard, modes, settings |
| `src/ui/mode.rs` | The modes, what each remembers, and the selection |
| `src/ui/browse.rs` | The grid and the list (with each mode's columns) |
| `src/ui/inspector.rs` | The preview pane, with a section per kind |
| `src/ui/image_view.rs` | The pane's zoomable picture |
| `src/ui/sound.rs` | The player: transport, waveforms, now playing |
| `src/ui/viewport.rs` | The pane's 3D view (wgpu) |
| `src/ui/folders.rs` | The folder tree and the list of the library's folders |
| `src/ui/organize.rs` | Favorites, collections and categories in the sidebar, menus, chips, dragging |
| `src/ui/sheet_view.rs` | The pane's sprite sheet player and grid |
| `src/ui/previews.rs` | Thumbnails on the GPU, waveforms and model facts the window has |
| `src/ui/theme.rs`, `widgets.rs`, `format.rs` | Colours, fonts and icons; the hand-drawn controls; numbers and dates |
| `src/platform.rs` | Open / show in file manager: the only per-OS code |
| `src/diag.rs` | Error log and panic hook (release builds have no console) |
| `src/bin/bench.rs` | The benchmark |
| `build.rs`, `assets/` | The app icon, and putting it with the app's name into the Windows exe (`src/main.rs` sets the window's) |
| `.cargo/config.toml` | Links the C runtime into Windows builds, so they need no Visual C++ redistributable |

The cache and log live in the OS's per-user cache folder (`%LOCALAPPDATA%\AssetBrowser\cache` on Windows). Settings
(mode, layouts, volume and so on) are saved in the same file. What you make yourself is kept apart, so clearing the
cache can't lose it: the library's folders, favorites, collections, categories and sprite sheet choices are in
`%APPDATA%\AssetBrowser\data\user.sqlite`.

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
