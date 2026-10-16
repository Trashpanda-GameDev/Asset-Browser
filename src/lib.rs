//! Asset Browser: a fast, native browser for folders of game assets.
//!
//! The app (`src/main.rs`) and the tools (`src/bin/`) share these modules. Only what the tools use is `pub`, so the
//! compiler finds anything nothing uses.

pub mod animated;
pub mod app;
pub mod asset;
pub mod audio;
pub(crate) mod changelog;
pub mod db;
pub(crate) mod deletion;
pub(crate) mod detail;
pub mod diag;
pub mod groups;
pub(crate) mod keymap;
pub mod library;
pub(crate) mod memory;
pub mod model;
pub mod paths;
pub(crate) mod platform;
pub(crate) mod psd_composite;
pub mod scan;
pub(crate) mod sheets;
pub(crate) mod storage;
pub mod thumbs;
pub mod ui;
pub(crate) mod updates;
pub(crate) mod watch;

#[cfg(test)]
mod test_support;
