//! Setups (`[[setup]]` in the config): a named set of apps, the workspaces they go on,
//! their tiling order and each workspace's layout. Restoring one (`restore-setup NAME`, its
//! own key, or the launcher's picker) moves the apps that are open into place and starts the rest;
//! their windows are put where they belong as they appear (`wm.rs`).
//!
//! Pure logic only: which open window fills which spot (`plan`), the picker's thumbnails, and
//! the edits the settings window's layout editor makes (`insert_index`, `set_ratio`, ...), which
//! follow the rules lattice itself tiles by, so what you build there is what you get.

use crate::config::{NewWindow, Setup, SetupWorkspace};
use crate::layout::{self, LayoutKind, Params, Rect};

/// One app of a setup: where it goes.
#[derive(Clone, Debug, PartialEq)]
pub struct Slot {
    /// Workspace index (0-based).
    pub ws: usize,
    /// Monitor index (0-based, left to right).
    pub mon: usize,
    /// Position in that workspace's tiling order.
    pub order: usize,
    pub exe: String,
    pub run: String,
}

/// What restoring a setup does.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Plan {
    pub slots: Vec<Slot>,
    /// Open windows that fill a slot: (slot index, window).
    pub claims: Vec<(usize, isize)>,
    /// Slots with no open window: their app is started.
    pub launch: Vec<usize>,
}

/// How many monitors a setup is laid out for: `monitors`, or the highest its entries name
/// (at least 1).
pub fn monitor_count(s: &Setup) -> usize {
    let named = s.workspaces.iter().filter_map(|w| w.monitor).max().unwrap_or(1);
    s.monitors.unwrap_or(named).max(named).max(1)
}

/// Which of a setup's monitors (0-based) an entry is on, as the editor and thumbnails draw it:
/// a one-monitor setup's entries are on its only one, and an entry without a monitor is on
/// the primary's place (or the last).
pub fn monitor_slot(count: usize, m: Option<usize>, primary: usize) -> usize {
    if count <= 1 {
        return 0;
    }
    m.and_then(|m| m.checked_sub(1)).unwrap_or(primary).min(count - 1)
}

/// The desktop area each of a setup's `count` monitors is drawn with: a one-monitor setup's
/// is the primary's; otherwise monitor k is the k-th connected one from the left, and one
/// that isn't connected is the primary's size, past the right edge of the rest.
pub fn monitor_areas(count: usize, connected: &[Rect], primary: usize) -> Vec<Rect> {
    let fallback = Rect::new(0, 0, 1920, 1040);
    let prim = connected.get(primary).or(connected.first()).copied().unwrap_or(fallback);
    if count <= 1 {
        return vec![prim];
    }
    let mut right = connected.iter().map(|r| r.x + r.w).max().unwrap_or(prim.x + prim.w);
    let top = connected.iter().map(|r| r.y).min().unwrap_or(prim.y);
    (0..count)
        .map(|k| match connected.get(k) {
            Some(r) => *r,
            None => {
                let r = Rect::new(right, top, prim.w, prim.h);
                right += prim.w;
                r
            }
        })
        .collect()
}

/// A setup entry as it lands on this desktop.
#[derive(Clone, Copy, Debug)]
pub struct Placed<'a> {
    pub entry: &'a SetupWorkspace,
    /// Workspace and monitor indexes.
    pub ws: usize,
    pub mon: usize,
    /// Its monitor isn't connected: its apps join the primary's.
    pub folded: bool,
    /// Where its first app goes in that monitor's tiling order: after the apps of the entries
    /// that belong there, for a folded one.
    pub start: usize,
    /// Its splits are the monitor's: the first entry that belongs there, else the first folded
    /// onto it. The apps of others go past the splits it saved, on the default ratio.
    pub splits: bool,
}

