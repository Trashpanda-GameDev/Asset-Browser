//! The few things that work differently on each OS. Nothing else in the app should call into a
//! specific platform.

use std::io;
use std::path::Path;
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
