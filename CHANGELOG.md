# Changelog

What's new in each version of Asset Browser, written for the people using it. The app shows a
version's notes the first time it starts after updating to it (and every version's under Settings →
Updates → What's new), and the release workflow publishes the same notes with the release.

As things change, add them under a `## Unreleased` heading at the top. To release, run
`npm run deploy` on main: it renames the heading to the version and date, as below.

## Unreleased

- **Copy files to paste them anywhere:** right-click and **Copy file**, or press Ctrl+Shift+C, then
  paste into a folder to put a copy there, or into another app that takes files. It's in the preview
  pane too, and copies every selected file at once.
- A damaged Photoshop file can no longer close the app by asking for more memory than there is: it
  shows as a file that can't be read.
- When a file can't be opened or shown in its folder, the status bar says why, where nothing
  happened before.

## 0.2.1 – 14 Oct 2026

- **Files added to your folders show up by themselves**, without reopening the app, and so do files
  changed, renamed or removed: a folder is scanned again as soon as the changes in it settle.
- What's new no longer comes back after an older copy of the app has been opened.

## 0.2.0 – 13 Oct 2026

- **Settings**, from the gear at the top right or Ctrl+,: updates, keyboard shortcuts, storage and
  memory in one place.
- **Check for updates** whenever you like, and see **what's new** after an update, or for every
  version, from the Updates page.
- **Change any keyboard shortcut**, or reset them, on the Keyboard page.
- **See how much space the cache takes** and clear any part of it on the Storage page: thumbnails,
  waveforms, the folder index and more, with your favorites, collections and categories kept unless
  you choose them too. The log's folder is a click away there as well.
- **Set how much memory the app may use** on the Memory page.
- The preview pane no longer jumps while moving through GIFs and sprite sheets with the arrow keys:
  the animation controls now sit below the details, and every detail keeps its place while it loads.
- The title bar is dark, in the app's own colours, instead of the system's light one.

## 0.1.0 – 12 Oct 2026

The first release.

- **Browse folders of game assets**: images (PNG, JPEG, PSD, TGA, WebP, GIF and more), sounds (WAV,
  OGG, MP3, FLAC and more) and 3D models (FBX, OBJ, glTF, STL, PLY, 3DS, Collada, Blender's saved
  preview), with thumbnails made in the background and kept for next time.
- **Several folders in one library**: add them with +, from the folder list at the top left, or by
  dropping them on the window.
- **Modes** for everything, images, sounds or models (Ctrl+1 to 4), each with its own layout, size,
  order and selection, in a grid or a list.
- **The preview pane** shows the selected asset large: pictures zoom and pan, sounds play with a
  waveform to click on, models turn in a 3D view.
- **Favorites, collections and categories** gather assets from any folder without moving a file.
- **Sprite sheets and animated GIFs** play as animations, with their grid adjustable.
- **Delete** files and folders to the Recycle Bin.
- **Updates** download in the background and install when you restart or close the app.
