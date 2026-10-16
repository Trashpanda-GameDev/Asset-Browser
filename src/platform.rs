//! The few things that work differently on each OS: opening and revealing files, copying them to
//! the clipboard, deleting them to the Recycle Bin or Trash, the window's title bar and the
//! process's memory. Nothing else in the app should call into a specific platform.

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
        let _ = (window, caption, text, border);
    }
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