/// Where each of a setup's entries goes with `connected` monitors, `primary` the primary's
/// index: monitor k on the k-th from the left, a one-monitor setup (and an entry without a
/// monitor) on the primary. A monitor that isn't connected folds onto the primary, after its
/// own apps, so nothing is lost with fewer monitors than the setup was made with.
pub fn placed(setup: &Setup, ws_index: impl Fn(usize) -> usize, connected: usize, primary: usize) -> Vec<Placed<'_>> {
    let count = monitor_count(setup);
    let mut out: Vec<Placed> = setup
        .workspaces
        .iter()
        .map(|w| {
            let (mon, folded) = match w.monitor.and_then(|m| m.checked_sub(1)) {
                Some(i) if count > 1 && i < connected => (i, false),
                Some(_) if count > 1 => (primary, true),
                _ => (primary, false),
            };
            Placed { entry: w, ws: ws_index(w.number), mon, folded, start: 0, splits: false }
        })
        .collect();
    let mut next: std::collections::HashMap<(usize, usize), usize> = std::collections::HashMap::new();
    for folded in [false, true] {
        for p in out.iter_mut().filter(|p| p.folded == folded) {
            let n = next.entry((p.ws, p.mon)).or_insert(0);
            (p.start, p.splits) = (*n, *n == 0);
            *n += p.entry.apps.len().max(1);
        }
    }
    out
}

/// An open window as `plan` sees it.
#[derive(Clone, Copy, Debug)]
pub struct OpenWindow<'a> {
    pub hwnd: isize,
    pub exe: &'a str,
    pub ws: usize,
    pub mon: usize,
    /// Its place in its workspace's tiling order.
    pub place: usize,
}

/// Matches the setup's apps to open windows of the same program. A window already where it
/// goes (workspace and monitor) is taken first, then one on the right workspace, each in tiling
/// order (two Edge windows already in place keep their places), so restoring a setup that's in
/// place changes nothing; then one on the right monitor, so windows don't cross monitors for
/// nothing; then any other one, in the order given (most recently used first gives the window
/// you were last in). `ws_index` turns a 1-based workspace number into an index; monitors
/// go as `placed` puts them.
pub fn plan(setup: &Setup, open: &[OpenWindow], ws_index: impl Fn(usize) -> usize, connected: usize, primary: usize) -> Plan {
    let mut slots = Vec::new();
    for p in placed(setup, ws_index, connected, primary) {
        for (j, app) in p.entry.apps.iter().enumerate() {
            if app.exe.trim().is_empty() {
                continue;
            }
            slots.push(Slot { ws: p.ws, mon: p.mon, order: p.start + j, exe: app.exe.trim().to_string(), run: app.command_line().to_string() });
        }
    }
    let mut taken = vec![false; open.len()];
    let mut filled: Vec<Option<isize>> = vec![None; slots.len()];
    // 0: same workspace and monitor; 1: same workspace; 2: same monitor; 3: anywhere.
    for pass in 0..4 {
        for (i, slot) in slots.iter().enumerate() {
            if filled[i].is_some() {
                continue;
            }
            let mut fits = open.iter().enumerate().filter(|(j, o)| {
                !taken[*j]
                    && o.exe.eq_ignore_ascii_case(&slot.exe)
                    && (pass >= 2 || o.ws == slot.ws)
                    && (pass == 1 || pass == 3 || o.mon == slot.mon)
            });
            let found = if pass < 2 { fits.min_by_key(|(_, o)| o.place) } else { fits.next() }.map(|(j, _)| j);
            if let Some(j) = found {
                taken[j] = true;
                filled[i] = Some(open[j].hwnd);
            }
        }
    }
    let claims = filled.iter().enumerate().filter_map(|(i, h)| Some((i, (*h)?))).collect();
    let launch = filled.iter().enumerate().filter(|(_, h)| h.is_none()).map(|(i, _)| i).collect();
    Plan { slots, claims, launch }
}

/// `order` with the windows in `first` moved to the front in that order; the rest keep theirs.
pub fn arrange(order: &[isize], first: &[isize]) -> Vec<isize> {
    let mut out: Vec<isize> = first.iter().copied().filter(|h| order.contains(h)).collect();
    out.extend(order.iter().copied().filter(|h| !first.contains(h)));
    out
}

/// A setup's layout, filling in what it leaves out from `[general]`.
pub fn workspace_layout(w: &SetupWorkspace, default: LayoutKind) -> LayoutKind {
    w.layout.unwrap_or(default)
}

