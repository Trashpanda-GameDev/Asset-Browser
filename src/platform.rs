//! The few things that work differently on each OS: opening and revealing files, copying them to
//! the clipboard, dragging them out of the window, deleting them to the Recycle Bin or Trash, the
//! window's title bar, the UI fonts and the process's memory. Nothing else in the app should call
//! into a specific platform.

use std::cell::RefCell;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Opens the file with its default app.
pub fn open(path: &Path) -> io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        Command::new("explorer").arg(native(path)).spawn().map(drop)
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg(path).spawn().map(drop)
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        Command::new("xdg-open").arg(path).spawn().map(drop)
    }
}

/// Opens a web address in the default browser.
pub fn open_link(url: &str) -> io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        Command::new("explorer").arg(url).spawn().map(drop)
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg(url).spawn().map(drop)
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        Command::new("xdg-open").arg(url).spawn().map(drop)
    }
}

/// Shows the file selected in the OS file manager.
pub fn reveal(path: &Path) -> io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // Explorer wants exactly `/select,"path"`; Rust's own quoting of the whole argument breaks it.
        Command::new("explorer").raw_arg(format!("/select,\"{}\"", native(path))).spawn().map(drop)
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg("-R").arg(path).spawn().map(drop)
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        // There's no portable "select this file" on Linux; open its folder instead.
        Command::new("xdg-open").arg(path.parent().unwrap_or(path)).spawn().map(drop)
    }
}

thread_local! {
    /// Kept for good: on Linux, copied files are offered to other apps for as long as it lives.
    static CLIPBOARD: RefCell<Option<arboard::Clipboard>> = const { RefCell::new(None) };
}

/// Why files couldn't be put on the clipboard.
#[derive(Debug, thiserror::Error)]
pub enum CopyError {
    #[error("the files aren't there any more")]
    Missing,
    #[error(transparent)]
    Clipboard(#[from] arboard::Error),
}

/// Puts the files themselves on the clipboard, as a file manager's Copy does: pasted into a folder
/// (Explorer, Finder or the like) they're copied there, and apps that take pasted files get them.
/// Files that aren't there any more are left out.
pub fn copy_files(paths: &[PathBuf]) -> Result<(), CopyError> {
    // Checked first, so what was on the clipboard stays when there's nothing to copy.
    if !paths.iter().any(|path| path.exists()) {
        return Err(CopyError::Missing);
    }
    CLIPBOARD.with_borrow_mut(|kept| {
        let clipboard = match kept.take() {
            Some(clipboard) => clipboard,
            None => arboard::Clipboard::new()?,
        };
        let clipboard = kept.insert(clipboard);
        // On Windows, arboard adds the files to whatever is on the clipboard: an app pasting text
        // would get what was copied before.
        clipboard.clear()?;
        Ok(clipboard.set().file_list(paths)?)
    })
}

/// Why files couldn't be dragged out of the window.
#[derive(Debug, thiserror::Error)]
pub enum DragError {
    #[error("the files aren't there any more")]
    Missing,
    /// What the shell said.
    #[error("{0}")]
    Shell(io::Error),
}

/// Drags the files out of the window as Explorer does, into a folder, the desktop or another app
/// that takes files. They're offered only to be copied, so a drop never moves one out of its folder.
/// Returns once they're dropped or the drag is given up (Esc, or let go where nothing takes them);
/// the window doesn't redraw meanwhile. Files that aren't there any more are left out.
pub fn drag_files_out(window: &impl raw_window_handle::HasWindowHandle, paths: &[PathBuf]) -> Result<(), DragError> {
    let paths: Vec<&Path> = paths.iter().map(PathBuf::as_path).filter(|path| path.exists()).collect();
    if paths.is_empty() {
        return Err(DragError::Missing);
    }
    #[cfg(target_os = "windows")]
    {
        shell_drag::drag(window, &paths).map_err(|err| DragError::Shell(err.into()))
    }
    #[cfg(not(target_os = "windows"))]
    {
        // Only made for Windows so far.
        let _ = window;
        Ok(())
    }
}

/// Dragging files out with the shell's own data object for them, the one Explorer drags: other
/// apps get the files as from Explorer, with their thumbnails.
#[cfg(target_os = "windows")]
mod shell_drag {
    use std::path::Path;

    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{CoTaskMemFree, IBindCtx, IDataObject};
    use windows::Win32::System::Ole::{DROPEFFECT_COPY, IDropSource};
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        BHID_DataObject, IShellItemArray, SHCreateShellItemArrayFromIDLists, SHDoDragDrop, SHParseDisplayName,
    };
    use windows::core::{HSTRING, Result};

