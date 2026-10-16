//! The keys the window's settings are saved under in `library.sqlite`. They're stored, so a key
//! never changes: a test pins them. The shortcuts' keys are the keymap's (`keys.<action>`).

use crate::ui::mode::Mode;

pub const MODE: &str = "ui.mode";
pub const INSPECTOR: &str = "ui.inspector";
pub const AUTOPLAY: &str = "sound.autoplay";
pub const LOOP: &str = "sound.loop";
pub const VOLUME: &str = "sound.volume";
pub const BACKDROP: &str = "image.backdrop";
pub const TURNTABLE: &str = "model.turntable";
pub const WIREFRAME: &str = "model.wireframe";
/// The memory limit, in MB.
pub const MEMORY: &str = "memory.limit_mb";
/// The last version whose "What's new" was shown (or would have been, on a first install).
pub const WHATS_NEW: &str = "app.whats_new_seen";
/// The one folder that was open, from before the library could have several: it becomes the
/// library's first folder.
pub const LAST_ROOT: &str = "last_root";

/// A mode's own setting, `ui.<mode>.<name>`: its layout, tile size and orders.
pub fn of_mode(mode: Mode, name: &str) -> String {
    format!("ui.{}.{name}", mode.key())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_stay_as_they_are_saved() {
        let keys =
            [MODE, INSPECTOR, AUTOPLAY, LOOP, VOLUME, BACKDROP, TURNTABLE, WIREFRAME, MEMORY, WHATS_NEW, LAST_ROOT];
        assert_eq!(
            keys,
            [
                "ui.mode",
                "ui.inspector",
                "sound.autoplay",
                "sound.loop",
                "sound.volume",
                "image.backdrop",
                "model.turntable",
                "model.wireframe",
                "memory.limit_mb",
                "app.whats_new_seen",
                "last_root",
            ]
        );
        assert_eq!(of_mode(Mode::Sounds, "group_sort"), "ui.sounds.group_sort");
    }
}
