//! The launcher's OS side, one back end per OS with the same modules and functions (like
//! lattice's own `crate::platform`, kept separate so the launcher imports nothing from the rest
//! of lattice):
//!
//! - `system`: the app catalog, icons, launching, file actions, focus;
//! - `control`: command mode's system commands (power, audio output, Wi-Fi);
//! - `everything`: voidtools Everything's IPC (Windows only; elsewhere it's never running);
//! - `files`: what the file index needs (hidden files, a background thread, the home folder);
//! - `bench`: what `--launcher-bench` needs (on screen?, typing).

use std::sync::Arc;

/// One launchable app.
#[derive(Clone, Debug)]
pub struct App {
    /// What the Start menu (or Applications folder) calls it.
    pub name: Arc<str>,
    /// How the OS names it. On Windows its AppsFolder parsing name: an AppUserModelID
    /// ("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App") or a path, possibly under a
    /// known-folder GUID ("{6D809377-...}\cmd.exe").
    pub id: Arc<str>,
}

/// Icon pixels: straight-alpha RGBA, `size` x `size`.
pub struct Icon {
    pub size: u32,
    pub rgba: Arc<[u8]>,
}

#[cfg(windows)]
#[path = "windows/mod.rs"]
mod imp;

#[cfg(target_os = "macos")]
#[path = "macos/mod.rs"]
mod imp;

pub use imp::*;
