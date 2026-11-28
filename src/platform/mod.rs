//! Everything lattice asks of the operating system, behind one API with a back end per OS:
//! `windows/` (Win32, the real one) and `macos/` (stubs for now: each says what it can't do
//! yet, once, in the log). The rest of lattice is the same code on both and only talks to the
//! OS through here.
//!
//! The back ends are picked at compile time and have the same modules with the same functions,
//! so a call that one of them lacks fails the build for that OS (`scripts/check-platforms.sh`
//! checks both). The modules:
//!
//! - `win`: windows and monitors (list, place, show, hide, focus);
//! - `hooks`: the WM thread's event source (window events, the keyboard hook, display changes)
//!   and waking that thread;
//! - `border`: the focus border's windows;
//! - `anim`: the animation back end (live previews) and the frame clock, plus
//!   `--animation-preview`;
//! - `icon`: app icons; `sysinfo`: CPU, memory, battery and the local time;
//! - `ipc`: the transport under `lattice msg/query/subscribe`;
//! - `control`: the tour and the settings window talking to the running lattice;
//! - `process`: starting programs, one instance at a time, consoles, DPI;
//! - `paths`: where config and data live; `update`: where the installer put lattice.

use crate::layout::Rect;

/// A top-level window, as the OS names it: an `HWND` on Windows (a `CGWindowID` on macOS, once
/// that back end exists). Plain data, so it can live in maps, cross threads and go in JSON.
pub type WindowId = isize;

/// Facts about a window that decide whether (and how) lattice manages it.
pub struct Candidate {
    pub exe: String,
    pub class: String,
    pub title: String,
    pub resizable: bool,
}

#[derive(Clone, Debug)]
pub struct MonitorInfo {
    pub handle: isize,
    pub rect: Rect,
    /// Work area: the monitor minus the taskbar (or menu bar and Dock).
    pub work: Rect,
    pub primary: bool,
    pub scale: f32,
}

#[cfg(windows)]
#[path = "windows/mod.rs"]
mod imp;

#[cfg(target_os = "macos")]
#[path = "macos/mod.rs"]
mod imp;

#[cfg(not(any(windows, target_os = "macos")))]
compile_error!("lattice runs on Windows and (in progress) macOS");

pub use imp::*;