    pub(super) fn drag(window: &impl HasWindowHandle, paths: &[&Path]) -> Result<()> {
        let Ok(handle) = window.window_handle() else { return Ok(()) };
        let RawWindowHandle::Win32(win32) = handle.as_raw() else { return Ok(()) };
        let hwnd = HWND(win32.hwnd.get() as *mut core::ffi::c_void);
        let ids = ItemIds::of(paths)?;
        // SAFETY: the ids are alive until `ids` is dropped, after the array has copied them.
        let items: IShellItemArray = unsafe { SHCreateShellItemArrayFromIDLists(&ids.as_const()) }?;
        // SAFETY: a COM call on a live object.
        let data: IDataObject = unsafe { items.BindToHandler(None::<&IBindCtx>, &BHID_DataObject) }?;
        // Copy only: a drop on the same drive would otherwise move the files out of their folder.
        // SAFETY: the window is ours and outlives the drag; with no drop source the shell's is used.
        unsafe { SHDoDragDrop(Some(hwnd), &data, None::<&IDropSource>, DROPEFFECT_COPY) }?;
        Ok(())
    }

    /// The shell's ids for the files, freed when dropped.
    struct ItemIds(Vec<*mut ITEMIDLIST>);

    impl ItemIds {
        fn of(paths: &[&Path]) -> Result<Self> {
            let mut ids = Self(Vec::with_capacity(paths.len()));
            for &path in paths {
                // The library's paths can mix in '/', which the shell turns down ("The parameter is
                // incorrect").
                let path = HSTRING::from(super::native(path).as_str());
                let mut id = std::ptr::null_mut();
                // SAFETY: a NUL-terminated path that outlives the call, and a pointer for the id.
                unsafe { SHParseDisplayName(&path, None::<&IBindCtx>, &mut id, 0, None) }?;
                ids.0.push(id);
            }
            Ok(ids)
        }

        fn as_const(&self) -> Vec<*const ITEMIDLIST> {
            self.0.iter().map(|&id| id.cast_const()).collect()
        }
    }

    impl Drop for ItemIds {
        fn drop(&mut self) {
            for &id in &self.0 {
                // SAFETY: each id came from SHParseDisplayName, allocated with the COM allocator,
                // and is freed once.
                unsafe { CoTaskMemFree(Some(id.cast_const().cast())) };
            }
        }
    }
}

/// The path with the OS's own separators, for showing to people and handing to other programs.
pub fn native(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) { text.replace('/', "\\") } else { text.into_owned() }
}

/// What the OS calls the place deleted files go.
pub fn trash_name() -> &'static str {
    if cfg!(windows) { "Recycle Bin" } else { "Trash" }
}

/// Moves files and folders to the Recycle Bin (the Trash on macOS and Linux) in one go. It can
/// take a while for many files or a big folder: not for the UI thread.
pub fn move_to_trash(paths: &[PathBuf]) -> Result<(), trash::Error> {
    trash::delete_all(paths)
}

