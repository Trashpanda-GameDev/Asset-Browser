//! The few things that work differently on each OS: opening and revealing files, and deleting
//! them to the Recycle Bin or Trash. Nothing else in the app should call into a specific platform.

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
pub fn move_to_trash(paths: &[PathBuf]) -> Result<(), String> {
    trash::delete_all(paths).map_err(|err| err.to_string())
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
