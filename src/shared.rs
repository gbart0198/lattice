//! State shared between the window-manager thread and the bar (Rune) thread.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_APP};

use crate::config::Command;
use crate::win::{self, Hwnd};

/// Posted to the WM thread: "drain your queues".
pub const WM_LATTICE_WAKE: u32 = WM_APP + 1;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WsInfo {
    /// 1-based number shown in the bar.
    pub number: usize,
    /// Shown on some monitor.
    pub visible: bool,
    /// Shown on the monitor that has focus.
    pub focused: bool,
    /// The windows on it, in tiling order.
    pub apps: Vec<AppInfo>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AppInfo {
    /// Short app name ("firefox"), shown when there's no icon or labels are on.
    pub name: String,
    pub icon: Option<crate::icon::AppIcon>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BarModel {
    pub workspaces: Vec<WsInfo>,
    pub title: String,
    pub layout: String,
}

static WM_THREAD: AtomicU32 = AtomicU32::new(0);
static BAR_MODEL: Mutex<Option<BarModel>> = Mutex::new(None);
pub static BAR_GEN: AtomicU64 = AtomicU64::new(0);
static BAR_WAKER: OnceLock<rune::Waker> = OnceLock::new();
static COMMANDS: Mutex<Vec<Command>> = Mutex::new(Vec::new());
/// Every window lattice has hidden, so they can be shown again on exit or crash.
static HIDDEN: Mutex<Option<HashSet<Hwnd>>> = Mutex::new(None);

pub fn set_wm_thread(id: u32) {
    WM_THREAD.store(id, Ordering::SeqCst);
}

pub fn wake_wm() {
    let id = WM_THREAD.load(Ordering::SeqCst);
    if id != 0 {
        unsafe {
            let _ = PostThreadMessageW(id, WM_LATTICE_WAKE, WPARAM(0), LPARAM(0));
        }
    }
}

/// Queue a command for the WM thread (used by the bar's click handlers).
pub fn send_command(cmd: Command) {
    COMMANDS.lock().unwrap().push(cmd);
    wake_wm();
}

pub fn take_commands() -> Vec<Command> {
    std::mem::take(&mut *COMMANDS.lock().unwrap())
}

pub fn publish(model: BarModel) {
    let mut slot = BAR_MODEL.lock().unwrap();
    if slot.as_ref() == Some(&model) {
        return;
    }
    *slot = Some(model);
    drop(slot);
    BAR_GEN.fetch_add(1, Ordering::SeqCst);
    wake_bar();
}

pub fn bar_model() -> BarModel {
    BAR_MODEL.lock().unwrap().clone().unwrap_or_default()
}

pub fn set_bar_waker(w: rune::Waker) {
    let _ = BAR_WAKER.set(w);
}

pub fn wake_bar() {
    if let Some(w) = BAR_WAKER.get() {
        w();
    }
}

pub fn mark_hidden(hwnd: Hwnd, hidden: bool) {
    let mut g = HIDDEN.lock().unwrap_or_else(|e| e.into_inner());
    let set = g.get_or_insert_with(HashSet::new);
    let changed = if hidden { set.insert(hwnd) } else { set.remove(&hwnd) };
    if changed {
        save_hidden(set);
    }
}

/// Shows every window lattice hid. Safe to call from a panic hook.
pub fn restore_hidden() {
    let mut g = HIDDEN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(set) = g.take() {
        for hwnd in set {
            if win::is_window(hwnd) {
                win::show(hwnd);
            }
        }
    }
    let _ = std::fs::remove_file(hidden_path());
}

// The hidden set is also kept on disk, so windows come back even if lattice is killed
// (Task Manager, a crash the panic hook can't catch, a forced logoff).

fn hidden_path() -> std::path::PathBuf {
    crate::config::config_dir().join("hidden.txt")
}

/// One "hwnd pid" line per hidden window; the pid guards against a reused handle.
fn save_hidden(set: &HashSet<Hwnd>) {
    let text: String = set.iter().map(|h| format!("{h:x} {}\n", win::pid(*h))).collect();
    let _ = std::fs::create_dir_all(crate::config::config_dir());
    let _ = std::fs::write(hidden_path(), text);
}

/// Called at startup: shows windows a previous lattice hid and never got to restore.
pub fn recover_hidden_from_last_run() {
    let path = hidden_path();
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    let mut restored = 0;
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let (Some(h), Some(p)) = (parts.next(), parts.next()) else { continue };
        let (Ok(hwnd), Ok(pid)) = (isize::from_str_radix(h, 16), p.parse::<u32>()) else { continue };
        if win::is_window(hwnd) && win::pid(hwnd) == pid && !win::is_visible(hwnd) {
            win::show(hwnd);
            restored += 1;
        }
    }
    if restored > 0 {
        log::warn!("restored {restored} window(s) a previous lattice run left hidden");
    }
    let _ = std::fs::remove_file(path);
}
