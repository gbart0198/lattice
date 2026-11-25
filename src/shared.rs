//! State shared between the window-manager thread and the bar (Rune) thread.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

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
    /// Short app name ("firefox"), shown when there's no icon or labels are on. Shared with
    /// the WM's window record, so publishing the model doesn't copy it.
    pub name: Arc<str>,
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
/// The hidden set changed since `hidden.txt` was last written (`flush_hidden`).
static HIDDEN_DIRTY: AtomicBool = AtomicBool::new(false);

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
        HIDDEN_DIRTY.store(true, Ordering::Relaxed);
    }
}

/// Writes `hidden.txt` if the set changed. The WM thread calls this once it has worked through
/// a batch of events, so a workspace switch that hides or shows N windows writes the file
/// once instead of N times.
pub fn flush_hidden() {
    if !HIDDEN_DIRTY.swap(false, Ordering::Relaxed) {
        return;
    }
    let g = HIDDEN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(set) = g.as_ref() {
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

// ---------------------------------------------------------------------------------------------
// The product tour (`lattice --tour`) runs as its own process and asks this one, through the
// "lattice-control" window, where things are on screen and what the user just did. These are
// the answers, kept current by the bar (anchors), the WM thread (workspace, layout, focus) and
// the keyboard hook (Escape).

/// Tour -> lattice: "what's going on?" `wParam` 1 while the tour runs, 0 as it ends. Returns
/// [`TourStatus::pack`].
pub const WM_TOUR_STATUS: u32 = WM_APP + 4;
/// Tour -> lattice: "where is this?" `wParam` an [`Anchor`]. Returns [`pack_rect`], 0 for
/// nowhere.
pub const WM_TOUR_RECT: u32 = WM_APP + 5;

/// Things the tour points at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    Bar = 0,
    Workspaces = 1,
    Layout = 2,
    Stats = 3,
    Launcher = 4,
    Focused = 5,
}

impl Anchor {
    pub fn from_index(i: usize) -> Option<Anchor> {
        [Anchor::Bar, Anchor::Workspaces, Anchor::Layout, Anchor::Stats, Anchor::Launcher, Anchor::Focused].get(i).copied()
    }
}

/// Screen rects (physical pixels, packed) of the bar's parts, published by the bar as it lays
/// out: bar, workspaces, layout button, stats.
static BAR_ANCHORS: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
/// The focused managed window (0: none), for the tour's spotlight.
static FOCUSED: AtomicIsize = AtomicIsize::new(0);
/// Active workspace index (low byte) and layout (next byte), from the WM's last publish.
static WS_LAYOUT: AtomicU32 = AtomicU32::new(0);
/// When the tour last checked in (`GetTickCount64` ms), and how often Escape was pressed for it.
static TOUR_SEEN: AtomicU64 = AtomicU64::new(0);
static TOUR_ESC: AtomicU32 = AtomicU32::new(0);

/// A rect as 4 x 16 bits (x, y signed; w, h unsigned); 0 means none.
pub fn pack_rect(r: crate::layout::Rect) -> u64 {
    if r.w <= 0 || r.h <= 0 {
        return 0;
    }
    let c = |v: i32| (v.clamp(i16::MIN as i32, i16::MAX as i32) as i16 as u16) as u64;
    let u = |v: i32| v.clamp(1, u16::MAX as i32) as u64;
    c(r.x) | c(r.y) << 16 | u(r.w) << 32 | u(r.h) << 48
}

pub fn unpack_rect(v: u64) -> Option<crate::layout::Rect> {
    if v == 0 {
        return None;
    }
    let s = |shift: u32| (v >> shift) as u16 as i16 as i32;
    let u = |shift: u32| (v >> shift) as u16 as i32;
    Some(crate::layout::Rect::new(s(0), s(16), u(32), u(48)))
}

pub fn set_bar_anchor(a: Anchor, r: crate::layout::Rect) {
    if let Some(slot) = BAR_ANCHORS.get(a as usize) {
        slot.store(pack_rect(r), Ordering::Relaxed);
    }
}

pub fn set_focus_state(focused: Option<Hwnd>, workspace: usize, layout: u8) {
    // Keep the last managed window while focus is elsewhere (the tour's card, the launcher):
    // it's still the one with the border. One that has gone or been hidden reads as none.
    if let Some(h) = focused {
        FOCUSED.store(h, Ordering::Relaxed);
    }
    WS_LAYOUT.store((workspace.min(255) as u32) | (layout as u32) << 8, Ordering::Relaxed);
}

/// What the tour learns from one status call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TourStatus {
    pub workspace: u8,
    pub layout: u8,
    pub launcher: bool,
    /// Escape presses so far (wraps).
    pub escapes: u16,
}

impl TourStatus {
    pub fn pack(self) -> u64 {
        self.workspace as u64 | (self.layout as u64) << 8 | (self.launcher as u64) << 16 | (self.escapes as u64) << 32
    }

    pub fn unpack(v: u64) -> Self {
        TourStatus { workspace: v as u8, layout: (v >> 8) as u8, launcher: (v >> 16) & 1 == 1, escapes: (v >> 32) as u16 }
    }
}

fn now_ms() -> u64 {
    unsafe { windows::Win32::System::SystemInformation::GetTickCount64() }
}

/// The control window's answer to [`WM_TOUR_STATUS`] (on the WM thread).
pub fn tour_status(running: bool) -> u64 {
    TOUR_SEEN.store(if running { now_ms() } else { 0 }, Ordering::Relaxed);
    let wl = WS_LAYOUT.load(Ordering::Relaxed);
    TourStatus {
        workspace: wl as u8,
        layout: (wl >> 8) as u8,
        launcher: crate::launcher::is_visible(),
        escapes: TOUR_ESC.load(Ordering::Relaxed) as u16,
    }
    .pack()
}

/// The control window's answer to [`WM_TOUR_RECT`] (on the WM thread).
pub fn tour_rect(anchor: usize) -> u64 {
    match Anchor::from_index(anchor) {
        Some(Anchor::Launcher) => crate::launcher::screen_rect().map_or(0, |(x, y, w, h)| pack_rect(crate::layout::Rect::new(x, y, w, h))),
        Some(Anchor::Focused) => {
            let h = FOCUSED.load(Ordering::Relaxed);
            if h == 0 || !win::is_visible(h) {
                return 0;
            }
            win::frame_rect(h).map_or(0, pack_rect)
        }
        Some(a) => BAR_ANCHORS[a as usize].load(Ordering::Relaxed),
        None => 0,
    }
}

/// The tour checked in within the last moment and hasn't said it ended. A tour process that
/// died stops counting after a second and a half, so Escape is never kept from other apps.
pub fn tour_active() -> bool {
    let seen = TOUR_SEEN.load(Ordering::Relaxed);
    seen != 0 && now_ms().saturating_sub(seen) < 1500
}

/// The keyboard hook saw Escape while the tour ran.
pub fn tour_escape() {
    TOUR_ESC.fetch_add(1, Ordering::Relaxed);
}
