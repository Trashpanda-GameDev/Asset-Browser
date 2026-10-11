# AssetBrowser

A fast, native browser for folders of game assets: point it at a folder and browse its images, sounds and models
with previews. Written in Rust with [egui](https://github.com/emilk/egui) on wgpu. Windows first; macOS and Linux
are kept buildable.

## Run

```
cargo run --release                  # opens the last folder you used
cargo run --release -- F:\Resources  # opens this folder
```

- **Click** selects, **double-click** or **Enter** opens the file in its default app, **arrow keys** move the selection.
- **Right-click** a tile: Open, Show in folder, Copy path.
- The left panel filters by folder (including everything below it). The search box matches every word against the
  file's path, so folder names work too.

## How it stays fast

- **The index is cached.** Reopening a folder shows the last scan at once (22 ms for 60k files) and rescans in the
  background. A scan walks the tree on every core (51 ms for 60k files in 5k folders once the OS has the folders
  cached).
- **Thumbnails are made once.** They're stored in one SQLite file, not thousands of small files, keyed by path, size
  and modification time: an edited file gets a new thumbnail. Served from the cache, 50,000 thumbnails/s.
- **On-screen first.** The grid asks only for the visible rows plus a few around them, every frame; that list replaces
  the work queue, so scrolling past something drops it rather than making it first.
- **The UI thread never touches the disk or decodes anything,** and only draws the rows on screen.
- **PSDs are read from their flattened picture,** and only the rows the thumbnail needs (`src/psd_composite.rs`).

## Measure it

```
cargo run --release --bin bench -- <folder> [--thumbs N] [--only TEXT] [--offset K] [--threads N] [--sheet out.png]
```

Prints scan, index and search timings, then makes thumbnails for an even sample of the folder's images twice: cold
(into a throwaway cache) and warm. `--only .psd` limits the sample to matching paths, `--offset` picks a different
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

## Where things are

| File | What it does |
|---|---|
| `src/scan.rs` | Parallel folder walk; which files count as assets |
| `src/library.rs` | Sorted entries, folder tree, filtering |
| `src/db.rs` | SQLite: folder index, thumbnail cache, settings |
| `src/thumbs.rs` | Worker pool, cache writer, decoding and resizing |
| `src/psd_composite.rs` | Reads a PSD/PSB's flattened picture at reduced size |
| `src/app.rs` | The window |
| `src/platform.rs` | Open / show in file manager: the only per-OS code |
| `src/diag.rs` | Error log and panic hook (release builds have no console) |
| `src/bin/bench.rs` | The benchmark |

The cache and log live in the OS's per-user cache folder (`%LOCALAPPDATA%\AssetBrowser\cache` on Windows).

## Not yet

Sound playback and waveforms, 3D model previews, material previews, SVG, dragging files out into other apps.
