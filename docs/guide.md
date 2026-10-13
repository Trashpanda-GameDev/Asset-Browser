# Using Asset Browser

Everything the app does, and every key. [Back to the README](../README.md)

## Installing and updating

Get it from the repository's [Releases](https://github.com/Trashpanda-GameDev/Asset-Browser/releases), for 64-bit
Windows 10 and 11, in either of two forms. Both update themselves.

- **`TrashpandaAssetBrowser-win-Setup.exe`** installs it for the current user, without admin rights, into
  `%LOCALAPPDATA%\TrashpandaAssetBrowser`, with Start menu and desktop shortcuts, then starts it. It's uninstalled
  from Windows' **Installed apps**.
- **`TrashpandaAssetBrowser-win-Portable.zip`** runs from wherever it's unzipped: start **Asset Browser.exe** in it.

Until the app is code-signed, Windows warns about the download ("Windows protected your PC": **More info**,
**Run anyway**); the updates it downloads itself aren't warned about.

The app checks for a newer release when it starts, every 6 hours, and when asked (**Settings → Updates → Check for
updates**, or a click on its version at the right end of the status bar). It downloads one in the background and
shows **Restart to update**; if you don't, the update is installed when you close the app. The first start after an
update shows **What's new** once; every version's notes are under **Settings → Updates → What's new**. Your library,
favorites and settings are kept apart from the app (see [Where your things are](#where-your-things-are)), so
updating, uninstalling or deleting the portable folder leaves them alone.

## Your library

- **Folders:** add as many as you like with **+** beside Folders, from the folder list at the top left, or by dropping
  a folder on the window. They're browsed together: **All folders** shows everything, and each folder has its own
  tree below it. Right-click one to open it, copy its path or remove it from the library; its files are untouched,
  and its favorites, collections and categories come back if it's added again. A folder that can't be read (an
  unplugged drive, say) keeps what was found there last time, marked with a warning.
- **It keeps itself current.** Files added to, changed in, renamed in or removed from the library's folders show up
  by themselves while the app is open: the OS says what changed, and that folder is scanned again once the changes
  settle (after 0.75 s of quiet, or every 4 s during a long copy). Reads and files that aren't assets don't count.
- **Filtering:** the left panel filters by folder (including everything below it). The search box matches every word
  against the file's path in its folder and its categories, so folder names work too.

## Browsing

- **Modes**, the tabs at the top (or **Ctrl+1** to **Ctrl+4**): **All**, **Images**, **Sounds** and **Models**. A mode
  shows only its kind, counts only its kind in the folder tree, and remembers its own layout, tile size, order and
  selection.
- **Grid or list**, top right. The list's columns follow the mode: sizes for images; a waveform, length and format for
  sounds; triangle counts for models. Click a column header to sort, or use the order menu above the assets.

## The preview pane

The pane on the right shows the selected asset large, with what's known about it and Open, Show in folder and Copy
path. The button at the top right hides it.

- **Images:** scroll to zoom at the pointer, drag to pan, double-click for actual pixels. Checkerboard, dark or light
  behind transparency, for the tiles too.
- **Sprite sheets** play as animations. A sheet with Aseprite or TexturePacker JSON beside it (`hero.json` for
  `hero.png`) plays at once, with each animation to pick: Aseprite's tags, or an atlas's frames grouped by name
  (`run-1`, `run-2`…), with the file's frame timing. So does GameMaker's `name_strip8.png`. For other sheets the
  film-strip button shows how many frames were found between transparent gaps; adjust the grid (across, down,
  border, gap, an animation per row or column, or one) and it's remembered for that file. The overview below marks
  the frame shown: click one to see it. Frames without timing of their own play at 8 fps until you pick a speed.
- **Animated GIFs** play at once, at their own speed (**File timing**; turn it off to pick one), with every frame in
  the overview. A big one is scaled down evenly so all its frames fit in 64 MB.
- **Animations zoom and pan like pictures:** scroll, drag, double-click for actual pixels, while they play.
- **Sounds:** the waveform (click or drag on it to jump), play/pause, stop, loop and volume. Sounds play as soon as
  they're selected; turn off **Auto-play** in the toolbar to stop that.
- **Models:** a 3D view. Drag to orbit, right-drag to pan, scroll to zoom; buttons for a turntable, the wireframe and
  resetting the view. FBX, OBJ (with its .mtl), glTF/GLB, STL, PLY, 3DS and Collada are read with their materials and
  base-colour textures; Blender files show the preview picture Blender saved in them. The pane lists triangles,
  vertices, meshes, materials, textures (and any that couldn't be found), size in metres and animations.

## Favorites, collections and categories

They're in the sidebar, and gather assets from any folder without moving a file.

- **Favorites:** the star on a tile or row, the pane's Favorite button, or **F** for the selection.
- **Collections** are lists for a purpose ("Forest level props"), browsed like a folder.
- **Categories** are coloured labels ("Footsteps", "UI"): dots on the tiles, chips in the list and the pane, and the
  search box finds assets by them. **Uncategorized** holds everything in no category: put assets in categories from
  there and they leave it, the next one taking the selection, so you can work through it.
- Make one with **+**, or by dropping assets on "New collection" or "New category"; add to one by dragging assets onto
  it (with the rest of the selection), from the right-click menu, or with the pane's chips. Right-click one to rename,
  recolour or delete it (the files stay where they are). Files moved or deleted since are flagged, with a click to
  take them out.

## Selecting and deleting

- **Selecting:** click; **Ctrl+click** adds or removes; **Shift+click** selects a range; **Ctrl+A** selects everything
  shown. With several selected, the pane shows how many, their size and their thumbnails, and Copy path copies them
  all.
- **Deleting:** **Delete** (or right-click, **Delete…**) asks, then moves the selected files to the Recycle Bin (the
  Trash on macOS and Linux); right-click a folder in the tree for **Delete folder…**, which takes everything in it.
  The asset after them is selected next. They can be put back from the Recycle Bin, favorites, collections and
  categories included: those keep them meanwhile, flagged as missing. While a collection or category is browsed, the
  question also offers to just take them out of it. On a drive without a Recycle Bin (a USB stick, a network drive)
  the question says it can't be undone. The library's own folders are never deleted from here, only removed from
  the library.

## Keys

Arrows move the selection (Shift extends it), Page Up/Down and Home/End jump, **Enter** opens the file in its default
app (so does double-click), **Space** plays or pauses, **Esc** stops the sound or else clears the selection,
**Ctrl+F** searches, **Ctrl+C** copies the selected paths, **F** stars them, **Delete** deletes them (after asking).
Right-click for Open, Show in folder, Copy path, Favorites, collections and categories, and Delete. Every shortcut can
be changed, or added (the preview pane, grid, list, show in folder), in Settings.

## Settings

The gear at the top right, or **Ctrl+,**:

- **Updates:** the version, **Check for updates**, **What's new** (every version's notes) and the releases page.
- **Keyboard:** click a shortcut and press the new keys; a shortcut taken from another action leaves it with none.
- **Storage:** how big the cache and your data are, part by part, and clearing any of them: thumbnails, waveforms,
  3D model details, the folder index, the settings, and (asked again, since they can't be brought back) favorites,
  collections and categories, sprite sheet choices and the library's folders. The files and the logs are a click
  away.
- **Memory:** how much memory the app may use, shared out among decoding, thumbnails on the graphics card, sounds read
  ahead, model textures and GIF frames; 1.5 GB keeps what it used before the setting existed.

## Where your things are

The cache and the log live in the OS's per-user cache folder (`%LOCALAPPDATA%\AssetBrowser\cache` on Windows), and
the settings (mode, layouts, volume and so on) are saved in the same file. What you make yourself is kept apart, so
clearing the cache can't lose it: the library's folders, favorites, collections, categories and sprite sheet choices
are in `%APPDATA%\AssetBrowser\data\user.sqlite`. Failed update checks go in the app's log too; Velopack's own
updater logs to `%LOCALAPPDATA%\velopack\velopack_TrashpandaAssetBrowser.log`.

## Not yet

Material previews, SVG, Opus sounds (symphonia has no Opus decoder), dragging files out into other apps, playing a
model's animations, and the geometry inside Blender files (only the picture Blender saved). Builds for macOS and Linux
would have to be made on those systems, and aren't released yet.
