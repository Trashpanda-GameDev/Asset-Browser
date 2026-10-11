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
- **Sounds** play as soon as they're selected (turn off **Auto-play** in the toolbar to stop that). **Space** plays or
  pauses, **Esc** stops. The player bar shows the waveform: click or drag on it to jump. **Loop** repeats without a gap.
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
- **Sound never waits on the disk or a lock.** A decoder thread keeps 2 s of audio ready in a lock-free ring buffer; the
  audio callback only copies from it. The next sounds after the selection are read into memory ahead of time, so
  stepping through a folder with the arrow keys starts each one in under a millisecond.
- **Waveforms are made once,** like thumbnails, and drawn as one mesh per tile.

## Measure it

```
cargo run --release --bin bench -- <folder> [--thumbs N] [--sounds N] [--only TEXT] [--offset K] [--threads N] [--sheet out.png]
```

Prints scan, index and search timings, then makes thumbnails and waveforms for an even sample of the folder's images
and sounds, twice each: cold (into a throwaway cache) and warm. Then it times how long a sound takes to start, from disk
and from memory. `--only .psd` limits the sample to matching paths, `--offset` picks a different
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

## Where things are

| File | What it does |
|---|---|
| `src/scan.rs` | Parallel folder walk; which files count as assets |
| `src/library.rs` | Sorted entries, folder tree, filtering |
| `src/db.rs` | SQLite: folder index, thumbnail and waveform caches, settings |
| `src/thumbs.rs` | Preview worker pool and cache writer; image decoding and resizing |
| `src/audio.rs` | Playback (cpal + symphonia, lock-free ring buffer), read-ahead, waveforms |
| `src/psd_composite.rs` | Reads a PSD/PSB's flattened picture at reduced size |
| `src/app.rs` | The window |
| `src/platform.rs` | Open / show in file manager: the only per-OS code |
| `src/diag.rs` | Error log and panic hook (release builds have no console) |
| `src/bin/bench.rs` | The benchmark |

The cache and log live in the OS's per-user cache folder (`%LOCALAPPDATA%\AssetBrowser\cache` on Windows).

## Not yet

3D model previews, material previews, SVG, Opus sounds (symphonia has no Opus decoder), dragging files out into
other apps.