/// Tiles of a setup workspace in an area of the given size: where `tile` would put its apps.
pub fn tiles(w: &SetupWorkspace, area: Rect, defaults: &Defaults) -> Vec<Rect> {
    layout::tile(workspace_layout(w, defaults.layout), area, w.apps.len(), &params(w, defaults, 1.0))
}

/// The tiling settings a preview needs from `[general]`.
#[derive(Clone, Copy, Debug)]
pub struct Defaults {
    pub layout: LayoutKind,
    pub split_ratio: f32,
    pub master_ratio: f32,
    pub gaps_inner: i32,
    pub gaps_outer: i32,
    pub new_window: NewWindow,
}

impl Defaults {
    pub fn from(g: &crate::config::General) -> Self {
        Defaults {
            layout: g.layout,
            split_ratio: g.split_ratio,
            master_ratio: g.master_ratio,
            gaps_inner: g.gaps_inner,
            gaps_outer: g.gaps_outer,
            new_window: g.new_window,
        }
    }
}

/// Layout parameters for a setup workspace, gaps scaled by `scale` (a preview is smaller than
/// the screen).
pub fn params<'a>(w: &'a SetupWorkspace, d: &Defaults, scale: f32) -> Params<'a> {
    let kind = workspace_layout(w, d.layout);
    Params {
        gaps_inner: (d.gaps_inner as f32 * scale).round() as i32,
        gaps_outer: (d.gaps_outer as f32 * scale).round() as i32,
        ratio: if kind == LayoutKind::Master { w.master_ratio.unwrap_or(d.master_ratio) } else { d.split_ratio },
        splits: &w.splits,
    }
}

// ---------------------------------------------------------------------------------------------
// Editing (the settings window's layout editor)

/// Where a new app goes among `len`, the way lattice places a new window: after the `selected`
/// one (the editor's stand-in for the focused window) with `new_window = "after_focused"`, else
/// at the end. Split ratios stay with their positions, as on a live workspace.
pub fn insert_index(len: usize, selected: Option<usize>, new_window: NewWindow) -> usize {
    match (new_window, selected) {
        (NewWindow::AfterFocused, Some(i)) if i < len => i + 1,
        _ => len,
    }
}

/// Forgets ratios past the last split of `apps` tiles, as lattice does when a window goes.
pub fn forget_splits(splits: &mut Vec<f32>, apps: usize) {
    splits.truncate(apps.saturating_sub(1));
}

/// Sets the ratio a seam stands for (see `layout::seams`), rounded to a percent.
pub fn set_ratio(splits: &mut Vec<f32>, master: &mut Option<f32>, target: layout::ResizeTarget, ratio: f32, default_split: f32) {
    let ratio = (ratio.clamp(0.1, 0.9) * 100.0).round() / 100.0;
    match target {
        layout::ResizeTarget::Master => *master = Some(ratio),
        layout::ResizeTarget::Split(i) => {
            if splits.len() <= i {
                splits.resize(i + 1, default_split);
            }
            splits[i] = ratio;
        }
    }
}

/// The short name an app goes by in previews: the exe without ".exe".
pub fn app_label(exe: &str) -> String {
    let exe = exe.trim();
    let stem = exe.len().checked_sub(4).filter(|&n| exe[n..].eq_ignore_ascii_case(".exe")).map_or(exe, |n| &exe[..n]);
    if stem.is_empty() { "app".into() } else { stem.to_string() }
}

/// The command line that starts the program at `path`, quoted when it has spaces.
pub fn run_for_path(path: &str) -> String {
    if path.contains(' ') { format!("\"{path}\"") } else { path.to_string() }
}

/// A short description of a setup: "1: Code, Terminal · 2: msedge" (a workspace's apps on
/// every monitor together).
pub fn summary(s: &Setup) -> String {
    let parts: Vec<String> = numbers(&s.workspaces)
        .into_iter()
        .map(|n| {
            let names: Vec<String> = s.workspaces.iter().filter(|w| w.number == n).flat_map(|w| &w.apps).map(|a| app_label(&a.exe)).collect();
            format!("{n}: {}", names.join(", "))
        })
        .collect();
    if parts.is_empty() { "No apps yet".into() } else { parts.join("  ·  ") }
}

