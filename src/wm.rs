//! The window manager's state machine. Runs entirely on the WM thread; `hooks.rs` feeds it
//! events and commands one at a time.
//!
//! Workspaces span every monitor: one workspace is active at a time, each window belongs to a
//! workspace *and* a monitor, and switching workspaces swaps the contents of all monitors at
//! once. Switching hides the old workspace's windows (`SW_HIDE`) and shows the new one's: no
//! Windows virtual desktops, so there's no delay. With animations on (`anim.rs`), every tile
//! placement glides there, and a switch slides the old workspace off screen before hiding it.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::anim::{self, After, Animator, Glide, Kind};
use crate::config::{self, AnimStyle, Command, Config, Dir, NewWindow, WsTarget};
use crate::events;
use crate::ipc;
use crate::layout::{self, LayoutKind, Params, Rect};
use crate::setups;
use crate::shared::{self, BarModel, WsInfo};
use crate::win::{self, Hwnd, MonitorInfo};

#[derive(Clone, Copy, Debug)]
pub enum Event {
    Show(Hwnd),
    Hide(Hwnd),
    Destroy(Hwnd),
    Cloaked(Hwnd),
    Uncloaked(Hwnd),
    Foreground(Hwnd),
    MinimizeStart(Hwnd),
    MinimizeEnd(Hwnd),
    MoveSizeEnd(Hwnd),
    NameChange(Hwnd),
    /// A window (any window, system-wide) moved or resized; only the focused one matters, for
    /// the focus border.
    Location(Hwnd),
}

struct Window {
    ws: usize,
    /// Index into `Wm::monitors`: which screen of its workspace the window lives on.
    mon: usize,
    floating: bool,
    minimized: bool,
    /// Nudge the window to redraw after showing it (rule `repaint = true`).
    repaint: bool,
    exe: String,
    class: String,
    /// The bar's short name for it (`app_name`), kept so publishing doesn't rebuild it.
    name: Arc<str>,
    icon: Option<crate::icon::AppIcon>,
    title: String,
    /// When it last had focus (`Wm::focus_seq`; 0: never), for window search's recency order.
    focused_at: u64,
}

struct Workspace {
    /// Every window on the workspace, all monitors; tiled ones are laid out in this order.
    windows: Vec<Hwnd>,
    layout: LayoutKind,
    split_ratio: f32,
    master_ratio: f32,
    /// Dwindle per-split ratios on each monitor, from directional resizes; splits past the end
    /// use `split_ratio`.
    splits: HashMap<usize, Vec<f32>>,
    last_focus: Option<Hwnd>,
    /// Last focused window on each monitor, for focus-monitor and closing windows.
    mon_focus: HashMap<usize, Hwnd>,
    fullscreen: Option<Hwnd>,
}

struct Monitor {
    info: MonitorInfo,
    bar_px: i32,
}

pub struct Wm {
    cfg: Config,
    windows: HashMap<Hwnd, Window>,
    workspaces: Vec<Workspace>,
    monitors: Vec<Monitor>,
    /// The workspace on screen (on every monitor).
    active: usize,
    focused_monitor: usize,
    /// Show/hide calls lattice made whose WinEvents haven't arrived yet, so they aren't
    /// mistaken for the app showing or hiding itself.
    pending_show: HashMap<Hwnd, u32>,
    pending_hide: HashMap<Hwnd, u32>,
    /// Windows lattice has asked to be hidden, and when.
    hidden: HashMap<Hwnd, Instant>,
    pub quit: bool,
    pub reload_keymap: bool,
    /// The focus border (None when turned off).
    border: Option<crate::border::Border>,
    /// The managed window that had focus at the last border update: its moves are the only
    /// location changes that matter.
    border_watch: Option<Hwnd>,
    /// Something happened that may move the border; `update_border` looks once per batch.
    border_dirty: bool,
    /// Windows gliding or sliding somewhere (empty, and free, when nothing moves).
    anim: Animator,
    /// Counts focus changes, to stamp `Window::focused_at`.
    focus_seq: u64,
    /// Script events (`events.rs`): the last snapshot, whether anything since could have
    /// changed it, and events that aren't state changes (a reload).
    events: events::Tracker,
    events_dirty: bool,
    pending_events: Vec<events::Event>,
    /// The saved workspace being restored, while it waits for apps it started.
    restore: Option<Restore>,
}

/// A saved workspace being restored: where its apps go, and which windows fill each spot. Apps
/// it started take their spot when their window opens, until `until`.
struct Restore {
    name: String,
    slots: Vec<setups::Slot>,
    filled: Vec<Option<Hwnd>>,
    /// Each workspace's saved split ratios. Retiling forgets ratios past the last split, so
    /// they're put back as each window arrives.
    splits: HashMap<usize, Vec<f32>>,
    until: Instant,
}

/// How long a restore waits for the apps it started to open a window.
const RESTORE_WAIT: Duration = Duration::from_secs(60);

/// What a directional search can land on: a window, or an empty monitor.
#[derive(Clone, Copy)]
enum Target {
    Window(Hwnd),
    Monitor(usize),
}

impl Wm {
    pub fn new(cfg: Config) -> Self {
        let mut wm = Wm {
            windows: HashMap::new(),
            workspaces: Vec::new(),
            monitors: Vec::new(),
            active: 0,
            focused_monitor: 0,
            pending_show: HashMap::new(),
            pending_hide: HashMap::new(),
            hidden: HashMap::new(),
            quit: false,
            reload_keymap: false,
            border: crate::border::Border::new(cfg.border.clone()),
            border_watch: None,
            border_dirty: true,
            anim: Animator::new(),
            focus_seq: 0,
            events: events::Tracker::default(),
            events_dirty: false,
            pending_events: Vec::new(),
            restore: None,
            cfg,
        };
        wm.build_workspaces();
        wm.refresh_monitors();
        wm.adopt_existing();
        wm.focused_monitor = wm.monitor_index_at(win::cursor_pos());
        wm.retile(wm.active);
        wm.publish();
        wm.publish_setups();
        wm
    }

    fn build_workspaces(&mut self) {
        let g = &self.cfg.general;
        while self.workspaces.len() < g.workspaces {
            self.workspaces.push(Workspace {
                windows: Vec::new(),
                layout: g.layout,
                split_ratio: g.split_ratio,
                master_ratio: g.master_ratio,
                splits: HashMap::new(),
                last_focus: None,
                mon_focus: HashMap::new(),
                fullscreen: None,
            });
        }
    }

    /// (Re)reads the monitor list. Windows keep their monitor; windows on a monitor that went
    /// away move to the primary one.
    fn refresh_monitors(&mut self) {
        let old: Vec<isize> = self.monitors.iter().map(|m| m.info.handle).collect();
        self.monitors = win::monitors()
            .into_iter()
            .map(|info| {
                let bar_px = if self.cfg.bar.enabled && info.primary { (self.cfg.bar.height * info.scale).round() as i32 } else { 0 };
                Monitor { info, bar_px }
            })
            .collect();
        let primary = self.monitors.iter().position(|m| m.info.primary).unwrap_or(0);
        let remap: Vec<usize> = old
            .iter()
            .map(|h| self.monitors.iter().position(|m| m.info.handle == *h).unwrap_or(primary))
            .collect();
        for w in self.windows.values_mut() {
            w.mon = remap.get(w.mon).copied().unwrap_or(primary);
        }
        for ws in &mut self.workspaces {
            ws.mon_focus = ws.mon_focus.drain().filter_map(|(m, h)| remap.get(m).map(|n| (*n, h))).collect();
            ws.splits = ws.splits.drain().filter_map(|(m, v)| remap.get(m).map(|n| (*n, v))).collect();
        }
        self.focused_monitor = self.focused_monitor.min(self.monitors.len().saturating_sub(1));
        // Keep the bar across the top of the primary monitor's (possibly new) work area.
        if let Some(m) = self.monitors.iter().find(|m| m.info.primary && m.bar_px > 0) {
            let bar = win::find_own_window("lattice-bar");
            if bar != 0 {
                win::place_overlay(bar, Rect::new(m.info.work.x, m.info.work.y, m.info.work.w, m.bar_px));
            }
        }
    }

