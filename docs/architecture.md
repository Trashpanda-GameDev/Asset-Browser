# How the code fits together

The rules the code follows so it stays easy to change: which part may use which, how the window's parts talk to
each other, and how errors, stored data and tests are handled. [Working on Asset Browser](development.md) has the map
of the files. [Back to the README](../README.md)

Some older code doesn't follow these rules yet and is being brought in line: the architecture test and the
`refactor:` marks (below) list what's left.

## Layers

Every module sits in a layer and uses only modules in its own layer or below. Nothing below `ui` knows about egui, so
the core can be tested, measured and reused without a window.

| Layer | Its job | Modules |
|---|---|---|
| app | The window's state and what happens each frame: takes in what the services finished, applies what the views asked for | `app` |
| ui | Draws the window from the state it's given, and returns what the user asked for; the keyboard shortcuts | `ui` |
| services | Slow work on background threads, behind a handle | `rescan`, `thumbs`, `audio`, `playback`, `detail`, `deletion`, `updates`, `watch`, `storage`, `memory` |
| domain | What assets are and how to read them; the library and the user's organizing | `asset`, `library`, `scan`, `groups`, `sheets`, `model`, `imaging`, `preview_key`, `folder_index`, `animated`, `changelog` |
| foundation | Plumbing: where files go, the databases, the log, calls into the OS, background threads | `paths`, `db`, `diag`, `platform`, `work` |

The programs (`src/main.rs`, `bench`, `shots`) sit on top of all of them.

`tests/architecture.rs` checks this on every test run: no module uses a higher layer, no modules depend on each
other in a circle, nothing below `ui` uses egui, and no view reads files, starts threads or opens databases. A new
module needs a place in its list of layers. What breaks a rule today is listed there as an exception, and an
exception the code no longer needs fails the test, so the list only shrinks.

## How the parts talk

- **The app** (`src/app/`) owns the state and the services. Each frame it takes in what the services finished,
  rebuilds what's shown if something changed, handles the keys, draws the panels, and then does what they asked for.
- **Views** (`src/ui/`) draw and return intents: `show(ui, &Args) -> Output`, where the output is what the user asked
  for (`header::HeaderCommand`, `organize::GroupCommand`, `sound::SoundCommand`, `settings::Command`,
  `browse::BrowseOutput`). A view changes nothing but its own view state, such as a picture's zoom or the text being
  typed (which comes in its args by value, like `HeaderArgs::query`). It reads no files, starts no threads and opens
  no databases: the app gives it what it shows, the settings it draws with included (`display::Display`), and applies
  the output after the panel's closure returns, so nothing changes halfway through drawing a frame. The app never
  writes a view's fields: it tells a view something through a method (`Settings::open`, `FolderTree::reveal`).
- **Services** keep slow work off the window's thread: `start(wake)` returns a handle, requests go in through its
  methods, results come back through `poll` or `try_recv`, `wake` asks the window to draw again when something is
  ready, and dropping the handle stops its threads. Nothing slow runs on the window's thread. The pieces they're
  built from are in `src/work.rs`: `spawn` starts a named thread (a thread the OS won't start is reported, and the
  app carries on without it), `lock` and `wait` keep working after another thread panicked, `WorkList` is the list
  of work a thread takes from (each new list replaces what's waiting), and `Wake` is the call that wakes the window.
- **State** belongs to the part that uses it. States that rule each other out are one enum, not several bools.

## Rules for code

Checked by clippy (`clippy.toml`, and `[lints]` in `Cargo.toml`), with warnings as errors in the release workflow:

- A function does one thing: at most 60 lines of code, a cognitive complexity of at most 15, and at most 5
  parameters (more go in an args struct, like `InspectorArgs`). A longer function becomes named steps.
- No flag arguments and no clusters of bools: they become an enum that names the cases.
- `#[expect(clippy::..., reason = "...")]` marks code that breaks a rule, and says why. A reason starting with
  `refactor:` is a to-do. The build fails once the code no longer needs its mark, so marks can't go stale.

Not checked by a tool, but expected:

- One job per module. A file with much more than 400 lines of code (not counting its tests) is usually doing two.
- Names say what a thing is or does, in whole words.
- Comments say why, not what.
- Nothing is written twice: the second copy becomes a shared function.

## Errors

- A module that can fail returns its own error type: an enum with `thiserror`, whose message is what the user reads
  (`platform::CopyError` is a small one). Errors from a library pass through as they are (`trash::Error`).
- The app shows errors with `diag::report` (the log, and the status bar); `diag::note` only logs, for errors not worth
  showing, such as an update check while offline.
- No error is dropped without saying why: a `let _ =` on a `Result` has a comment.

## Stored data

- `library.sqlite` is a cache: everything in it can be made again from the files, except the settings.
- `user.sqlite` holds what only the user can make: the library's folders, favorites, collections, categories and
  sprite-sheet choices. Its tables are all made in one place (`db::open_user_data`), and it changes shape only
  through a migration there that keeps what's there.
- Every SQLite file is opened with `db::connect`, and several writes that belong together go in one
  `db::in_transaction`.
- Every settings key and every number stored for an enum (asset kinds, group kinds) has one owner, and a test pins
  its value: changing one loses what people saved.
- Changing how a cache stores something means bumping its version number, which clears that part of everyone's cache.

## Tests

- Tests sit next to the code, in a `mod tests` at the end of the file.
- Files a test makes go in a `test_support::TempDir`: a new folder of its own, removed when the test ends, even when
  it fails.
- They never touch the user's own folders, settings or data, the network or the clipboard. A test that has to is
  `#[ignore]`d and says how to run it.
- Code without tests gets them before it's changed.
- Everything that reads files made elsewhere has a damaged-file test: whatever the bytes, it must not panic.

## Adding things

- **A kind of file:** its extension in `AssetKind::from_extension` (`src/asset.rs`), and what makes its preview in
  `src/thumbs/worker.rs`.
- **A model format:** a reader in `src/model/`, `parse(bytes, file, options)`, and a line in `model::FORMATS` with its
  extensions (which also go in `AssetKind::from_extension`).
- **A keyboard shortcut:** an `Action` in `src/ui/keymap.rs` with its id (it's saved, so it never changes), label, group
  and default; what it does goes in `window_keys` (the window's own) or `asset_keys` (the selected assets), in
  `src/app/actions.rs`.
- **A setting:** its key in `src/app/setting_keys.rs` (it's stored, so it never changes, and the test there pins
  it), read in `load_settings` and written in `save_settings` (`src/app/settings.rs`).
- **A settings page:** a `Page` in `src/ui/settings/mod.rs`, and a file of its own beside it that draws it.
- **Something for one mode:** see [adding to a mode](development.md#adding-to-a-mode).
- **Slow work:** a service, as above.