/// The workspace numbers a setup puts apps on, in the order it first names them.
pub fn numbers(entries: &[SetupWorkspace]) -> Vec<usize> {
    let mut out = Vec::new();
    for w in entries.iter().filter(|w| !w.apps.is_empty()) {
        if !out.contains(&w.number) {
            out.push(w.number);
        }
    }
    out
}

/// A workspace drawn small: every monitor it uses and the tiles on them, as fractions
/// (`[x, y, w, h]`) of the box around those monitors, whose width over height is `aspect`.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Thumb {
    pub aspect: f32,
    pub screens: Vec<[f32; 4]>,
    pub tiles: Vec<[f32; 4]>,
}

/// The size of a `aspect`-shaped box fitted inside `w` × `h`.
pub fn fit(aspect: f32, w: f32, h: f32) -> (f32, f32) {
    if aspect <= 0.0 {
        (w, h)
    } else if aspect >= w / h {
        (w, w / aspect)
    } else {
        (h * aspect, h)
    }
}

/// The thumbnail of workspace `n` of a setup: each of its monitors (as `monitor_areas` gives
/// them), empty or not.
pub fn ws_thumb(s: &Setup, n: usize, areas: &[Rect], primary: usize, d: &Defaults) -> Thumb {
    let count = areas.len().max(1);
    let mut entries: Vec<SetupWorkspace> = Vec::new();
    for k in 0..count {
        let mine = s.workspaces.iter().filter(|w| w.number == n && monitor_slot(count, w.monitor, primary) == k);
        let mut e = SetupWorkspace { number: n, monitor: Some(k + 1), layout: None, splits: Vec::new(), master_ratio: None, apps: Vec::new() };
        for (i, w) in mine.enumerate() {
            if i == 0 {
                (e.layout, e.splits, e.master_ratio) = (w.layout, w.splits.clone(), w.master_ratio);
            }
            e.apps.extend(w.apps.iter().cloned());
        }
        entries.push(e);
    }
    // An empty monitor still shows the workspace's layout on the others; that's all it shares.
    let refs: Vec<&SetupWorkspace> = entries.iter().collect();
    let area = |m: Option<usize>| areas.get(m.map_or(0, |m| m - 1)).copied().unwrap_or(Rect::new(0, 0, 1920, 1040));
    thumb(&refs, area, d)
}

