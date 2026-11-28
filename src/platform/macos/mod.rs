//! The macOS back end, not written yet: every function has the Windows back end's signature
//! and does nothing useful. Each says so in the log, once (`unsupported`), so a macOS build
//! starts, runs the bar, and is honest about what's missing.

pub mod anim;
pub mod border;
pub mod control;
pub mod hooks;
pub mod icon;
pub mod ipc;
pub mod paths;
pub mod process;
pub mod sysinfo;
pub mod update;
pub mod win;

/// Logs that `what` isn't implemented on macOS yet, the first time it's asked for.
pub(crate) fn unsupported(what: &'static str) {
    use std::collections::HashSet;
    use std::sync::Mutex;
    static SEEN: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);
    let mut g = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if g.get_or_insert_with(HashSet::new).insert(what) {
        log::warn!("macOS: {what} isn't implemented yet");
    }
}
