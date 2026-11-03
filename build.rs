//! Puts the app icon and name into the Windows executable, where Explorer, the taskbar and Task
//! Manager look for them. The window's own icon is set when it opens, in `src/main.rs`. And the key
//! that feedback is sent to GitHub with, when the build has one (`src/feedback.rs`).

use std::time::{SystemTime, UNIX_EPOCH};

/// Bytes in the feedback key's mask, repeated over the key.
const MASK_LENGTH: u32 = 16;

fn main() {
    feedback_key();
    println!("cargo:rerun-if-changed=assets/icon.ico");
    // Build scripts run on the machine building, so this asks about the target, not `cfg!(windows)`.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon("assets/icon.ico").set("ProductName", "Asset Browser").set("FileDescription", "Asset Browser");
    // Without the Windows SDK's resource compiler the app still builds, just without its icon.
    if let Err(err) = resource.compile_for("asset_browser") {
        println!("cargo:warning=The executable has no icon: {err}");
    }
}

/// The release workflow builds with the key in `ASSET_BROWSER_FEEDBACK_TOKEN` (from its secret
/// `FEEDBACK_TOKEN`); it never goes in the repository. It's kept in the executable scrambled with a
/// mask made for each build, so tools looking for GitHub keys in files don't find it as text. That
/// stops nobody who looks: what the key may do is limited to the repository's issues instead (see
/// docs/releasing.md). Without it the file holds no key, and feedback opens GitHub's page instead.
fn feedback_key() {
    println!("cargo:rerun-if-env-changed=ASSET_BROWSER_FEEDBACK_TOKEN");
    let key = std::env::var("ASSET_BROWSER_FEEDBACK_TOKEN").unwrap_or_default();
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.subsec_nanos());
    // Odd bytes only: a zero in the mask would leave that byte of the key as it is.
    let mask: Vec<u8> = (0..MASK_LENGTH).map(|at| (nanos.rotate_left(at * 5) as u8) | 1).collect();
    let scrambled: Vec<u8> = key.trim().bytes().zip(mask.iter().cycle()).map(|(byte, mask)| byte ^ mask).collect();
    let code = format!("const KEY_MASK: &[u8] = &{mask:?};\nconst SCRAMBLED_KEY: &[u8] = &{scrambled:?};\n");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    std::fs::write(out.join("feedback_key.rs"), code).expect("write the feedback key's file");
}
