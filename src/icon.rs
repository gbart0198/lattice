//! App icons for the bar, as straight-alpha RGBA that Rune can display. The OS finds and renders
//! them (`platform::icon`).

use std::sync::Arc;

pub use crate::platform::icon::{for_exe, for_window};

/// Rendered icon size in pixels (the bar shows it at ~16pt; 32px stays sharp at 200%).
pub const SIZE: u32 = 32;

/// An app icon, compared by `key` so the bar model stays cheap to diff.
#[derive(Clone, Debug)]
pub struct AppIcon {
    /// A hash of the pixels: windows with the same icon (ten terminals) share one key, so the
    /// bar keeps one image for all of them, and copying the key allocates nothing.
    pub key: u64,
    pub rgba: Arc<[u8]>,
}

impl PartialEq for AppIcon {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl AppIcon {
    pub fn new(rgba: Arc<[u8]>) -> Self {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        rgba.hash(&mut h);
        AppIcon { key: h.finish(), rgba }
    }
}
