# Working on Asset Browser

How the code is laid out, how it stays fast, and the tools for measuring it and taking pictures of it.
[Back to the README](../README.md)

It's written in Rust with [egui](https://github.com/emilk/egui) on wgpu. Windows comes first; it's written to build
on macOS and Linux too, and the release workflow tests it on both, but it hasn't been run there yet.

```
cargo run --release                  # opens the library: the folders you added
cargo run --release -- F:\Resources  # adds this folder to the library (or shows it, if it's in already)
```

Work goes on the `dev` branch and into `main` with a pull request; releases are made from `main`
([releasing](releasing.md)). What's new for the people using the app goes in [`CHANGELOG.md`](../CHANGELOG.md) under
**Unreleased** as it's made.

## Where things are

| File | What it does |
|---|---|
| `src/scan.rs` | Parallel folder walk; which files count as assets |
| `src/watch.rs` | Told by the OS when the library's folders change, to rescan them once the changes settle |
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
| `src/app/` | The window's state (`BrowserApp`, in `mod.rs`) and what it does, a file an area: the library's folders and scans (`library.rs`), what's browsed and selected (`view.rs`), keys and asset actions (`actions.rs`), favorites, collections and categories (`organizing.rs`), deleting (`deleting.rs`), settings (`settings.rs`), the window's parts (`header.rs`, `toolbar.rs`, `sidebar.rs`, `panels.rs`, `toast.rs`), and the screenshots tool's hooks (`screenshots.rs`) |
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
| `src/bin/shots/` | Screenshots of the window and the settings, to check the look; and the README's pictures and clips, of made-up assets |
| `build.rs`, `assets/` | The app icon, and putting it with the app's name into the Windows exe (`src/main.rs` sets the window's) |
| `.cargo/config.toml` | Links the C runtime into Windows builds, so they need no Visual C++ redistributable |
| `.github/workflows/release.yml` | For release commits only: tests on Windows, macOS and Linux, builds and publishes the release, keeps two caches per job |
| `scripts/deploy.mjs`, `package.json` | `npm run deploy`: the next version's commit and tag on `main`, pushed to start the release |

The icon is `assets/icon.png` (1024 px); `assets/icon.ico` holds every size Windows uses, made from it. Where the app
keeps the cache, the log and the user's own data is in the [guide](guide.md#where-your-things-are); setting
`ASSET_BROWSER_HOME` to a folder puts all of them there instead.

## How it stays fast

- **The index is cached.** The library opens with the last scan of each folder at once (22 ms for 60k files) and
  rescans them in the background. A scan walks the tree on every core (51 ms for 60k files in 5k folders once the OS
  has the folders cached).
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
- **Sound never waits on the disk or a lock.** A decoder thread keeps 2 s of audio ready in a lock-free ring buffer;
  the audio callback only copies from it. The next sounds after the selection are read into memory ahead of time, so
  stepping through a folder with the arrow keys starts each one in under a millisecond.
- **Waveforms are made once,** like thumbnails, and drawn as one mesh per tile.
- **Model thumbnails are rendered on the CPU** by the same workers, then cached like the rest; nothing waits on the
  GPU.
- **The preview pane loads its large picture or model on its own thread,** newest request first: arrowing through a
  folder never queues up full-size loads, and the thumbnail stands in until the large one is ready.
- **The 3D view draws into a texture on the GPU egui already uses,** and only redraws when the camera, the model or
  the pane's size changes.

## Measure it

```
cargo run --release --bin bench -- <folder> [--thumbs N] [--sounds N] [--models N] [--gifs N] [--only TEXT] [--offset K] [--threads N] [--sheet out.png]
```

Prints scan, index and search timings, then makes thumbnails, waveforms and model thumbnails for an even sample of the
folder's images, sounds and models, twice each: cold (into a throwaway cache) and warm. Then it times how long a sound
takes to start, from disk and from memory, and loads the frames of the GIFs as the preview pane does. `--only .psd`
limits the sample to matching paths, `--offset` picks a different sample (the OS keeps recently read files in
memory, so compare settings on fresh files), and `--sheet` writes the thumbnails to one image to check by eye.

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

## Screenshots

```
cargo run --release --bin shots -- <folder> [<folder to show>]
```

Opens the app on your library and saves screenshots of the window, each settings page and "What's new", then the
window again at the end, into the folder, to check the look by eye; it closes itself after about 18 seconds, without
taking the keyboard. A second folder is added to the library and shown, as on the app's command line, so something
changed there during the run (a file copied in) shows in the last picture; take it out of the library afterwards.

```
cargo run --release --bin shots -- --demo docs/images F:\Assets
```

Takes the README's pictures and clips again, after the look changes; it takes about a minute. It draws and
synthesises a small library of made-up game assets (`src/bin/shots/demo.rs`: icons, tiles, backgrounds, sprite
sheets, GIFs, sounds and 3D models, with some favorites, collections and categories) into the folder given last, which
has to be empty or hold only what a run wrote before, since its path shows in the pictures (without one, it uses
`target/demo`). Then it shows each part of the app on it, pressing keys, typing searches and adding files as it goes,
and records the clips as GIFs (`src/bin/shots/clip.rs`: each frame holds only what changed since the one before,
which keeps them small). The app keeps its cache and settings in `target/demo` for the run, so your own library is
left alone.

## Adding to a mode

What belongs to one kind of asset lives in a few known places, so a new tool for it goes next to the others:

- What the mode browses, its name and icon: `Mode` in `src/ui/mode.rs`.
- Its list columns: `columns()` in `src/ui/browse.rs`.
- Its options above the assets: the `match mode` at the end of `BrowserApp::toolbar` in `src/app/toolbar.rs`.
- Its part of the preview pane: `image_section`, `sound_section` or `model_section`, and the matching
  `*_properties`, in `src/ui/inspector.rs`.
- Anything it should remember: `load_settings` and `save_settings` in `src/app/settings.rs`.
