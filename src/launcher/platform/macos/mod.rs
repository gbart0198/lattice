//! The launcher on macOS: not written yet (the Applications folder and Spotlight, NSWorkspace
//! icons and launching, IOKit, CoreAudio and CoreWLAN). The launcher opens, finds no apps and
//! offers no system commands; file search uses lattice's own index.

pub mod bench;
pub mod control;
pub mod everything;
pub mod files;
pub mod system;

/// Logs that `what` isn't implemented on macOS yet, the first time it's asked for.
fn unsupported(what: &'static str) {
    use std::collections::HashSet;
    use std::sync::Mutex;
    static SEEN: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);
    let mut g = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if g.get_or_insert_with(HashSet::new).insert(what) {
        log::warn!("macOS: launcher {what} isn't implemented yet");
    }
}
