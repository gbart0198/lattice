//! The window manager's state machine. Runs entirely on the WM thread; `hooks.rs` feeds it
//! events and commands one at a time.
//!
//! Workspaces span every monitor: one workspace is active at a time, each window belongs to a
//! workspace *and* a monitor, and switching workspaces swaps the contents of all monitors at
//! once. Switching hides the old workspace's windows (`SW_HIDE`) and shows the new one's: no
//! Windows virtual desktops, so there's no animation and no delay.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::config::{self, Command, Config, Dir, NewWindow, WsTarget};
use crate::layout::{self, LayoutKind, Params, Rect};
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
    /// A window moved or resized (keeps the focus border on it).
    Location,
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
    icon: Option<crate::icon::AppIcon>,
    title: String,
}

struct Workspace {
    /// Every window on the workspace, all monitors; tiled ones are laid out in this order.
    windows: Vec<Hwnd>,
    layout: LayoutKind,
    split_ratio: f32,
    master_ratio: f32,
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
    /// The focus-border overlay window (found lazily) and where it was last put.
    border_hwnd: Hwnd,
    border_rect: Option<Rect>,
}

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
            border_hwnd: 0,
            border_rect: None,
            cfg,
        };
        wm.build_workspaces();
        wm.refresh_monitors();
        wm.adopt_existing();
        wm.focused_monitor = wm.monitor_index_at(win::cursor_pos());
        wm.retile(wm.active);
        wm.publish();
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
        let mut rule_ws = None;
        let mut repaint = false;
        for r in &self.cfg.rules {
            if r.matches(&c.exe, &c.class, &c.title) {
                if r.ignore {
                    return false;
                }
                floating |= r.float;
                repaint |= r.repaint;
                if rule_ws.is_none() {
                    rule_ws = r.workspace.map(|w| w.saturating_sub(1).min(self.workspaces.len() - 1));
                }
            }
        }
        let ws = rule_ws.unwrap_or(self.active);
        let mon = mon.unwrap_or(self.focused_monitor);
        let minimized = win::is_minimized(hwnd);
        log::info!(
            "manage {hwnd:#x} '{}' ({}, {}) -> ws {} monitor {}{}",
            c.title, c.exe, c.class, ws + 1, mon, if floating { " floating" } else { "" }
        );
        self.windows.insert(hwnd, Window { ws, mon, floating, minimized, repaint, icon: crate::icon::for_window(hwnd, format!("{hwnd:x}")), exe: c.exe, title: c.title });

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
        self.retile(ws);
        self.publish();
        true
    }

    fn unmanage(&mut self, hwnd: Hwnd) {
        let Some(w) = self.windows.remove(&hwnd) else { return };
        log::info!("unmanage {hwnd:#x} '{}'", w.title);
        self.pending_hide.remove(&hwnd);
        self.pending_show.remove(&hwnd);
        self.hidden.remove(&hwnd);
        shared::mark_hidden(hwnd, false);
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
            self.hidden.insert(hwnd, Instant::now());
            if win::is_visible(hwnd) {
                *self.pending_hide.entry(hwnd).or_default() += 1;
            }
            win::hide(hwnd);
        }
        shared::mark_hidden(hwnd, true);
    }

    fn show_window(&mut self, hwnd: Hwnd) {
        if self.hidden.remove(&hwnd).is_some() {
            *self.pending_show.entry(hwnd).or_default() += 1;
            win::show(hwnd);
        }
        shared::mark_hidden(hwnd, false);
    }

    /// True once a window lattice hid has been hidden long enough that events about it are
    /// the app's doing, not late echoes of our own hide or an earlier focus change.
    fn settled(&self, hwnd: Hwnd) -> bool {
        self.hidden.get(&hwnd).is_none_or(|t| t.elapsed() > Duration::from_millis(600))
    }

    fn center_floating(&self, hwnd: Hwnd) {
        let area = self.tiling_area(self.windows[&hwnd].mon);
        let cur = win::frame_rect(hwnd).unwrap_or(area);
        // Keep the window's own size if it fits, else 60% of the area.
        let (w, h) = if cur.w < area.w && cur.h < area.h { (cur.w, cur.h) } else { (area.w * 3 / 5, area.h * 3 / 5) };
        win::place(hwnd, Rect::new(area.x + (area.w - w) / 2, area.y + (area.h - h) / 2, w, h));
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
            self.retile_monitor(ws_idx, mon);
        }
    }

    fn retile_monitor(&self, ws_idx: usize, mon: usize) {
        let ws = &self.workspaces[ws_idx];
        let area = self.tiling_area(mon);
        let tiled: Vec<Hwnd> = self
            .on_monitor(ws_idx, mon)
            .into_iter()
            .filter(|h| self.windows.get(h).is_some_and(|w| !w.floating && !w.minimized))
            .collect();
        if let Some(f) = ws.fullscreen.filter(|f| tiled.contains(f)) {
            place_if_moved(f, area);
            win::raise(f);
            return;
        }
        let params = Params {
            gaps_inner: self.cfg.general.gaps_inner,
            gaps_outer: self.cfg.general.gaps_outer,
            ratio: if ws.layout == LayoutKind::Master { ws.master_ratio } else { ws.split_ratio },
        };
        let rects = layout::tile(ws.layout, area, tiled.len(), &params);
        for (hwnd, r) in tiled.iter().zip(rects) {
            place_if_moved(*hwnd, r);
        }
        if ws.layout == LayoutKind::Monocle
            && let Some(top) = ws.mon_focus.get(&mon).filter(|f| tiled.contains(f))
        {
            win::raise(*top);
        }
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
        // Show the incoming windows first, then hide the outgoing ones: no flash of desktop.
        for hwnd in self.workspaces[ws].windows.clone() {
            self.show_window(hwnd);
        }
        self.retile(ws);
        for hwnd in self.workspaces[old].windows.clone() {
            self.hide_window(hwnd);
        }
    }

    /// Records that `hwnd` has focus (workspace memory, focused monitor).
    fn note_focus(&mut self, hwnd: Hwnd) {
        let Some(w) = self.windows.get(&hwnd) else { return };
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
            && let Some(r) = win::frame_rect(hwnd)
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
                if let Some(r) = win::frame_rect(h) {
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
        let from = cur.and_then(win::frame_rect).unwrap_or_else(|| self.tiling_area(self.focused_monitor));
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
            && let Some(from) = win::frame_rect(h)
            && let Some(Target::Window(other)) = self.neighbor(from, Some(h), dir, Some(mon))
        {
            let list = &mut self.workspaces[ws].windows;
            if let (Some(a), Some(b)) = (list.iter().position(|x| *x == h), list.iter().position(|x| *x == other)) {
                list.swap(a, b);
            }
            self.retile(ws);
            self.focus(h);
            return;
        }
        // Nothing to swap with on this screen: move to the monitor in that direction.
        if let Some(t) = self.monitor_toward(mon, dir) {
            self.move_window_to_monitor(h, t);
            self.focus(h);
            self.publish();
        }
    }

    // -----------------------------------------------------------------------------------------
    // Commands

    pub fn command(&mut self, cmd: Command) {
        log::debug!("command {cmd:?}");
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
            Command::Resize(delta) => {
                let w = &mut self.workspaces[self.active];
                let r = if w.layout == LayoutKind::Master { &mut w.master_ratio } else { &mut w.split_ratio };
                *r = (*r + delta).clamp(0.1, 0.9);
                self.retile(self.active);
            }
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
                }
                self.cfg = cfg;
                self.build_workspaces();
                self.reload_keymap = true;
                self.refresh_monitors();
                self.retile(self.active);
                self.publish();
                log::info!("config reloaded");
            }
            Command::Exec(line) => spawn(&line),
            Command::Settings => open_settings(),
            Command::OpenConfig => open_config(),
            Command::Quit => self.quit = true,
        }
    }

    pub fn config(&self) -> &Config {
        &self.cfg
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
                    w.title = win::title(h);
                    self.publish();
                } else if win::is_visible(h) {
                    self.try_manage(h, None, true);
                }
            }
            // Handled by `update_border`, which runs after every event.
            Event::Location => {}
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
            .find(|x| win::frame_rect(*x).is_some_and(|r| r.contains(cursor)));
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
                apps: ws.windows.iter().filter_map(|h| self.windows.get(h)).map(|w| shared::AppInfo { name: app_name(&w.exe, &w.title), icon: w.icon.clone() }).collect(),
            })
            .collect();
        let title = self.focused_window().and_then(|h| self.windows.get(&h)).map(|w| w.title.clone()).unwrap_or_default();
        shared::publish(BarModel { workspaces, title, layout: self.workspaces[self.active].layout.name().to_string() });
    }

    /// Keeps the focus-border overlay wrapped around the focused window (or hidden when no
    /// managed window on screen has focus). Runs after every event and command; it only
    /// touches the overlay when the target rectangle actually changed.
    pub fn update_border(&mut self) {
        let b = &self.cfg.border;
        if !b.enabled {
            return;
        }
        if self.border_hwnd == 0 || !win::is_window(self.border_hwnd) {
            self.border_hwnd = win::find_own_window(crate::border::TITLE);
            self.border_rect = None;
            if self.border_hwnd == 0 {
                return; // Rune hasn't created it yet
            }
        }
        let target = self.focused_window().filter(|h| {
            let w = &self.windows[h];
            w.ws == self.active && !w.minimized && !self.hidden.contains_key(h) && win::is_visible(*h) && !win::is_maximized(*h)
        });
        let rect = target.and_then(|h| {
            let r = win::frame_rect(h)?;
            let scale = self.monitors.get(self.windows[&h].mon).map(|m| m.info.scale).unwrap_or(1.0);
            let m = (b.margin() * scale).round() as i32;
            Some(Rect::new(r.x - m, r.y - m, r.w + 2 * m, r.h + 2 * m))
        });
        if rect == self.border_rect {
            return;
        }
        self.border_rect = rect;
        match rect {
            Some(r) => win::place_overlay(self.border_hwnd, r),
            None => win::hide(self.border_hwnd),
        }
    }

    pub fn shutdown(&mut self) {
        shared::restore_hidden();
        for h in self.hidden.keys() {
            win::show(*h);
        }
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

fn place_if_moved(hwnd: Hwnd, r: Rect) {
    if win::frame_rect(hwnd) != Some(r) || win::is_maximized(hwnd) {
        win::place(hwnd, r);
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

fn spawn(line: &str) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // `start` resolves App Paths (wt.exe, code, ...) and URIs like a Run dialog would.
    let result = std::process::Command::new("cmd")
        .raw_arg(format!("/C start \"\" {line}"))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
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
            if let Err(e) = std::process::Command::new(exe).arg("--settings").spawn() {
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
