//! Puts the app icon and name into the Windows executable, where Explorer, the taskbar and Task
//! Manager look for them. The window's own icon is set when it opens, in `src/main.rs`.

fn main() {
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