/// Whether what's deleted at `path` can be put back. Windows keeps a Recycle Bin only on fixed
/// drives: on a USB stick or a network drive the shell deletes for good (after its own warning).
/// On macOS and Linux, `trash` fails instead of deleting for good.
pub fn has_trash(path: &Path) -> bool {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
        use windows_sys::Win32::System::WindowsProgramming::DRIVE_FIXED;
        // The drive's root ("F:\", "\\server\share\"), which is what GetDriveTypeW asks for.
        let Some(root) = path.ancestors().last() else { return false };
        let mut wide: Vec<u16> = root.as_os_str().encode_wide().collect();
        if wide.last() != Some(&u16::from(b'\\')) {
            wide.push(u16::from(b'\\'));
        }
        wide.push(0);
        // SAFETY: a NUL-terminated UTF-16 string that outlives the call, which only reads it.
        let kind = unsafe { GetDriveTypeW(wide.as_ptr()) };
        kind == DRIVE_FIXED
    }
    #[cfg(not(target_os = "windows"))]
    {
        // Elsewhere `trash` fails rather than deleting for good, so there's nothing to ask.
        let _ = path;
        true
    }
}

/// Gives the window's title bar the app's colours instead of the system's light ones: dark on
/// Windows 10, and on Windows 11 exactly `caption`, with `text` for the title and `border` around
/// the window. Elsewhere the window's dark theme (`ViewportCommand::SetTheme`) is all there is.
pub fn color_title_bar(
    window: &impl raw_window_handle::HasWindowHandle,
    caption: [u8; 3],
    text: [u8; 3],
    border: [u8; 3],
) {
    #[cfg(target_os = "windows")]
    {
        use raw_window_handle::RawWindowHandle;
        use windows_sys::Win32::Graphics::Dwm::{
            DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
            DwmSetWindowAttribute,
        };
        let Ok(handle) = window.window_handle() else { return };
        let RawWindowHandle::Win32(win32) = handle.as_raw() else { return };
        let hwnd = win32.hwnd.get() as *mut core::ffi::c_void;
        let set = |attribute: i32, value: u32| {
            // SAFETY: the window outlives the call (its handle is borrowed), and the value is the
            // 4-byte BOOL or COLORREF the attribute takes, which DWM only reads.
            unsafe { DwmSetWindowAttribute(hwnd, attribute as u32, (&raw const value).cast(), 4) };
        };
        let color_ref = |[r, g, b]: [u8; 3]| u32::from(r) | u32::from(g) << 8 | u32::from(b) << 16;
        set(DWMWA_USE_IMMERSIVE_DARK_MODE, 1);
        // Windows 10 refuses these and keeps the dark bar.
        set(DWMWA_CAPTION_COLOR, color_ref(caption));
        set(DWMWA_TEXT_COLOR, color_ref(text));
        set(DWMWA_BORDER_COLOR, color_ref(border));
    }
    #[cfg(not(target_os = "windows"))]
    {
        // Only Windows lets the title bar take colours.
        let _ = (window, caption, text, border);
    }
}

/// The OS's own UI fonts, as their files hold them; `None` where one isn't found (egui's own
/// font stands in).
pub struct SystemFonts {
    pub regular: Option<Vec<u8>>,
    pub semibold: Option<Vec<u8>>,
    pub monospace: Option<Vec<u8>>,
}

/// Reads the OS's UI fonts from where it keeps them.
pub fn system_fonts() -> SystemFonts {
    SystemFonts {
        regular: read_first(REGULAR_FONTS),
        semibold: read_first(SEMIBOLD_FONTS),
        monospace: read_first(MONOSPACE_FONTS),
    }
}

#[cfg(windows)]
const REGULAR_FONTS: &[&str] = &["%WINDIR%/Fonts/segoeui.ttf"];
#[cfg(windows)]
const SEMIBOLD_FONTS: &[&str] = &["%WINDIR%/Fonts/seguisb.ttf", "%WINDIR%/Fonts/segoeuib.ttf"];
#[cfg(windows)]
const MONOSPACE_FONTS: &[&str] = &["%WINDIR%/Fonts/CascadiaMono.ttf", "%WINDIR%/Fonts/consola.ttf"];

