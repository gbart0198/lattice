//! State shared between the window-manager thread and the bar (Rune) thread.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::config::Command;
use crate::platform::{WindowId, win};

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

static BAR_MODEL: Mutex<Option<BarModel>> = Mutex::new(None);
pub static BAR_GEN: AtomicU64 = AtomicU64::new(0);
static BAR_WAKER: OnceLock<rune::Waker> = OnceLock::new();
static COMMANDS: Mutex<Vec<Command>> = Mutex::new(Vec::new());
/// Every window lattice has hidden, so they can be shown again on exit or crash.
static HIDDEN: Mutex<Option<HashSet<WindowId>>> = Mutex::new(None);
/// The hidden set changed since `hidden.txt` was last written (`flush_hidden`).
static HIDDEN_DIRTY: AtomicBool = AtomicBool::new(false);
/// Windows made transparent for an animation, so they can be made opaque again on exit or
/// crash (an app left invisible is worse than one left hidden).
static FADED: Mutex<Option<HashMap<WindowId, win::Faded>>> = Mutex::new(None);

/// Wakes the WM thread to drain its queues (`hooks.rs`). Any thread.
pub fn wake_wm() {
    crate::platform::hooks::wake();
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

/// Wakes the bar to redraw, unless it's hidden behind a fullscreen app.
pub fn wake_bar() {
    if bar_suppressed() {
        return;
    }
    if let Some(w) = BAR_WAKER.get() {
        w();
    }
}

/// Set by the WM thread while a fullscreen app covers the bar's monitor and the bar is hidden:
/// nothing wakes the bar to redraw then (its model and readings still update, unseen). Clearing
/// it wakes the bar once to catch up.
static BAR_SUPPRESSED: AtomicBool = AtomicBool::new(false);

pub fn bar_suppressed() -> bool {
    BAR_SUPPRESSED.load(Ordering::SeqCst)
}

pub fn set_bar_suppressed(on: bool) {
    BAR_SUPPRESSED.store(on, Ordering::SeqCst);
    if !on {
        wake_bar();
    }
}

pub fn mark_hidden(hwnd: WindowId, hidden: bool) {
    let mut g = HIDDEN.lock().unwrap_or_else(|e| e.into_inner());
    let set = g.get_or_insert_with(HashSet::new);
    let changed = if hidden { set.insert(hwnd) } else { set.remove(&hwnd) };
    if changed {
        HIDDEN_DIRTY.store(true, Ordering::Relaxed);
    }
}

pub fn mark_faded(hwnd: WindowId, faded: Option<win::Faded>) {
    let mut g = FADED.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_with(HashMap::new);
    let changed = match faded {
        Some(f) => map.insert(hwnd, f) != Some(f),
        None => map.remove(&hwnd).is_some(),
    };
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
    drop(g);
    let g = FADED.lock().unwrap_or_else(|e| e.into_inner());
    save_faded(g.as_ref());
}

/// Shows every window lattice hid, and makes every one it made transparent opaque again. Safe
/// to call from a panic hook.
pub fn restore_hidden() {
    let mut g = FADED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(map) = g.take() {
        for (hwnd, f) in map {
            if win::is_window(hwnd) {
                win::unfade(hwnd, f);
            }
        }
    }
    drop(g);
    let _ = std::fs::remove_file(faded_path());
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
fn save_hidden(set: &HashSet<WindowId>) {
    let text: String = set.iter().map(|h| format!("{h:x} {}\n", win::pid(*h))).collect();
    let _ = std::fs::create_dir_all(crate::config::config_dir());
    let _ = std::fs::write(hidden_path(), text);
}

fn faded_path() -> std::path::PathBuf {
    crate::config::config_dir().join("faded.txt")
}

/// One "hwnd pid faded" line per transparent window, `faded` as `Faded::encode` writes it.
fn save_faded(map: Option<&HashMap<WindowId, win::Faded>>) {
    let Some(map) = map.filter(|m| !m.is_empty()) else {
        let _ = std::fs::remove_file(faded_path());
        return;
    };
    let text: String = map
        .iter()
        .map(|(h, f)| format!("{h:x} {} {}\n", win::pid(*h), f.encode()))
        .collect();
    let _ = std::fs::create_dir_all(crate::config::config_dir());
    let _ = std::fs::write(faded_path(), text);
}

fn parse_faded(line: &str) -> Option<(WindowId, u32, win::Faded)> {
    let mut p = line.split_whitespace();
    let hwnd = isize::from_str_radix(p.next()?, 16).ok()?;
    let pid = p.next()?.parse().ok()?;
    Some((hwnd, pid, win::Faded::decode(p)?))
}

/// Called at startup: shows windows a previous lattice hid and never got to restore.
pub fn recover_hidden_from_last_run() {
    if let Ok(text) = std::fs::read_to_string(faded_path()) {
        let mut restored = 0;
        for (hwnd, pid, f) in text.lines().filter_map(parse_faded) {
            if win::is_window(hwnd) && win::pid(hwnd) == pid {
                win::unfade(hwnd, f);
                restored += 1;
            }
        }
        if restored > 0 {
            log::warn!("made {restored} window(s) a previous lattice run left transparent opaque again");
        }
        let _ = std::fs::remove_file(faded_path());
    }
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
// The product tour (`lattice --tour`) runs as its own process and asks this one
// (`platform::control`) where things are on screen and what the user just did. These are the
// answers, kept current by the bar (anchors), the WM thread (workspace, layout, focus) and the
// keyboard hook (Escape).

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
/// When the tour last checked in (`now_ms`), and how often Escape was pressed for it.
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

pub fn set_focus_state(focused: Option<WindowId>, workspace: usize, layout: u8) {
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

/// Milliseconds on a monotonic clock, never 0 (0 means "never" in `TOUR_SEEN`).
fn now_ms() -> u64 {
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64 + 1
}

/// The answer to the tour's status call (`platform::control::tour_status`), on the WM thread.
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

/// The answer to the tour's "where is this?" (`platform::control::tour_rect`), on the WM thread.
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
