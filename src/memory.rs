//! How much memory the app may use (Settings → Memory), shared out among what it holds on to:
//! pictures being decoded for thumbnails, thumbnails on the graphics card, sounds read ahead,
//! decoded model textures and GIF frames. What the window and the graphics driver need whatever
//! the setting comes off the top. A single very large file can still take more for a moment.

/// The setting, in megabytes.
pub const DEFAULT_MB: u64 = 1536;
pub const MIN_MB: u64 = 768;
pub const MAX_MB: u64 = 8192;
/// What the app needs whatever the setting: the window, the graphics driver's buffers, the library
/// and the sound output (about 400 MB private at startup on Windows with D3D12, measured).
const BASE_MB: u64 = 400;
/// A thumbnail on the graphics card: 192 × 192 RGBA.
const THUMBNAIL_BYTES: u64 = 192 * 192 * 4;

/// What each part may hold under a limit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shares {
    /// Bytes of pictures being decoded for thumbnails at once (`imaging::set_decode_limit`).
    pub decoding: u64,
    /// Thumbnails kept on the graphics card (`Previews::set_max_textures`).
    pub thumbnails: usize,
    /// Bytes of sounds read ahead of the selection (`audio::set_read_ahead_budget`).
    pub read_ahead: u64,
    /// Bytes of decoded textures kept for models (`model::set_texture_cache_budget`).
    pub model_textures: u64,
    /// Pixels of a GIF's frames (`animated::set_max_pixels`).
    pub animation_pixels: u64,
}

/// The parts' shares of `limit_mb`. At the default they're what the app used before the setting:
/// 512 MB decoding, 1,500 thumbnails, 160 MB of sounds, 128 MB of textures, 16 MP of frames.
pub fn shares(limit_mb: u64) -> Shares {
    let spare = (limit_mb.clamp(MIN_MB, MAX_MB) - BASE_MB) << 20;
    let part = |percent: u64| spare / 100 * percent;
    Shares {
        decoding: part(45),
        thumbnails: (part(19) / THUMBNAIL_BYTES).clamp(300, 8000) as usize,
        read_ahead: part(14),
        model_textures: part(11),
        // Held twice, on the CPU and on the graphics card, at 4 bytes a pixel; 4096² at most.
        animation_pixels: (part(11) / 8).min(4096 * 4096),
    }
}

/// Puts a limit into effect for the background threads, from their next piece of work. The
/// thumbnails on the graphics card are the window's to trim: it gets the shares back.
pub fn apply(limit_mb: u64) -> Shares {
    let shares = shares(limit_mb);
    crate::imaging::set_decode_limit(shares.decoding);
    crate::audio::set_read_ahead_budget(shares.read_ahead);
    crate::model::set_texture_cache_budget(shares.model_textures);
    crate::animated::set_max_pixels(shares.animation_pixels);
    shares
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_keeps_what_was_there() {
        let shares = shares(DEFAULT_MB);
        let mb = |bytes: u64| bytes >> 20;
        assert!((505..=515).contains(&mb(shares.decoding)), "{shares:?}");
        assert!((1450..=1550).contains(&shares.thumbnails), "{shares:?}");
        assert!((150..=165).contains(&mb(shares.read_ahead)), "{shares:?}");
        assert!((120..=130).contains(&mb(shares.model_textures)), "{shares:?}");
        assert!((15 << 20..=16 << 20).contains(&shares.animation_pixels), "{shares:?}");
    }

    #[test]
    fn more_memory_never_means_less() {
        let mut last = shares(MIN_MB);
        for limit in (MIN_MB..=MAX_MB).step_by(256) {
            let next = shares(limit);
            assert!(next.decoding >= last.decoding && next.thumbnails >= last.thumbnails);
            assert!(next.read_ahead >= last.read_ahead && next.model_textures >= last.model_textures);
            assert!(next.animation_pixels >= last.animation_pixels);
            last = next;
        }
        assert_eq!(shares(0), shares(MIN_MB), "below the least is the least");
        assert_eq!(shares(u64::MAX), shares(MAX_MB));
    }
}