#[cfg(target_os = "macos")]
const REGULAR_FONTS: &[&str] = &["/System/Library/Fonts/SFNS.ttf"];
#[cfg(target_os = "macos")]
const SEMIBOLD_FONTS: &[&str] = &[];
#[cfg(target_os = "macos")]
const MONOSPACE_FONTS: &[&str] = &["/System/Library/Fonts/SFNSMono.ttf"];

#[cfg(not(any(windows, target_os = "macos")))]
const REGULAR_FONTS: &[&str] = &[
    "/usr/share/fonts/truetype/inter/Inter-Regular.ttf",
    "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
];
#[cfg(not(any(windows, target_os = "macos")))]
const SEMIBOLD_FONTS: &[&str] = &[
    "/usr/share/fonts/truetype/inter/Inter-SemiBold.ttf",
    "/usr/share/fonts/truetype/noto/NotoSans-SemiBold.ttf",
    "/usr/share/fonts/noto/NotoSans-SemiBold.ttf",
];
#[cfg(not(any(windows, target_os = "macos")))]
const MONOSPACE_FONTS: &[&str] = &["/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"];

fn read_first(candidates: &[&str]) -> Option<Vec<u8>> {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".to_owned());
    candidates.iter().find_map(|path| std::fs::read(path.replace("%WINDIR%", &windir)).ok())
}

/// How much memory the app holds right now (its private bytes), where the OS says.
pub fn memory_in_use() -> Option<u64> {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        let mut counters: PROCESS_MEMORY_COUNTERS_EX = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        // SAFETY: `counters` is a writable PROCESS_MEMORY_COUNTERS_EX of the size given, and the
        // current-process pseudo handle needs no closing.
        let ok = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), (&raw mut counters).cast(), size) };
        (ok != 0).then_some(counters.PrivateUsage as u64)
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    /// Stands in for the window where a test never gets as far as needing one.
    struct NoWindow;

    impl raw_window_handle::HasWindowHandle for NoWindow {
        fn window_handle(&self) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
            Err(raw_window_handle::HandleError::Unavailable)
        }
    }

    #[test]
    fn files_that_are_gone_are_not_dragged_out() {
        let dir = TempDir::new("drag-out");
        let gone = dir.path().join("gone.png");
        assert!(matches!(drag_files_out(&NoWindow, &[gone]), Err(DragError::Missing)));
    }

    /// It puts files on the real clipboard, so it only runs when asked for:
    /// `cargo test copied_files_can_be_pasted -- --ignored`.
    #[test]
    #[ignore]
    fn copied_files_can_be_pasted() {
        let dir = std::env::temp_dir().join("asset-browser-copy-test");
        std::fs::create_dir_all(&dir).unwrap();
        // Nothing to copy leaves the clipboard as it was.
        let mut clipboard = arboard::Clipboard::new().unwrap();
        clipboard.set_text("copied before").unwrap();
        assert!(copy_files(&[dir.join("gone.png")]).is_err());
        assert_eq!(clipboard.get_text().unwrap(), "copied before");
        let files = [dir.join("hero.png"), dir.join("jump sound.wav")];
        for file in &files {
            std::fs::write(file, b"made up").unwrap();
        }
        copy_files(&files).unwrap();
        let pasted = clipboard.get().file_list().unwrap();
        let names =
            |paths: &[PathBuf]| paths.iter().map(|path| path.file_name().map(ToOwned::to_owned)).collect::<Vec<_>>();
        assert_eq!(names(&pasted), names(&files));
        assert!(pasted.iter().all(|path| path.is_file()), "{pasted:?}");
        // And only them: the text from before is gone.
        assert!(clipboard.get_text().is_err());
        // The files stay, to paste somewhere by hand; the next run writes them again.
    }
}