    /// Manages windows that were open before lattice started: they join the active
    /// workspace on the monitor they're on.
    fn adopt_existing(&mut self) {
        let mut all = win::enum_windows();
        all.reverse(); // bottom of the z-order first, so the topmost window ends up focused
        for hwnd in all {
            if !self.windows.contains_key(&hwnd) {
                let mon = self.monitor_index_of_handle(win::monitor_of(hwnd));
                self.try_manage(hwnd, Some(mon), false);
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // Geometry helpers

    fn monitor_index_of_handle(&self, handle: isize) -> usize {
        self.monitors.iter().position(|m| m.info.handle == handle).unwrap_or(0)
    }

    fn monitor_index_at(&self, p: (i32, i32)) -> usize {
        self.monitor_index_of_handle(win::monitor_at(p))
    }

    fn tiling_area(&self, mon: usize) -> Rect {
        let m = &self.monitors[mon];
        let mut r = m.info.work;
        r.y += m.bar_px;
        r.h -= m.bar_px;
        r
    }

    /// The managed window that has keyboard focus, if any.
    fn focused_window(&self) -> Option<Hwnd> {
        let fg = win::foreground();
        self.windows.contains_key(&fg).then_some(fg)
    }

    /// A rule's 1-based workspace number as an index, clamped to the workspaces there are.
    fn ws_index(&self, n: usize) -> usize {
        n.saturating_sub(1).min(self.workspaces.len() - 1)
    }

    fn resolve(&self, t: WsTarget) -> usize {
        let n = self.workspaces.len();
        match t {
            WsTarget::Index(i) => i.min(n - 1),
            WsTarget::Next => (self.active + 1) % n,
            WsTarget::Prev => (self.active + n - 1) % n,
        }
    }

    /// Windows of workspace `ws` on monitor `mon`, in tiling order.
    fn on_monitor(&self, ws: usize, mon: usize) -> Vec<Hwnd> {
        self.workspaces[ws].windows.iter().copied().filter(|h| self.windows.get(h).is_some_and(|w| w.mon == mon)).collect()
    }

    // -----------------------------------------------------------------------------------------
    // Managing windows

    /// `mon`: the monitor to put the window on (startup); `None` means the focused monitor.
    fn try_manage(&mut self, hwnd: Hwnd, mon: Option<usize>, runtime: bool) -> bool {
        if self.windows.contains_key(&hwnd) {
            return false;
        }
        let Some(c) = win::candidate(hwnd) else { return false };
        let mut floating = self.cfg.general.float_unresizable && !c.resizable;
        let mut repaint = false;
        for r in self.cfg.rules.iter().filter(|r| r.matches(&c.exe, &c.class, &c.title)) {
            if r.ignore {
                return false;
            }
            floating |= r.float;
            repaint |= r.repaint;
        }
        // A window of an app a saved workspace is starting takes its spot there (not a splash
        // screen: those float, and close again).
        let claim = if runtime && !floating { self.claim_slot(hwnd, &c.exe) } else { None };
        // Every window of a pinned app goes there, splash screens and updaters included (they
        // open hidden on that workspace); you stay where you are.
        let ws = claim
            .or_else(|| config::rule_workspace(&self.cfg.rules, &c.exe, &c.class, &c.title).map(|n| self.ws_index(n)))
            .unwrap_or(self.active);
        let mon = if claim.is_some() { self.primary_monitor() } else { mon.unwrap_or(self.focused_monitor) };
        let minimized = win::is_minimized(hwnd);
        log::info!(
            "manage {hwnd:#x} '{}' ({}, {}) -> ws {} monitor {}{}",
            c.title, c.exe, c.class, ws + 1, mon, if floating { " floating" } else { "" }
        );
        let name = app_name(&c.exe, &c.title).into();
        let icon = crate::icon::for_window(hwnd);
        self.windows.insert(hwnd, Window { ws, mon, floating, minimized, repaint, name, icon, exe: c.exe, class: c.class, title: c.title, focused_at: 0 });

        let w = &mut self.workspaces[ws];
        let pos = match (self.cfg.general.new_window, w.last_focus) {
            (NewWindow::AfterFocused, Some(f)) if runtime => {
                w.windows.iter().position(|x| *x == f).map(|i| i + 1).unwrap_or(w.windows.len())
            }
            _ => w.windows.len(),
        };
        w.windows.insert(pos, hwnd);
        if ws != self.active {
            self.hide_window(hwnd);
        } else if runtime {
            self.note_focus(hwnd);
            if floating {
                self.center_floating(hwnd);
            }
        }
        if claim.is_some() {
            log::info!("{hwnd:#x} ({}) takes its place in saved workspace '{}'", self.windows[&hwnd].exe, self.restore.as_ref().map_or("", |r| r.name.as_str()));
            self.arrange_restored();
        }
        self.retile(ws);
        self.publish();
        true
    }

    fn unmanage(&mut self, hwnd: Hwnd) {
        let Some(w) = self.windows.remove(&hwnd) else { return };
        log::info!("unmanage {hwnd:#x} '{}'", w.title);
        self.anim.cancel(hwnd, false);
        self.pending_hide.remove(&hwnd);
        self.pending_show.remove(&hwnd);
        self.hidden.remove(&hwnd);
        shared::mark_hidden(hwnd, false);
        // A window that filled a saved workspace's spot (a splash screen, the first window of
        // an app that opens another) gives it up while the restore still waits.
        if let Some(r) = self.restore.as_mut() {
            for f in r.filled.iter_mut().filter(|f| **f == Some(hwnd)) {
                *f = None;
            }
        }
        let ws = &mut self.workspaces[w.ws];
        ws.windows.retain(|x| *x != hwnd);
        if ws.fullscreen == Some(hwnd) {
            ws.fullscreen = None;
        }
        ws.mon_focus.retain(|_, h| *h != hwnd);
        let was_focus = ws.last_focus == Some(hwnd);
        if was_focus {
            ws.last_focus = None;
        }
        self.retile(w.ws);
        // Hand focus to a window on the same screen (Hyprland-like), not whatever Windows
        // picks from the z-order.
        if was_focus && w.ws == self.active {
            self.focus_monitor_window(w.mon);
        }
        self.publish();
    }

    /// Hides a window. Show/hide requests are asynchronous, so whether a window *should* be
    /// hidden is tracked here, never read back from Windows mid-transition: requests reach
    /// the app in order, so the last one wins.
    fn hide_window(&mut self, hwnd: Hwnd) {
        if !self.hidden.contains_key(&hwnd) {
            // Mid-animation it's transparent: it stays so until the hide lands, so it never
            // flashes up where it is.
            self.anim.cancel(hwnd, true);
            self.hidden.insert(hwnd, Instant::now());
            if win::is_visible(hwnd) {
                *self.pending_hide.entry(hwnd).or_default() += 1;
            }
            win::hide(hwnd);
        }
        shared::mark_hidden(hwnd, true);
    }

    fn show_window(&mut self, hwnd: Hwnd) {
        // A window still sliding away was never really hidden: calling off its hide is enough.
        if self.hidden.remove(&hwnd).is_some() && !self.anim.take_hide(hwnd) {
            *self.pending_show.entry(hwnd).or_default() += 1;
            win::show(hwnd);
            // Back before a hide that was waiting to make it opaque landed.
            self.anim.unfade_now(hwnd);
        }
        shared::mark_hidden(hwnd, false);
    }

    /// `hide_window` after sliding the window's preview `dy` pixels (off its screen). It counts
    /// as hidden from now on; the real hide happens when the slide ends (`after_anim`). The
    /// window itself never moves. One that can't be previewed is hidden at once.
    fn slide_out(&mut self, hwnd: Hwnd, style: AnimStyle, dy: i32, clip: Option<Rect>) {
        if self.hidden.contains_key(&hwnd) {
            return;
        }
        let Some(from) = self.rect_of_now(hwnd).filter(|_| win::is_visible(hwnd) && !win::is_minimized(hwnd)) else {
            self.hide_window(hwnd);
            return;
        };
        let t = anim::timing(style, Kind::Slide);
        if !self.anim.glide(hwnd, Glide::sliding(from, from.offset(0, dy), t, clip).hiding()) {
            self.hide_window(hwnd);
            return;
        }
        self.hidden.insert(hwnd, Instant::now());
        shared::mark_hidden(hwnd, true);
    }

    /// Does what a finished animation left to do.
    fn after_anim(&mut self, hwnd: Hwnd, after: After) {
        if after != After::Hide {
            return;
        }
        // Shown again since (its workspace came back): nothing to do.
        if !self.hidden.contains_key(&hwnd) || !self.windows.contains_key(&hwnd) {
            self.anim.unfade_now(hwnd);
            return;
        }
        if win::is_visible(hwnd) {
            *self.pending_hide.entry(hwnd).or_default() += 1;
        }
        win::hide(hwnd);
    }

    /// Moves animating windows one frame on. The WM loop calls this on every wakeup; the frame
    /// clock wakes it once a frame while anything moves.
    pub fn animate(&mut self) {
        if !self.anim.busy() {
            return;
        }
        let done = self.anim.tick(Instant::now());
        if !done.is_empty() {
            // The focus border sits out animations; bring it back once its window lands.
            self.border_dirty = true;
        }
        for (h, after) in done {
            self.after_anim(h, after);
        }
    }

    /// Stops a window's animation where it is, still doing what it was going to do at the end
    /// (a window sliding away still gets hidden).
    fn stop_anim(&mut self, hwnd: Hwnd) {
        let hiding = self.anim.is_hiding(hwnd);
        if let Some(after) = self.anim.cancel(hwnd, hiding) {
            self.after_anim(hwnd, after);
        }
    }

    /// Lands every animation at once.
    fn finish_animations(&mut self) {
        for (h, after) in self.anim.finish_all() {
            self.after_anim(h, after);
        }
        self.border_dirty = true;
    }

    /// Where a window is, or where it's headed if it's animating: what layout decisions (focus,
    /// neighbours, drops) should go by.
    fn rect_of(&self, hwnd: Hwnd) -> Option<Rect> {
        self.anim.target(hwnd).or_else(|| win::frame_rect(hwnd))
    }

    /// Puts a window at `r`, in one move. With animations on, its preview glides there from
    /// where it appears to be while the window itself, invisible, waits to be moved when the
    /// preview lands.
    fn place_tile(&mut self, hwnd: Hwnd, r: Rect) {
        if self.anim.target(hwnd) == Some(r) {
            return;
        }
        let cur = self.rect_of_now(hwnd);
        let maximized = win::is_maximized(hwnd);
        if let (Some(style), Some(c)) = (self.cfg.animations.active(), cur)
            && c != r
            && !maximized
            && !self.hidden.contains_key(&hwnd)
            && win::is_visible(hwnd)
            && self.anim.glide(hwnd, Glide::moving(c, r, anim::timing(style, Kind::Move)))
        {
            return;
        }
        self.stop_anim(hwnd);
        if win::frame_rect(hwnd) != Some(r) || maximized {
            win::place(hwnd, r);
        }
    }

    /// True once a window lattice hid has been hidden long enough that events about it are
    /// the app's doing, not late echoes of our own hide or an earlier focus change.
    fn settled(&self, hwnd: Hwnd) -> bool {
        self.hidden.get(&hwnd).is_none_or(|t| t.elapsed() > Duration::from_millis(600))
    }

    fn center_floating(&mut self, hwnd: Hwnd) {
        let area = self.tiling_area(self.windows[&hwnd].mon);
        let cur = self.rect_of(hwnd).unwrap_or(area);
        // Keep the window's own size if it fits, else 60% of the area.
        let (w, h) = if cur.w < area.w && cur.h < area.h { (cur.w, cur.h) } else { (area.w * 3 / 5, area.h * 3 / 5) };
        self.place_tile(hwnd, Rect::new(area.x + (area.w - w) / 2, area.y + (area.h - h) / 2, w, h));
        win::raise(hwnd);
    }

    // -----------------------------------------------------------------------------------------
    // Tiling

    /// Lays out workspace `ws` on every monitor (only if it's on screen).
    fn retile(&mut self, ws_idx: usize) {
        if ws_idx != self.active {
            return;
        }
        for mon in 0..self.monitors.len() {
            let n = self.retile_monitor(ws_idx, mon);
            // Split ratios belong to positions: forget those past the last split.
            if let Some(v) = self.workspaces[ws_idx].splits.get_mut(&mon) {
                v.truncate(n.saturating_sub(1));
            }
        }
    }

    /// Tiled (not floating or minimized) windows of workspace `ws` on monitor `mon`, in order.
    fn tiled(&self, ws: usize, mon: usize) -> Vec<Hwnd> {
        self.on_monitor(ws, mon).into_iter().filter(|h| self.windows.get(h).is_some_and(|w| !w.floating && !w.minimized)).collect()
    }

    fn params(&self, ws_idx: usize, mon: usize) -> Params<'_> {
        let ws = &self.workspaces[ws_idx];
        Params {
            gaps_inner: self.cfg.general.gaps_inner,
            gaps_outer: self.cfg.general.gaps_outer,
            ratio: if ws.layout == LayoutKind::Master { ws.master_ratio } else { ws.split_ratio },
            splits: ws.splits.get(&mon).map_or(&[], |v| v),
        }
    }

    /// Lays out one monitor; returns how many windows it tiles.
    fn retile_monitor(&mut self, ws_idx: usize, mon: usize) -> usize {
        let (placements, top, n) = self.layout_monitor(ws_idx, mon);
        for (hwnd, r) in placements {
            self.place_tile(hwnd, r);
        }
        if let Some(t) = top {
            win::raise(t);
        }
        n
    }

    /// Where the tiled windows of workspace `ws` on `mon` go, the one to keep on top
    /// (fullscreen or monocle), and how many windows it tiles.
    fn layout_monitor(&self, ws_idx: usize, mon: usize) -> (Vec<(Hwnd, Rect)>, Option<Hwnd>, usize) {
        let ws = &self.workspaces[ws_idx];
        let area = self.tiling_area(mon);
        let tiled = self.tiled(ws_idx, mon);
        let n = tiled.len();
        if let Some(f) = ws.fullscreen.filter(|f| tiled.contains(f)) {
            return (vec![(f, area)], Some(f), n);
        }
        let rects = layout::tile(ws.layout, area, n, &self.params(ws_idx, mon));
        let top = (ws.layout == LayoutKind::Monocle).then(|| ws.mon_focus.get(&mon).copied()).flatten().filter(|f| tiled.contains(f));
        (tiled.into_iter().zip(rects).collect(), top, n)
    }

    /// Expands the focused tiled window toward `dir` by moving the split boundary on that side
    /// (or, at the screen edge, the opposite one: see `layout::resize_target`).
    fn resize_dir(&mut self, dir: Dir, amount: f32) {
        let Some(h) = self.focused_window() else { return };
        let (ws_idx, mon) = (self.windows[&h].ws, self.windows[&h].mon);
        if ws_idx != self.active || self.workspaces[ws_idx].fullscreen.is_some() {
            return;
        }
        let tiled = self.tiled(ws_idx, mon);
        let Some(k) = tiled.iter().position(|x| *x == h) else { return };
        let target = layout::resize_target(self.workspaces[ws_idx].layout, self.tiling_area(mon), tiled.len(), k, &self.params(ws_idx, mon), dir);
        let ws = &mut self.workspaces[ws_idx];
        match target {
            Some(layout::ResizeTarget::Master) => ws.master_ratio = layout::nudge(ws.master_ratio, dir, amount),
            Some(layout::ResizeTarget::Split(i)) => {
                let v = ws.splits.entry(mon).or_default();
                if v.len() <= i {
                    v.resize(i + 1, ws.split_ratio);
                }
                v[i] = layout::nudge(v[i], dir, amount);
            }
            None => return,
        }
        self.retile(ws_idx);
    }

    // -----------------------------------------------------------------------------------------
    // Workspaces and focus

    /// Makes `ws` the workspace on screen, on all monitors.
    fn show_workspace(&mut self, ws: usize) {
        let old = self.active;
        if old == ws {
            return;
        }
        self.active = ws;
        let Some(style) = self.cfg.animations.active() else {
            // Show the incoming windows first, then hide the outgoing ones: no flash of desktop.
            for hwnd in self.workspaces[ws].windows.clone() {
                self.show_window(hwnd);
            }
            self.retile(ws);
            for hwnd in self.workspaces[old].windows.clone() {
                self.hide_window(hwnd);
            }
            return;
        };
        // Animated: a higher workspace comes up from below and pushes the old one off the top
        // (a lower one, the other way). Vertical, so side-by-side monitors never see each
        // other's windows pass through.
        let down = if ws > old { 1 } else { -1 };
        let mut targets: HashMap<Hwnd, (Rect, usize)> = HashMap::new();
        for mon in 0..self.monitors.len() {
            for (i, (h, r)) in self.layout_monitor(ws, mon).0.into_iter().enumerate() {
                targets.insert(h, (r, i));
            }
        }
        let slide = anim::timing(style, Kind::Slide);
        for hwnd in self.workspaces[ws].windows.clone() {
            let w = &self.windows[&hwnd];
            let height = self.monitors.get(w.mon).map_or(0, |m| m.info.rect.h);
            if w.minimized {
                self.show_window(hwnd);
                continue;
            }
            // Tiled windows go to their tile; floating ones back where they are (a window
            // sliding away never really moved).
            let (target, order) = match targets.get(&hwnd) {
                Some(t) => *t,
                None => match win::frame_rect(hwnd) {
                    Some(r) => (r, 0),
                    None => {
                        self.show_window(hwnd);
                        continue;
                    }
                },
            };
            // Still on screen (sliding away, or carried along by move-to-workspace): its preview
            // turns around from where it is. Otherwise the preview starts a screen below its
            // spot, after a moment for the window to draw once it's shown (a window just shown
            // has a blank preview for a frame or two).
            let on_screen = !self.hidden.contains_key(&hwnd) || self.anim.is_hiding(hwnd);
            let (from, delay) = if on_screen {
                (self.rect_of_now(hwnd).unwrap_or(target), Duration::ZERO)
            } else {
                (target.offset(0, down * height), anim::WARMUP + slide.stagger * order.min(4) as u32)
            };
            let clip = Some(self.tiling_area(w.mon));
            // Before it's shown, so it never appears in place first. `show_window` calls off a
            // pending hide.
            if !self.anim.glide(hwnd, Glide::sliding(from, target, slide, clip).delay(delay)) {
                self.stop_anim(hwnd);
            }
            if win::frame_rect(hwnd) != Some(target) {
                win::place(hwnd, target);
            }
            self.show_window(hwnd);
        }
        self.retile(ws);
        for hwnd in self.workspaces[old].windows.clone() {
            let mon = self.windows.get(&hwnd).map(|w| w.mon).filter(|m| *m < self.monitors.len());
            let height = mon.map_or(0, |m| self.monitors[m].info.rect.h);
            self.slide_out(hwnd, style, -down * height, mon.map(|m| self.tiling_area(m)));
        }
    }

    /// Where a window is on screen this moment (mid-animation included).
    fn rect_of_now(&self, hwnd: Hwnd) -> Option<Rect> {
        self.anim.current(hwnd).or_else(|| win::frame_rect(hwnd))
    }

    /// Records that `hwnd` has focus (workspace memory, focused monitor).
    fn note_focus(&mut self, hwnd: Hwnd) {
        self.focus_seq += 1;
        let Some(w) = self.windows.get_mut(&hwnd) else { return };
        w.focused_at = self.focus_seq;
        let (ws, mon) = (w.ws, w.mon);
        self.workspaces[ws].last_focus = Some(hwnd);
        self.workspaces[ws].mon_focus.insert(mon, hwnd);
        if ws == self.active {
            self.focused_monitor = mon;
        }
    }

    fn focus(&mut self, hwnd: Hwnd) {
        win::focus(hwnd);
        self.note_focus(hwnd);
        if let Some(w) = self.windows.get(&hwnd)
            && (self.workspaces[w.ws].layout == LayoutKind::Monocle || w.floating)
        {
            win::raise(hwnd);
        }
        if self.cfg.general.mouse_follows_focus
            && let Some(r) = self.rect_of(hwnd)
            && !r.contains(win::cursor_pos())
        {
            let (x, y) = r.center();
            win::set_cursor(x, y);
        }
    }

    /// Focuses the active workspace's window on `mon` (the last one used there), or the
    /// desktop if that screen is empty.
    fn focus_monitor_window(&mut self, mon: usize) {
        let ws = self.active;
        let here = self.on_monitor(ws, mon);
        let target = self.workspaces[ws]
            .mon_focus
            .get(&mon)
            .copied()
            .filter(|h| here.contains(h))
            .or_else(|| here.iter().copied().find(|h| !self.windows[h].minimized));
        self.focused_monitor = mon;
        match target {
            Some(h) => self.focus(h),
            None => {
                win::focus_desktop();
                if self.cfg.general.mouse_follows_focus {
                    let (x, y) = self.tiling_area(mon).center();
                    win::set_cursor(x, y);
                }
            }
        }
    }

    /// Focuses the window you last used on the active workspace, else something on the
    /// focused monitor.
    fn focus_active_workspace(&mut self) {
        let ws = &self.workspaces[self.active];
        let target = ws.fullscreen.or(ws.last_focus).filter(|h| ws.windows.contains(h));
        match target {
            Some(h) => self.focus(h),
            None => self.focus_monitor_window(self.focused_monitor),
        }
    }

    fn switch_to_workspace(&mut self, ws: usize) {
        if ws != self.active {
            self.show_workspace(ws);
            self.focus_active_workspace();
        }
        self.publish();
    }

    /// Moves a managed window to workspace `to` (same monitor), appending it to the tiling
    /// order. With `follow`, the caller is about to show `to`, so the window isn't hidden.
    fn move_window_to_ws(&mut self, hwnd: Hwnd, to: usize, follow: bool) {
        let Some(from) = self.windows.get(&hwnd).map(|w| w.ws) else { return };
        if from == to {
            return;
        }
        let f = &mut self.workspaces[from];
        f.windows.retain(|x| *x != hwnd);
        if f.fullscreen == Some(hwnd) {
            f.fullscreen = None;
        }
        f.mon_focus.retain(|_, h| *h != hwnd);
        if f.last_focus == Some(hwnd) {
            f.last_focus = None;
        }
        self.workspaces[to].windows.push(hwnd);
        self.windows.get_mut(&hwnd).unwrap().ws = to;
        self.note_focus(hwnd);
        if to == self.active || follow {
            self.show_window(hwnd);
        } else if let Some(style) = self.cfg.animations.active()
            && from == self.active
        {
            // Off it goes, the way that workspace lies.
            let mon = self.windows[&hwnd].mon.min(self.monitors.len().saturating_sub(1));
            let height = self.monitors.get(mon).map_or(0, |m| m.info.rect.h);
            let clip = (mon < self.monitors.len()).then(|| self.tiling_area(mon));
            self.slide_out(hwnd, style, if to > from { height } else { -height }, clip);
        } else {
            self.hide_window(hwnd);
        }
        self.retile(from);
        self.retile(to);
    }

    /// Moves a window to another monitor within its workspace.
    fn move_window_to_monitor(&mut self, hwnd: Hwnd, mon: usize) {
        let Some(w) = self.windows.get_mut(&hwnd) else { return };
        if w.mon == mon {
            return;
        }
        w.mon = mon;
        let (ws, floating) = (w.ws, w.floating);
        self.workspaces[ws].mon_focus.retain(|_, h| *h != hwnd);
        if self.workspaces[ws].fullscreen == Some(hwnd) {
            self.workspaces[ws].fullscreen = None;
        }
        if floating {
            self.center_floating(hwnd);
        }
        self.retile(ws);
    }

    // -----------------------------------------------------------------------------------------
    // Directional navigation

    /// Finds the nearest window (or empty monitor) of the active workspace from `from` in
    /// direction `dir`. `only_mon` restricts the search to tiled windows on one monitor.
    fn neighbor(&self, from: Rect, exclude: Option<Hwnd>, dir: Dir, only_mon: Option<usize>) -> Option<Target> {
        let ws_idx = self.active;
        let ws = &self.workspaces[ws_idx];
        let mut candidates: Vec<(Target, Rect)> = Vec::new();
        for mon in 0..self.monitors.len() {
            if only_mon.is_some_and(|m| m != mon) {
                continue;
            }
            let here = self.on_monitor(ws_idx, mon);
            // Only the top window counts on a fullscreen or monocle screen.
            let top = ws.fullscreen.filter(|f| here.contains(f)).or_else(|| {
                (ws.layout == LayoutKind::Monocle).then(|| ws.mon_focus.get(&mon).copied()).flatten().filter(|f| here.contains(f))
            });
            let visible = match top {
                Some(t) if only_mon.is_none() => vec![t],
                _ => here,
            };
            let mut any = false;
            for h in visible {
                let w = &self.windows[&h];
                if w.minimized || Some(h) == exclude || (only_mon.is_some() && w.floating) {
                    continue;
                }
                if let Some(r) = self.rect_of(h) {
                    candidates.push((Target::Window(h), r));
                    any = true;
                }
            }
            if !any && only_mon.is_none() {
                candidates.push((Target::Monitor(mon), self.tiling_area(mon)));
            }
        }
        let (cx, cy) = from.center();
        candidates
            .into_iter()
            .filter_map(|(t, r)| {
                let (x, y) = r.center();
                let (ahead, gap, overlap, offset) = match dir {
                    Dir::Left => (x < cx, from.x - r.right(), overlap(from.y, from.bottom(), r.y, r.bottom()), (y - cy).abs()),
                    Dir::Right => (x > cx, r.x - from.right(), overlap(from.y, from.bottom(), r.y, r.bottom()), (y - cy).abs()),
                    Dir::Up => (y < cy, from.y - r.bottom(), overlap(from.x, from.right(), r.x, r.right()), (x - cx).abs()),
                    Dir::Down => (y > cy, r.y - from.bottom(), overlap(from.x, from.right(), r.x, r.right()), (x - cx).abs()),
                };
                if !ahead {
                    return None;
                }
                // Prefer windows that line up with us, then the closest edge, then centres.
                let score = gap.max(0) as i64 * 4 + offset as i64 + if overlap > 0 { 0 } else { 1_000_000 };
                Some((score, t))
            })
            .min_by_key(|(s, _)| *s)
            .map(|(_, t)| t)
    }

    /// The monitor next to `mon` in direction `dir`.
    fn monitor_toward(&self, mon: usize, dir: Dir) -> Option<usize> {
        let (cx, cy) = self.monitors[mon].info.rect.center();
        self.monitors
            .iter()
            .enumerate()
            .filter(|(i, m)| {
                let (x, y) = m.info.rect.center();
                *i != mon
                    && match dir {
                        Dir::Left => x < cx,
                        Dir::Right => x > cx,
                        Dir::Up => y < cy,
                        Dir::Down => y > cy,
                    }
            })
            .min_by_key(|(_, m)| {
                let (x, y) = m.info.rect.center();
                (x - cx).abs() + (y - cy).abs()
            })
            .map(|(i, _)| i)
    }

    fn focus_dir(&mut self, dir: Dir) {
        let cur = self.focused_window().filter(|h| self.windows[h].ws == self.active);
        let ws = self.active;
        // Monocle: left/up and right/down cycle through the stack on this monitor.
        if let Some(h) = cur
            && self.workspaces[ws].layout == LayoutKind::Monocle
            && self.workspaces[ws].fullscreen.is_none()
            && !self.windows[&h].floating
        {
            let mon = self.windows[&h].mon;
            let list: Vec<Hwnd> = self.on_monitor(ws, mon).into_iter().filter(|x| !self.windows[x].minimized && !self.windows[x].floating).collect();
            if let Some(i) = list.iter().position(|x| *x == h) {
                let n = list.len();
                let next = match dir {
                    Dir::Right | Dir::Down => i + 1,
                    Dir::Left | Dir::Up => i.wrapping_sub(1),
                };
                if next < n {
                    self.focus(list[next]);
                    self.publish();
                    return;
                }
                // Past the end of the stack: carry on to the next monitor.
            }
        }
        let from = cur.and_then(|h| self.rect_of(h)).unwrap_or_else(|| self.tiling_area(self.focused_monitor));
        match self.neighbor(from, cur, dir, None) {
            Some(Target::Window(h)) => self.focus(h),
            Some(Target::Monitor(m)) => self.focus_monitor_window(m),
            None => return,
        }
        self.publish();
    }

    fn move_dir(&mut self, dir: Dir) {
        let Some(h) = self.focused_window() else { return };
        let (ws, mon, floating) = {
            let w = &self.windows[&h];
            (w.ws, w.mon, w.floating)
        };
        if ws != self.active {
            return;
        }
        if !floating
            && let Some(from) = self.rect_of(h)
            && let Some(Target::Window(other)) = self.neighbor(from, Some(h), dir, Some(mon))
        {
            let list = &mut self.workspaces[ws].windows;
            if let (Some(a), Some(b)) = (list.iter().position(|x| *x == h), list.iter().position(|x| *x == other)) {
                list.swap(a, b);
            }
            self.retile(ws);
            // The window you moved crosses over the one it swaps with, not under it.
            self.anim.raise(h);
            self.focus(h);
            return;
        }
        // Nothing to swap with on this screen: move to the monitor in that direction.
        if let Some(t) = self.monitor_toward(mon, dir) {
            self.move_window_to_monitor(h, t);
            self.anim.raise(h);
            self.focus(h);
            self.publish();
        }
    }

    // -----------------------------------------------------------------------------------------
    // Commands

    pub fn command(&mut self, cmd: Command) {
        log::debug!("command {cmd:?}");
        self.border_dirty = true;
        self.events_dirty = true;
        match cmd {
            Command::Focus(d) => self.focus_dir(d),
            Command::MoveWindow(d) => self.move_dir(d),
            Command::Workspace(t) => {
                let ws = self.resolve(t);
                self.switch_to_workspace(ws);
            }
            Command::MoveToWorkspace { target, follow } => {
                let to = self.resolve(target);
                let Some(h) = self.focused_window() else { return };
                let mon = self.windows[&h].mon;
                self.move_window_to_ws(h, to, follow);
                if follow {
                    self.switch_to_workspace(to);
                    self.focus(h);
                } else {
                    self.focus_monitor_window(mon);
                }
                self.publish();
            }
            Command::FocusMonitor(step) => {
                let n = self.monitors.len() as i32;
                let m = (self.focused_monitor as i32 + step).rem_euclid(n) as usize;
                self.focus_monitor_window(m);
                self.publish();
            }
            Command::MoveToMonitor(step) => {
                let Some(h) = self.focused_window() else { return };
                let n = self.monitors.len() as i32;
                let m = (self.windows[&h].mon as i32 + step).rem_euclid(n) as usize;
                self.move_window_to_monitor(h, m);
                self.focus(h);
                self.publish();
            }
            Command::Layout(kind) => {
                let w = &mut self.workspaces[self.active];
                w.layout = kind.unwrap_or(w.layout.next());
                self.retile(self.active);
                self.publish();
            }
            Command::ResizeDir(dir, amount) => self.resize_dir(dir, amount),
            Command::ToggleFloat => {
                let Some(h) = self.focused_window() else { return };
                let w = self.windows.get_mut(&h).unwrap();
                w.floating = !w.floating;
                let (ws, floating) = (w.ws, w.floating);
                if floating {
                    self.center_floating(h);
                }
                self.retile(ws);
            }
            Command::ToggleFullscreen => {
                let Some(h) = self.focused_window() else { return };
                let ws = self.windows[&h].ws;
                let w = &mut self.workspaces[ws];
                w.fullscreen = if w.fullscreen == Some(h) { None } else { Some(h) };
                if self.workspaces[ws].fullscreen.is_some() {
                    self.windows.get_mut(&h).unwrap().floating = false;
                }
                self.retile(ws);
            }
            Command::Close => {
                let fg = win::foreground();
                if self.windows.contains_key(&fg) || win::candidate(fg).is_some() {
                    win::close(fg);
                }
            }
            Command::Retile => {
                self.cleanup();
                self.refresh_monitors();
                self.adopt_existing();
                self.retile(self.active);
                self.publish();
            }
            Command::Reload => {
                let cfg = config::load();
                for ws in &mut self.workspaces {
                    ws.layout = cfg.general.layout;
                    ws.split_ratio = cfg.general.split_ratio;
                    ws.master_ratio = cfg.general.master_ratio;
                    ws.splits.clear();
                }
                let old_rules = std::mem::replace(&mut self.cfg, cfg).rules;
                // The border is ours (not the bar's), so its settings can apply right away.
                self.border = None;
                self.border = crate::border::Border::new(self.cfg.border.clone());
                self.build_workspaces();
                if self.cfg.animations.active().is_none() {
                    // Turned off: land whatever is mid-flight right away.
                    self.finish_animations();
                }
                self.reload_keymap = true;
                events::set_hooks(&self.cfg.hooks);
                self.pending_events.push(events::Event::new(events::Kind::ConfigReloaded));
                self.refresh_monitors();
                self.apply_changed_assignments(&old_rules);
                self.retile(self.active);
                self.publish();
                self.publish_setups();
                log::info!("config reloaded");
            }
            Command::Exec(line) => spawn(&line),
            Command::Launcher => crate::launcher::toggle(self.launcher_area()),
            Command::WindowSearch => crate::launcher::toggle_windows(self.launcher_area(), self.window_list()),
            Command::ListWindows => crate::launcher::set_windows(self.window_list()),
            Command::FocusWindow(h) => {
                let Some(ws) = self.windows.get(&h).map(|w| w.ws) else { return };
                if ws != self.active {
                    // Arrive on it, not on whatever was last used there.
                    self.workspaces[ws].last_focus = Some(h);
                    self.show_workspace(ws);
                }
                // `focus` restores a minimized window and raises a floating one.
                self.focus(h);
                self.publish();
            }
            Command::Settings => open_settings(),
            Command::Tour => crate::tour::start(),
            Command::RestoreSetup(name) => self.restore_setup(&name),
            Command::SetupPicker => crate::launcher::toggle_setups(self.launcher_area()),
            Command::OpenConfig => open_config(),
            Command::Quit => self.quit = true,
        }
    }

    /// The focused monitor's work area, to open the launcher on.
    fn launcher_area(&self) -> Option<crate::launcher::Area> {
        self.monitors.get(self.focused_monitor).map(|m| crate::launcher::Area {
            x: m.info.work.x,
            y: m.info.work.y,
            w: m.info.work.w,
            h: m.info.work.h,
            scale: m.info.scale,
        })
    }

    /// Every managed window for window search, most recently focused first (never-focused ones
    /// after, by workspace and tiling order).
    fn window_list(&self) -> Vec<crate::launcher::WindowEntry> {
        // The launcher may have focus already (switching it to window search): then the window
        // you were in is the active workspace's last one.
        let current = self.focused_window().or(self.workspaces[self.active].last_focus);
        let mut list: Vec<(u64, crate::launcher::WindowEntry)> = self
            .workspaces
            .iter()
            .flat_map(|ws| ws.windows.iter())
            .filter_map(|h| {
                let w = self.windows.get(h)?;
                Some((w.focused_at, crate::launcher::WindowEntry {
                    hwnd: *h,
                    title: w.title.clone(),
                    app: search_name(&w.exe, &w.name),
                    workspace: w.ws + 1,
                    monitor: w.mon + 1,
                    floating: w.floating,
                    minimized: w.minimized,
                    current: current == Some(*h),
                    icon: w.icon.as_ref().map(|i| crate::launcher::WindowIcon {
                        key: i.key,
                        size: crate::icon::SIZE,
                        rgba: i.rgba.clone(),
                    }),
                }))
            })
            .collect();
        list.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
        list.into_iter().map(|(_, e)| e).collect()
    }

    /// After a reload, moves open windows whose app->workspace rule changed (added, or pointed
    /// elsewhere) to their new workspace. Windows whose rule didn't change stay put, so a window
    /// you moved by hand isn't yanked back on every reload.
    fn apply_changed_assignments(&mut self, old_rules: &[config::Rule]) {
        let rules = &self.cfg.rules;
        let moves: Vec<(Hwnd, usize)> = self
            .windows
            .iter()
            .filter_map(|(h, w)| {
                let new = config::rule_workspace(rules, &w.exe, &w.class, &w.title)?;
                (config::rule_workspace(old_rules, &w.exe, &w.class, &w.title) != Some(new)).then_some((*h, new))
            })
            .collect();
        let focused = self.focused_window();
        let mut lost_focus = false;
        for (h, n) in moves {
            let to = self.ws_index(n);
            if self.windows[&h].ws != to {
                log::info!("{h:#x} '{}' -> ws {} (its rule changed)", self.windows[&h].title, to + 1);
                lost_focus |= focused == Some(h) && to != self.active;
                self.move_window_to_ws(h, to, false);
            }
        }
        if lost_focus {
            self.focus_monitor_window(self.focused_monitor);
        }
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    // -----------------------------------------------------------------------------------------
    // Saved workspaces (`setups.rs`)

    /// The primary monitor's index: saved workspaces tile there.
    fn primary_monitor(&self) -> usize {
        self.monitors.iter().position(|m| m.info.primary).unwrap_or(0)
    }

    /// Restores the saved workspace `name`: open windows of its apps move into place, the others
    /// are started (their windows take their spots as they open, see `claim_slot`), each of its
    /// workspaces gets its layout, and the first one comes on screen.
    fn restore_setup(&mut self, name: &str) {
        let Some(setup) = config::find_setup(&self.cfg.setups, name).cloned() else {
            log::warn!("restore-workspace: no saved workspace called '{name}'");
            return;
        };
        self.cleanup();
        // Most recently used first: of two open windows of an app, the one you were last in
        // is the one that moves.
        let mut open: Vec<(Hwnd, String, usize, u64)> =
            self.windows.iter().filter(|(_, w)| !w.floating).map(|(h, w)| (*h, w.exe.clone(), w.ws, w.focused_at)).collect();
        open.sort_by_key(|o| std::cmp::Reverse(o.3));
        let place = |h: Hwnd, ws: usize| self.workspaces[ws].windows.iter().position(|x| *x == h).unwrap_or(usize::MAX);
        let open_ref: Vec<setups::OpenWindow> = open.iter().map(|(h, exe, ws, _)| (*h, exe.as_str(), *ws, place(*h, *ws))).collect();
        let plan = setups::plan(&setup, &open_ref, |n| self.ws_index(n));
        log::info!(
            "restoring saved workspace '{}': {} in place, starting {}",
            setup.name, plan.claims.len(), plan.launch.len()
        );
        self.finish_animations();
        let primary = self.primary_monitor();
        let g = self.cfg.general.clone();
        let mut saved_splits = HashMap::new();
        for w in &setup.workspaces {
            let i = self.ws_index(w.number);
            let ws = &mut self.workspaces[i];
            ws.layout = w.layout.unwrap_or(g.layout);
            ws.master_ratio = w.master_ratio.unwrap_or(g.master_ratio).clamp(0.1, 0.9);
            ws.split_ratio = g.split_ratio;
            let splits: Vec<f32> = w.splits.iter().map(|r| r.clamp(0.1, 0.9)).collect();
            ws.splits.insert(primary, splits.clone());
            ws.fullscreen = None;
            saved_splits.insert(i, splits);
        }
        let mut filled = vec![None; plan.slots.len()];
        for &(i, h) in &plan.claims {
            filled[i] = Some(h);
            let to = plan.slots[i].ws;
            if self.windows[&h].ws != to {
                // Into place without sliding: the workspace switch below shows the result.
                let anim = self.cfg.animations.enabled;
                self.cfg.animations.enabled = false;
                self.move_window_to_ws(h, to, to == self.active);
                self.cfg.animations.enabled = anim;
            }
            let w = self.windows.get_mut(&h).expect("claimed window is managed");
            if w.mon != primary {
                w.mon = primary;
                self.workspaces[to].mon_focus.retain(|_, x| *x != h);
            }
        }
        for &i in &plan.launch {
            spawn(&plan.slots[i].run);
        }
        let first = plan.slots.first().map(|s| s.ws);
        let first_window = plan.slots.iter().zip(&filled).find(|(s, _)| Some(s.ws) == first).and_then(|(_, f)| *f);
        self.restore = Some(Restore {
            name: setup.name.clone(),
            slots: plan.slots,
            filled,
            splits: saved_splits,
            until: Instant::now() + RESTORE_WAIT,
        });
        self.arrange_restored();
        if let Some(ws) = first {
            if let Some(h) = first_window {
                self.workspaces[ws].last_focus = Some(h);
            }
            if ws != self.active {
                self.show_workspace(ws);
            } else {
                self.retile(ws);
            }
            self.focus_active_workspace();
        }
        self.retile(self.active);
        self.publish();
    }

    /// A spot of the saved workspace being restored for a new window of `exe`, which takes it:
    /// returns the spot's workspace.
    fn claim_slot(&mut self, hwnd: Hwnd, exe: &str) -> Option<usize> {
        let r = self.restore.as_mut()?;
        if Instant::now() > r.until {
            self.restore = None;
            return None;
        }
        let i = r.slots.iter().zip(&r.filled).position(|(s, f)| f.is_none() && s.exe.eq_ignore_ascii_case(exe))?;
        r.filled[i] = Some(hwnd);
        Some(r.slots[i].ws)
    }

    /// Puts the restored windows first on their workspaces, in the saved order.
    fn arrange_restored(&mut self) {
        let Some(r) = &self.restore else { return };
        let mut by_ws: HashMap<usize, Vec<(usize, Hwnd)>> = HashMap::new();
        for (s, f) in r.slots.iter().zip(&r.filled) {
            if let Some(h) = f.filter(|h| self.windows.get(h).is_some_and(|w| w.ws == s.ws)) {
                by_ws.entry(s.ws).or_default().push((s.order, h));
            }
        }
        let saved: Vec<(usize, Vec<f32>)> = by_ws.keys().filter_map(|ws| Some((*ws, r.splits.get(ws)?.clone()))).collect();
        let primary = self.primary_monitor();
        for (ws, splits) in saved {
            self.workspaces[ws].splits.insert(primary, splits);
        }
        for (ws, mut list) in by_ws {
            list.sort_by_key(|(order, _)| *order);
            let first: Vec<Hwnd> = list.into_iter().map(|(_, h)| h).collect();
            self.workspaces[ws].windows = setups::arrange(&self.workspaces[ws].windows, &first);
            self.retile(ws);
        }
    }

    /// Hands the launcher's picker the saved workspaces, with a thumbnail of each workspace.
    fn publish_setups(&self) {
        let d = setups::Defaults::from(&self.cfg.general);
        let area = if self.monitors.is_empty() { Rect::new(0, 0, 1920, 1080) } else { self.tiling_area(self.primary_monitor()) };
        let local = Rect::new(0, 0, area.w, area.h);
        let entries = self
            .cfg
            .setups
            .iter()
            .map(|s| crate::launcher::SetupEntry {
                name: s.name.clone(),
                key: s.key.clone().unwrap_or_default(),
                detail: setups::summary(s),
                workspaces: s
                    .workspaces
                    .iter()
                    .filter(|w| !w.apps.is_empty())
                    .map(|w| crate::launcher::SetupThumb {
                        number: w.number,
                        tiles: setups::tiles(w, local, &d)
                            .into_iter()
                            .map(|r| [r.x as f32 / area.w as f32, r.y as f32 / area.h as f32, r.w as f32 / area.w as f32, r.h as f32 / area.h as f32])
                            .collect(),
                    })
                    .collect(),
            })
            .collect();
        crate::launcher::set_setups(entries);
    }

    // -----------------------------------------------------------------------------------------
    // Scripting (`lattice query`, hooks, `lattice subscribe`)

    /// A snapshot for queries and event diffs. `rects` asks Windows where every window is,
    /// which only queries need.
    pub fn state(&self, rects: bool) -> ipc::State {
        let focused = self.focused_window();
        let mut windows = Vec::new();
        let mut workspaces = Vec::new();
        for (i, ws) in self.workspaces.iter().enumerate() {
            let mut mons: Vec<usize> = Vec::new();
            for h in &ws.windows {
                let Some(w) = self.windows.get(h) else { continue };
                if !mons.contains(&w.mon) {
                    mons.push(w.mon);
                }
                windows.push(ipc::WindowState {
                    hwnd: *h,
                    title: w.title.clone(),
                    exe: w.exe.clone(),
                    class: w.class.clone(),
                    workspace: i + 1,
                    monitor: w.mon,
                    floating: w.floating,
                    minimized: w.minimized,
                    fullscreen: ws.fullscreen == Some(*h),
                    focused: focused == Some(*h),
                    rect: if rects { self.rect_of(*h).map(Into::into) } else { None },
                });
            }
            mons.sort_unstable();
            workspaces.push(ipc::WorkspaceState {
                index: i + 1,
                active: i == self.active,
                layout: ws.layout.name(),
                windows: ws.windows.len(),
                monitors: mons,
                focused: ws.last_focus.filter(|h| ws.windows.contains(h)),
                fullscreen: ws.fullscreen,
            });
        }
        let monitors = self
            .monitors
            .iter()
            .enumerate()
            .map(|(i, m)| ipc::MonitorState {
                index: i,
                primary: m.info.primary,
                focused: i == self.focused_monitor,
                rect: m.info.rect.into(),
                work_area: m.info.work.into(),
                scale: m.info.scale,
            })
            .collect();
        ipc::State {
            workspaces,
            windows,
            monitors,
            active_workspace: self.active + 1,
            focused_monitor: self.focused_monitor,
            focused_window: focused,
        }
    }

    /// Takes the baseline for event diffs and announces `startup`.
    pub fn start_events(&mut self) {
        if events::listening() {
            self.events.update(self.state(false));
        }
        events::emit(events::Event::new(events::Kind::Startup).with("workspace", self.active + 1).with("version", env!("CARGO_PKG_VERSION")));
    }

    /// Sends what changed since the last batch to hooks and subscribers. The WM loop calls this
    /// once per batch; it does nothing unless a batch could have changed something and someone
    /// is listening.
    pub fn emit_events(&mut self) {
        for e in std::mem::take(&mut self.pending_events) {
            events::emit(e);
        }
        if !std::mem::take(&mut self.events_dirty) {
            return;
        }
        if !events::listening() {
            self.events.reset();
            return;
        }
        for e in self.events.update(self.state(false)) {
            events::emit(e);
        }
    }

    /// Forgets windows that died without us noticing.
    fn cleanup(&mut self) {
        let dead: Vec<Hwnd> = self.windows.keys().copied().filter(|h| !win::is_window(*h)).collect();
        for h in dead {
            self.unmanage(h);
        }
    }

    // -----------------------------------------------------------------------------------------
    // Events

    pub fn event(&mut self, ev: Event) {
        // Location changes arrive for every window on the desktop (and their child windows);
        // only the focused window's own moves can move the border.
        if !matches!(ev, Event::Location(h) if Some(h) != self.border_watch) {
            self.border_dirty = true;
        }
        if !matches!(ev, Event::Location(_)) {
            self.events_dirty = true;
        }
        match ev {
            Event::Show(h) => {
                if self.windows.contains_key(&h) {
                    if take_pending(&mut self.pending_show, h) {
                        // Our own show (workspace switch) has landed: some apps (Steam)
                        // come back black until something makes them render a frame.
                        if self.windows[&h].repaint {
                            win::repaint(h);
                        }
                        return;
                    }
                    // Only an app re-showing a window lattice hid counts as activation.
                    if self.hidden.contains_key(&h) && self.settled(h) && win::is_visible(h) {
                        self.activated_elsewhere(h);
                    }
                } else {
                    self.try_manage(h, None, true);
                }
            }
            Event::Uncloaked(h) => {
                if !self.windows.contains_key(&h) {
                    self.try_manage(h, None, true);
                }
            }
            Event::Hide(h) => {
                if self.windows.contains_key(&h)
                    && !take_pending(&mut self.pending_hide, h)
                    && !self.hidden.contains_key(&h)
                    && !win::is_visible(h)
                {
                    // The app hid itself (closed to tray, etc.).
                    self.unmanage(h);
                }
            }
            Event::Cloaked(h) => {
                // Cloaked windows live on another Windows virtual desktop, or are suspended UWP apps.
                if self.windows.contains_key(&h) && win::is_cloaked(h) {
                    self.unmanage(h);
                }
            }
            Event::Destroy(h) => self.unmanage(h),
            Event::Foreground(h) => {
                if let Some(w) = self.windows.get(&h) {
                    if w.ws == self.active {
                        self.note_focus(h);
                        if self.workspaces[self.active].layout == LayoutKind::Monocle {
                            win::raise(h);
                        }
                    } else if self.settled(h) && win::is_visible(h) && win::foreground() == h {
                        // A late event from before a workspace switch also lands here, so
                        // follow only a window that is really on screen and in front now.
                        self.activated_elsewhere(h);
                    }
                } else {
                    self.try_manage(h, None, true);
                }
                self.publish();
            }
            Event::MinimizeStart(h) => {
                self.stop_anim(h);
                if let Some(w) = self.windows.get_mut(&h) {
                    w.minimized = true;
                    let ws = w.ws;
                    self.retile(ws);
                }
            }
            Event::MinimizeEnd(h) => {
                if let Some(w) = self.windows.get_mut(&h) {
                    w.minimized = false;
                    let ws = w.ws;
                    if ws != self.active && self.settled(h) {
                        self.activated_elsewhere(h);
                    }
                    self.retile(ws);
                }
            }
            Event::MoveSizeEnd(h) => self.drag_ended(h),
            Event::NameChange(h) => {
                if let Some(w) = self.windows.get_mut(&h) {
                    // Titles change constantly (terminals animate them, browsers put the page
                    // in them). The bar only shows the focused window's title, plus a name
                    // that, for UWP apps, comes from the title: republish only for those.
                    let title = win::title(h);
                    if title == w.title {
                        return;
                    }
                    let name = app_name(&w.exe, &title);
                    let renamed = *w.name != *name;
                    if renamed {
                        w.name = name.into();
                    }
                    w.title = title;
                    if renamed || self.focused_window() == Some(h) {
                        self.publish();
                    }
                } else if win::is_visible(h) {
                    self.try_manage(h, None, true);
                }
            }
            // Handled by `update_border`, which runs after each batch of events.
            Event::Location(_) => {}
        }
    }

    /// A window on a hidden workspace was shown or activated by its app.
    fn activated_elsewhere(&mut self, h: Hwnd) {
        let ws = self.windows[&h].ws;
        if ws == self.active {
            return;
        }
        if self.cfg.general.follow_activation {
            log::info!("{h:#x} activated on hidden workspace {}; switching", ws + 1);
            self.workspaces[ws].last_focus = Some(h);
            self.switch_to_workspace(ws);
        } else {
            // Keep it where it belongs: re-hide what the app showed.
            self.hidden.remove(&h);
            self.hide_window(h);
        }
    }

    /// The user finished dragging a window: drop it where the cursor is.
    fn drag_ended(&mut self, h: Hwnd) {
        let Some(w) = self.windows.get(&h) else { return };
        let (ws, mon, floating) = (w.ws, w.mon, w.floating);
        if ws != self.active {
            return;
        }
        let cursor = win::cursor_pos();
        let target = self.monitor_index_at(cursor);
        if target != mon {
            if floating {
                // Floating windows keep the position they were dropped at.
                self.windows.get_mut(&h).unwrap().mon = target;
            } else {
                self.move_window_to_monitor(h, target);
            }
            self.note_focus(h);
            self.publish();
            return;
        }
        if floating {
            return;
        }
        // Dropped onto another tiled window: swap them.
        let under = self
            .on_monitor(ws, mon)
            .into_iter()
            .filter(|x| *x != h && !self.windows[x].floating && !self.windows[x].minimized)
            .find(|x| self.rect_of(*x).is_some_and(|r| r.contains(cursor)));
        if let Some(other) = under {
            let list = &mut self.workspaces[ws].windows;
            if let (Some(a), Some(b)) = (list.iter().position(|x| *x == h), list.iter().position(|x| *x == other)) {
                list.swap(a, b);
            }
        }
        self.retile(ws);
    }

    // -----------------------------------------------------------------------------------------
    // Bar

    pub fn publish(&self) {
        let workspaces = self
            .workspaces
            .iter()
            .enumerate()
            .map(|(i, ws)| WsInfo {
                number: i + 1,
                visible: i == self.active,
                focused: i == self.active,
                apps: ws.windows.iter().filter_map(|h| self.windows.get(h)).map(|w| shared::AppInfo { name: w.name.clone(), icon: w.icon.clone() }).collect(),
            })
            .collect();
        let title = self.focused_window().and_then(|h| self.windows.get(&h)).map(|w| w.title.clone()).unwrap_or_default();
        let layout = self.workspaces[self.active].layout.name();
        shared::set_focus_state(self.focused_window(), self.active, layout_code(layout));
        shared::publish(BarModel { workspaces, title, layout: layout.to_string() });
    }

    /// Keeps the focus border wrapped around the focused window (or hidden when no managed
    /// window on screen has focus). Runs once per batch of events, and only does anything when
    /// one of them could have moved it; the border itself repaints only on a size change.
    pub fn update_border(&mut self) {
        if !std::mem::take(&mut self.border_dirty) || self.border.is_none() {
            return;
        }
        let focused = self.focused_window();
        self.border_watch = focused;
        let target = focused.filter(|h| {
            let w = &self.windows[h];
            w.ws == self.active
                && !w.minimized
                && !self.hidden.contains_key(h)
                && !self.anim.is_animating(*h)
                && win::is_visible(*h)
                && !win::is_maximized(*h)
        });
        let placed = target.and_then(|h| {
            let scale = self.monitors.get(self.windows[&h].mon).map(|m| m.info.scale).unwrap_or(1.0);
            Some((win::frame_rect(h)?, scale))
        });
        if let Some(b) = self.border.as_mut() {
            b.set(placed);
        }
    }

    pub fn shutdown(&mut self) {
        self.finish_animations();
        self.anim.flush();
        shared::restore_hidden();
        for h in self.hidden.keys() {
            win::show(*h);
        }
    }
}

/// A layout as the tour's status reports it.
fn layout_code(name: &str) -> u8 {
    match name {
        "master" => 1,
        "monocle" => 2,
        _ => 0,
    }
}

fn overlap(a0: i32, a1: i32, b0: i32, b1: i32) -> i32 {
    a1.min(b1) - a0.max(b0)
}

fn take_pending(map: &mut HashMap<Hwnd, u32>, h: Hwnd) -> bool {
    match map.get_mut(&h) {
        Some(n) if *n > 0 => {
            *n -= 1;
            if *n == 0 {
                map.remove(&h);
            }
            true
        }
        _ => false,
    }
}

/// Short name for the bar: the exe without ".exe", or the title for UWP-hosted apps.
fn app_name(exe: &str, title: &str) -> String {
    let stem = exe.strip_suffix(".exe").or_else(|| exe.strip_suffix(".EXE")).unwrap_or(exe);
    let name = if stem.is_empty() || stem.eq_ignore_ascii_case("ApplicationFrameHost") {
        title.rsplit(" - ").next().unwrap_or(title)
    } else {
        stem
    };
    let name = name.to_lowercase();
    if name.chars().count() > 14 { name.chars().take(13).chain(std::iter::once('…')).collect() } else { name }
}

/// An app's name for window search: the whole exe name without ".exe" (the bar's short name
/// is lowercased and cut), or the bar's name for UWP-hosted apps, whose exe says nothing.
fn search_name(exe: &str, short: &str) -> String {
    let stem = exe.get(..exe.len().saturating_sub(4)).filter(|_| exe.to_ascii_lowercase().ends_with(".exe")).unwrap_or(exe);
    if stem.is_empty() || stem.eq_ignore_ascii_case("ApplicationFrameHost") { short.to_string() } else { stem.to_string() }
}

/// A process started on the user's behalf: no console window of its own, and without our
/// Rune switch (see `main`) in its environment.
pub fn background_command(program: &str) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut c = std::process::Command::new(program);
    c.creation_flags(CREATE_NO_WINDOW).env_remove(crate::CPU_FIRST_ENV);
    c
}

fn spawn(line: &str) {
    use std::os::windows::process::CommandExt;
    // `start` resolves App Paths (wt.exe, code, ...) and URIs like a Run dialog would.
    let result = background_command("cmd").raw_arg(format!("/C start \"\" {line}")).spawn();
    if let Err(e) = result {
        log::error!("exec '{line}': {e}");
    }
}

/// Opens the settings window: this same executable with `--settings` (it brings an already
/// open settings window to the front instead of opening a second one).
fn open_settings() {
    // We just handled the user's key press, so we may hand the foreground on: lets the new
    // window (or the existing one it activates) come to the front instead of flashing.
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(
            windows::Win32::UI::WindowsAndMessaging::ASFW_ANY,
        );
    }
    match std::env::current_exe() {
        Ok(exe) => {
            if let Err(e) = std::process::Command::new(exe).arg("--settings").env_remove(crate::CPU_FIRST_ENV).spawn() {
                log::error!("open settings: {e}");
            }
        }
        Err(e) => log::error!("open settings: {e}"),
    }
}

/// Opens config.toml with whatever is registered for .toml files, else Notepad.
fn open_config() {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{HSTRING, w};
    let path = config::config_path();
    let ok = unsafe { ShellExecuteW(None, w!("open"), &HSTRING::from(path.as_os_str()), None, None, SW_SHOWNORMAL) }.0 as isize > 32;
    if !ok && let Err(e) = std::process::Command::new("notepad.exe").arg(&path).spawn() {
        log::error!("open config: {e}");
    }
}