/// The thumbnail of one workspace from its setup `entries` (one per monitor). `area` gives a
/// setup monitor's tiling area on the desktop (where `tiles` would lay it out).
pub fn thumb(entries: &[&SetupWorkspace], area: impl Fn(Option<usize>) -> Rect, d: &Defaults) -> Thumb {
    let areas: Vec<Rect> = entries.iter().map(|w| area(w.monitor)).collect();
    let mut screens: Vec<Rect> = Vec::new();
    for a in &areas {
        if !screens.contains(a) {
            screens.push(*a);
        }
    }
    let Some(first) = screens.first() else { return Thumb::default() };
    let (x0, y0) = screens.iter().fold((first.x, first.y), |(x, y), r| (x.min(r.x), y.min(r.y)));
    let (x1, y1) = screens.iter().fold((first.x + first.w, first.y + first.h), |(x, y), r| (x.max(r.x + r.w), y.max(r.y + r.h)));
    let (bw, bh) = ((x1 - x0).max(1) as f32, (y1 - y0).max(1) as f32);
    let frac = |r: Rect| [(r.x - x0) as f32 / bw, (r.y - y0) as f32 / bh, r.w as f32 / bw, r.h as f32 / bh];
    Thumb {
        aspect: bw / bh,
        screens: screens.iter().map(|r| frac(*r)).collect(),
        tiles: entries.iter().zip(&areas).flat_map(|(w, a)| tiles(w, *a, d)).map(frac).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SetupApp;

    fn app(exe: &str) -> SetupApp {
        SetupApp { exe: exe.into(), run: String::new() }
    }

    fn ws(number: usize, apps: &[&str]) -> SetupWorkspace {
        SetupWorkspace { number, monitor: None, layout: None, splits: Vec::new(), master_ratio: None, apps: apps.iter().map(|a| app(a)).collect() }
    }

    fn open(v: &[(isize, &'static str, usize, usize)]) -> Vec<OpenWindow<'static>> {
        v.iter().map(|&(hwnd, exe, ws, place)| OpenWindow { hwnd, exe, ws, mon: 0, place }).collect()
    }

    /// Workspace numbers and monitors as indexes, the primary being monitor 1.
    fn idx(n: usize) -> usize {
        n - 1
    }

    fn dev() -> Setup {
        Setup {
            name: "Dev".into(),
            key: None,
            monitors: None,
            workspaces: vec![ws(1, &["Code.exe", "WindowsTerminal.exe"]), ws(2, &["msedge.exe"]), ws(3, &["slack.exe"])],
        }
    }

    #[test]
    fn plan_claims_open_windows_and_launches_the_rest() {
        // Edge is open twice (one already on workspace 2), Code once on 5; nothing else runs.
        let open = open(&[(10, "msedge.exe", 0, 0), (11, "Code.exe", 4, 0), (12, "msedge.exe", 1, 0), (13, "notepad.exe", 0, 1)]);
        let p = plan(&dev(), &open, idx, 4, 0);
        assert_eq!(p.slots.len(), 4);
        assert_eq!(p.slots[1], Slot { ws: 0, mon: 0, order: 1, exe: "WindowsTerminal.exe".into(), run: "WindowsTerminal.exe".into() });
        // The Edge already on workspace 2 stays; Code comes over from 5.
        assert_eq!(p.claims, vec![(0, 11), (2, 12)]);
        assert_eq!(p.launch, vec![1, 3]);
    }

    #[test]
    fn plan_prefers_windows_already_in_place() {
        // Two Code slots on different workspaces; the window on workspace 2 must not be taken
        // by the first slot just because it comes first.
        let s = Setup { workspaces: vec![ws(1, &["Code.exe"]), ws(2, &["code.exe"])], ..dev() };
        let open = open(&[(1, "Code.exe", 1, 0), (2, "Code.exe", 6, 0)]);
        assert_eq!(plan(&s, &open, idx, 4, 0).claims, vec![(0, 2), (1, 1)]);
        // One window for two slots: the other one is launched.
        let p = plan(&s, &open[..1], idx, 4, 0);
        assert_eq!((p.claims, p.launch), (vec![(1, 1)], vec![0]));
    }

    #[test]
    fn windows_in_place_keep_their_order() {
        // Two Edge slots and two Edge windows already on workspace 1, the second one used
        // last: they keep their places rather than swapping by recency.
        let s = Setup { workspaces: vec![ws(1, &["msedge.exe", "msedge.exe"])], ..dev() };
        let o = open(&[(7, "msedge.exe", 0, 1), (5, "msedge.exe", 0, 0)]);
        assert_eq!(plan(&s, &o, idx, 4, 0).claims, vec![(0, 5), (1, 7)]);
        // Brought from elsewhere, the one used last goes first.
        let o = open(&[(7, "msedge.exe", 3, 1), (5, "msedge.exe", 3, 0)]);
        assert_eq!(plan(&s, &o, idx, 4, 0).claims, vec![(0, 7), (1, 5)]);
    }

    #[test]
    fn monitors_keep_their_windows() {
        // Edge on each of two monitors of workspace 1; the setup wants one on each. The one on
        // monitor 2 comes first in tiling order, but each stays on its own monitor.
        let left = ws(1, &["msedge.exe"]);
        let right = SetupWorkspace { monitor: Some(2), ..ws(1, &["msedge.exe"]) };
        let s = Setup { workspaces: vec![left, right], ..dev() };
        let o = vec![
            OpenWindow { hwnd: 8, exe: "msedge.exe", ws: 0, mon: 1, place: 0 },
            OpenWindow { hwnd: 9, exe: "msedge.exe", ws: 0, mon: 0, place: 1 },
        ];
        let p = plan(&s, &o, idx, 4, 0);
        assert_eq!((p.slots[0].mon, p.slots[1].mon), (0, 1));
        assert_eq!(p.claims, vec![(0, 9), (1, 8)]);
        // From another workspace, an Edge on monitor 2 takes monitor 2's spot, though monitor
        // 1's comes first; monitor 1's app is started.
        let o = vec![OpenWindow { hwnd: 8, exe: "msedge.exe", ws: 4, mon: 1, place: 0 }];
        let p = plan(&s, &o, idx, 4, 0);
        assert_eq!((p.claims, p.launch), (vec![(1, 8)], vec![0]));
    }

    #[test]
    fn thumbnails_show_every_monitor() {
        let d = Defaults { layout: LayoutKind::Dwindle, split_ratio: 0.5, master_ratio: 0.55, gaps_inner: 0, gaps_outer: 0, new_window: NewWindow::End };
        let left = ws(1, &["a.exe", "b.exe"]);
        let right = SetupWorkspace { monitor: Some(2), ..ws(1, &["c.exe"]) };
        let area = |m: Option<usize>| if m == Some(2) { Rect::new(1000, 0, 1000, 500) } else { Rect::new(0, 0, 1000, 500) };
        let t = thumb(&[&left, &right], area, &d);
        assert_eq!(t.aspect, 4.0);
        assert_eq!(t.screens, vec![[0.0, 0.0, 0.5, 1.0], [0.5, 0.0, 0.5, 1.0]]);
        assert_eq!(t.tiles, vec![[0.0, 0.0, 0.25, 1.0], [0.25, 0.0, 0.25, 1.0], [0.5, 0.0, 0.5, 1.0]]);
        let s = Setup { workspaces: vec![left, ws(2, &["d.exe"]), right], ..dev() };
        assert_eq!(summary(&s), "1: a, b, c  ·  2: d");
    }

    #[test]
    fn arrange_puts_setup_windows_first() {
        assert_eq!(arrange(&[5, 6, 7, 8], &[8, 6]), vec![8, 6, 5, 7]);
        assert_eq!(arrange(&[5], &[9, 5]), vec![5], "only windows that are there");
    }

    #[test]
    fn inserting_follows_new_window() {
        let mut w = ws(1, &["a.exe", "b.exe", "c.exe"]);
        let mut add = |sel, nw, exe: &str| {
            let at = insert_index(w.apps.len(), sel, nw);
            w.apps.insert(at, app(exe));
            at
        };
        assert_eq!(add(Some(0), NewWindow::AfterFocused, "d.exe"), 1);
        assert_eq!(add(Some(0), NewWindow::End, "e.exe"), 4);
        assert_eq!(add(None, NewWindow::AfterFocused, "f.exe"), 5);
        assert_eq!(add(Some(99), NewWindow::AfterFocused, "g.exe"), 6);
        let names: Vec<&str> = w.apps.iter().map(|a| a.exe.as_str()).collect();
        assert_eq!(names, ["a.exe", "d.exe", "b.exe", "c.exe", "e.exe", "f.exe", "g.exe"]);
    }

    #[test]
    fn inserted_tile_matches_live_tiling() {
        // A dwindle workspace with a tuned first split: adding a third app halves the second
        // tile, exactly as a third window opening there would.
        let d = Defaults { layout: LayoutKind::Dwindle, split_ratio: 0.5, master_ratio: 0.55, gaps_inner: 0, gaps_outer: 0, new_window: NewWindow::End };
        let area = Rect::new(0, 0, 1000, 500);
        let mut w = ws(1, &["a.exe", "b.exe"]);
        set_ratio(&mut w.splits, &mut w.master_ratio, layout::ResizeTarget::Split(0), 0.7, d.split_ratio);
        let at = insert_index(w.apps.len(), Some(1), d.new_window);
        w.apps.insert(at, app("c.exe"));
        let live = layout::tile(LayoutKind::Dwindle, area, 3, &Params { gaps_inner: 0, gaps_outer: 0, ratio: 0.5, splits: &[0.7] });
        assert_eq!(tiles(&w, area, &d), live);
        assert_eq!(live[0], Rect::new(0, 0, 700, 500));
    }

    #[test]
    fn removing_forgets_unused_ratios() {
        let mut splits = vec![0.6, 0.3];
        forget_splits(&mut splits, 2);
        assert_eq!(splits, vec![0.6]);
        forget_splits(&mut splits, 0);
        assert!(splits.is_empty());
    }

    #[test]
    fn ratios_are_set_and_rounded() {
        let (mut splits, mut master) = (Vec::new(), None);
        set_ratio(&mut splits, &mut master, layout::ResizeTarget::Split(2), 0.333, 0.5);
        assert_eq!(splits, vec![0.5, 0.5, 0.33]);
        set_ratio(&mut splits, &mut master, layout::ResizeTarget::Master, 0.97, 0.5);
        assert_eq!(master, Some(0.9));
    }

    #[test]
    fn labels_and_summaries() {
        assert_eq!(app_label("Code.exe"), "Code");
        assert_eq!(app_label("SLACK.EXE"), "SLACK");
        assert_eq!(app_label("x"), "x");
        assert_eq!(run_for_path(r"C:\Program Files\x\x.exe"), r#""C:\Program Files\x\x.exe""#);
        assert_eq!(summary(&dev()), "1: Code, WindowsTerminal  ·  2: msedge  ·  3: slack");
    }

    #[test]
    fn fewer_monitors_fold_onto_the_primary() {
        // Made on two monitors: 2 apps on monitor 1, 1 on monitor 2, then opened with one.
        let mut left = ws(1, &["a.exe", "b.exe"]);
        left.monitor = Some(1);
        left.splits = vec![0.7];
        let mut right = ws(1, &["c.exe"]);
        right.monitor = Some(2);
        right.splits = vec![0.3];
        let s = Setup { workspaces: vec![right, left], ..dev() };
        assert_eq!(monitor_count(&s), 2);
        let p = placed(&s, idx, 1, 0);
        // Monitor 2 folds onto the primary, after monitor 1's apps; monitor 1 keeps its splits.
        assert!(p[0].folded && !p[1].folded);
        assert_eq!((p[0].mon, p[0].start, p[0].splits), (0, 2, false));
        assert_eq!((p[1].mon, p[1].start, p[1].splits), (0, 0, true));
        let orders: Vec<(String, usize)> = plan(&s, &[], idx, 1, 0).slots.into_iter().map(|s| (s.exe, s.order)).collect();
        assert_eq!(orders, vec![("c.exe".into(), 2), ("a.exe".into(), 0), ("b.exe".into(), 1)]);
        // With both connected, each keeps its own monitor and splits.
        let p = placed(&s, idx, 2, 1);
        assert_eq!((p[0].mon, p[0].folded, p[0].splits), (1, false, true));
        assert_eq!((p[1].mon, p[1].folded, p[1].splits), (0, false, true));
    }

    #[test]
    fn one_monitor_setups_go_on_the_primary() {
        let mut w = ws(1, &["a.exe"]);
        w.monitor = Some(1);
        let s = Setup { workspaces: vec![w], ..dev() };
        // The primary is the right of two: a one-monitor setup goes there, not on the left.
        assert_eq!(placed(&s, idx, 2, 1)[0].mon, 1);
        // Laid out for three, with two connected: monitor 1 is the left one.
        let s = Setup { monitors: Some(3), ..s };
        assert_eq!(monitor_count(&s), 3);
        assert_eq!((placed(&s, idx, 2, 1)[0].mon, placed(&s, idx, 2, 1)[0].folded), (0, false));
    }

    #[test]
    fn missing_monitors_are_drawn_past_the_right_edge() {
        let screens = [Rect::new(-1920, 0, 1920, 1080), Rect::new(0, 32, 1920, 1008)];
        // The primary (the right one) for a one-monitor setup.
        assert_eq!(monitor_areas(1, &screens, 1), vec![screens[1]]);
        let a = monitor_areas(3, &screens[..1], 0);
        assert_eq!(a[1], Rect::new(0, 0, 1920, 1080));
        assert_eq!(a[2], Rect::new(1920, 0, 1920, 1080));
        assert_eq!(monitor_slot(3, None, 1), 1);
        assert_eq!(monitor_slot(1, Some(2), 1), 0);
    }
}
