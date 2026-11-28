//! The WM thread's event source on macOS: not written yet (Accessibility observers for window
//! events, a CGEventTap for keybinds, display reconfiguration callbacks). Until then the WM
//! thread only wakes for what other threads send it (bar clicks, `lattice msg` once IPC
//! exists), through `wake`.

use std::sync::{Condvar, Mutex};

static WOKEN: Mutex<bool> = Mutex::new(false);
static WAKE: Condvar = Condvar::new();

/// Wakes the WM thread to drain its queues. Any thread.
pub fn wake() {
    *WOKEN.lock().unwrap_or_else(|e| e.into_inner()) = true;
    WAKE.notify_one();
}

pub struct Sources;

pub fn init() {}

pub fn install() -> Sources {
    super::unsupported("window events and keybinds");
    Sources
}

pub fn uninstall(_: Sources) {}

/// Blocks until `wake` is called. Never ends the loop by itself: `Wm::quit` does.
pub fn wait() -> bool {
    let mut woken = WOKEN.lock().unwrap_or_else(|e| e.into_inner());
    while !*woken {
        woken = WAKE.wait(woken).unwrap_or_else(|e| e.into_inner());
    }
    *woken = false;
    true
}
