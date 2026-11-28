//! The setups editor's state: the setups as edited (`SetupEdit`, one `WsEdit` a workspace
//! entry), the `Editor` that holds them with what's open and selected, and its operations on
//! monitors, workspaces and apps. Converts them to `config` types for drawing and saving.

use rune::TextEdit;
use rune_kit::ComboboxState;

use super::canvas::TileDrag;
use crate::config::{self, NewWindow, Setup, SetupApp, SetupWorkspace};
use crate::layout::{LayoutKind, Rect};
use crate::setups;

pub(super) struct AppEdit {
    pub(super) id: u64,
    pub(super) exe: TextEdit,
    /// How restoring starts it, which the editor doesn't show: what the picker or a capture
    /// gave, or the config's `run`. It goes with the program it was made for (`run_for`): change
    /// the program and it's started by its name instead.
    run: String,
    run_for: String,
}

impl AppEdit {
    /// The `run` to save: empty once the program was changed.
    pub(super) fn run(&self) -> &str {
        if self.exe.text.trim().eq_ignore_ascii_case(self.run_for.trim()) { self.run.trim() } else { "" }
    }
}

/// A setup's apps on one monitor of one workspace (a `[[setup.workspace]]` entry).
pub(super) struct WsEdit {
    pub(super) number: usize,
    /// Which of the setup's monitors (0-based, left to right).
    pub(super) monitor: usize,
    pub(super) layout: Option<LayoutKind>,
    pub(super) splits: Vec<f32>,
    pub(super) master_ratio: Option<f32>,
    pub(super) apps: Vec<AppEdit>,
}

pub(crate) struct SetupEdit {
    pub(super) id: u64,
    pub(super) name: TextEdit,
    pub(super) key: TextEdit,
    /// How many monitors it's laid out for (at least 1).
    pub(super) monitors: usize,
    pub(super) workspaces: Vec<WsEdit>,
}

impl SetupEdit {
    /// Where workspace `ws`'s entry for monitor `mon` is in `workspaces`, if it has one.
    pub(super) fn entry_index(&self, ws: usize, mon: usize) -> Option<usize> {
        self.workspaces.iter().position(|w| w.number == ws && w.monitor == mon)
    }

    /// Workspace `ws`'s entry for monitor `mon`, if it has one.
    pub(super) fn entry(&self, ws: usize, mon: usize) -> Option<&WsEdit> {
        self.workspaces.get(self.entry_index(ws, mon)?)
    }

    fn entry_mut(&mut self, ws: usize, mon: usize) -> Option<&mut WsEdit> {
        let i = self.entry_index(ws, mon)?;
        self.workspaces.get_mut(i)
    }
}

/// A monitor as the editor draws it: its tiling area on the desktop, in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Screen {
    pub(super) area: Rect,
}

/// The monitors, left to right as lattice numbers them, and which is the primary. The bar is
/// on the primary only, so its tiling area starts below it.
fn screens(cfg: &config::Config) -> (Vec<Screen>, usize) {
    let monitors = crate::platform::win::monitors();
    let primary = monitors.iter().position(|m| m.primary).unwrap_or(0);
    let screens: Vec<Screen> = monitors
        .iter()
        .map(|m| {
            let bar = if m.primary && cfg.bar.enabled { (cfg.bar.height * m.scale).round() as i32 } else { 0 };
            Screen { area: Rect::new(m.work.x, m.work.y + bar, m.work.w.max(1), (m.work.h - bar).max(1)) }
        })
        .collect();
    if screens.is_empty() { (vec![Screen { area: Rect::new(0, 0, 1920, 1040) }], 0) } else { (screens, primary) }
}

pub(crate) struct Editor {
    pub(crate) list: Vec<SetupEdit>,
    /// The setup being edited.
    pub(crate) sel: Option<usize>,
    /// The workspace shown on the canvas (1-based), and the monitor of it (0-based).
    pub(super) ws: usize,
    pub(super) mon: usize,
    /// The selected window: the editor's stand-in for the focused one.
    pub(super) tile: Option<usize>,
    pub(super) drag: Option<TileDrag>,
    /// The seam being dragged (its index in `layout::seams`), and the ratios from before, for
    /// a drag that's cancelled.
    pub(super) seam: Option<usize>,
    pub(super) seam_undo: (Vec<f32>, Option<f32>),
    pub(super) picker: ComboboxState,
    /// The "Add workspace" menu.
    pub(super) ws_picker: ComboboxState,
    /// The monitors: the canvas has the shape of the one shown.
    pub(super) screens: Vec<Screen>,
    pub(super) primary: usize,
    next_id: u64,
    pub(super) capturing: bool,
    /// The tiling dropdown, and the layout editor's help, open.
    pub(super) layout_open: bool,
    pub(super) info_open: bool,
}

impl Editor {
    pub(crate) fn load(cfg: &config::Config) -> Self {
        let (screens, primary) = screens(cfg);
        Self::load_on(cfg, screens, primary)
    }

    /// Loads the setups as on these monitors.
    pub(super) fn load_on(cfg: &config::Config, screens: Vec<Screen>, primary: usize) -> Self {
        let mut ed = Editor {
            list: Vec::new(),
            sel: None,
            ws: 1,
            mon: 0,
            tile: None,
            drag: None,
            seam: None,
            seam_undo: (Vec::new(), None),
            picker: ComboboxState::default(),
            ws_picker: ComboboxState::default(),
            screens,
            primary,
            next_id: 1,
            capturing: false,
            layout_open: false,
            info_open: false,
        };
        let list: Vec<SetupEdit> = cfg.setups.iter().map(|s| ed.from_config(s)).collect();
        ed.list = list;
        ed
    }

    pub(super) fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub(super) fn app(&mut self, exe: &str, run: &str) -> AppEdit {
        AppEdit { id: self.id(), exe: TextEdit::new(exe), run: run.to_string(), run_for: exe.to_string() }
    }

    fn from_config(&mut self, s: &Setup) -> SetupEdit {
        let count = setups::monitor_count(s);
        let workspaces = s
            .workspaces
            .iter()
            .map(|w| WsEdit {
                number: w.number,
                monitor: setups::monitor_slot(count, w.monitor, self.primary),
                layout: w.layout,
                splits: w.splits.clone(),
                master_ratio: w.master_ratio,
                apps: w.apps.iter().map(|a| self.app(&a.exe, &a.run)).collect(),
            })
            .collect();
        SetupEdit {
            id: self.id(),
            name: TextEdit::new(s.name.clone()),
            key: TextEdit::new(s.key.clone().unwrap_or_default()),
            monitors: count,
            workspaces,
        }
    }

    /// A new, empty setup, laid out for the monitors connected now.
    pub(super) fn blank(&mut self, name: String) -> SetupEdit {
        SetupEdit { id: self.id(), name: TextEdit::new(name), key: TextEdit::new(""), monitors: self.screens.len().max(1), workspaces: Vec::new() }
    }

    /// Opens setup `i`, showing its first workspace with apps (on its first monitor with apps).
    pub(super) fn select(&mut self, i: usize) {
        self.sel = Some(i);
        let first = self.list[i].workspaces.iter().find(|w| !w.apps.is_empty()).map(|w| (w.number, w.monitor));
        (self.ws, self.mon) = first.unwrap_or((1, self.home()));
        self.tile = None;
        self.drag = None;
        self.seam = None;
    }

    /// Back to the list.
    pub(super) fn close(&mut self) {
        self.sel = None;
        self.tile = None;
        self.drag = None;
        self.seam = None;
        self.picker.close();
        self.ws_picker.close();
        self.layout_open = false;
        self.info_open = false;
    }

    /// Shows workspace `n` (on its first monitor with apps), adding it (empty) if the setup
    /// doesn't use it yet.
    pub(super) fn show_ws(&mut self, n: usize) {
        self.ws = n;
        self.mon = self.setup().and_then(|s| s.workspaces.iter().find(|w| w.number == n && !w.apps.is_empty())).map_or(self.home(), |w| w.monitor);
        self.tile = None;
        self.shown_or_add();
    }

    /// Shows monitor `m` of the workspace on the canvas.
    pub(super) fn show_mon(&mut self, m: usize) {
        self.mon = m;
        self.tile = None;
    }

    /// How many monitors the open setup is laid out for.
    pub(super) fn count(&self) -> usize {
        self.setup().map_or(1, |s| s.monitors.max(1))
    }

    /// Where an entry without a monitor goes: the primary's place.
    fn home(&self) -> usize {
        setups::monitor_slot(self.count(), None, self.primary)
    }

    /// Whether monitor `m` of the setup is connected now (a one-monitor setup's always is: it's
    /// the primary).
    pub(super) fn connected(&self, m: usize) -> bool {
        self.count() <= 1 || m < self.screens.len()
    }

    /// What monitor `m` is on this desktop: "Monitor 2 · right · primary".
    pub(super) fn monitor_name(&self, m: usize) -> String {
        if self.count() <= 1 {
            return "Your primary monitor".into();
        }
        if !self.connected(m) {
            return format!("Monitor {} \u{b7} not connected", m + 1);
        }
        let side = match (self.screens.len(), m) {
            (2, 0) | (3, 0) => Some("left"),
            (2, 1) | (3, 2) => Some("right"),
            (3, 1) => Some("middle"),
            _ => None,
        };
        let mut out = format!("Monitor {}", m + 1);
        if let Some(side) = side {
            out += &format!(" \u{b7} {side}");
        }
        if m == self.primary {
            out += " \u{b7} primary";
        }
        out
    }

    /// Lays the setup out for one more monitor, right of the others, and shows it.
    pub(super) fn add_monitor(&mut self) {
        self.add_monitor_at(self.count());
    }

    /// Lays the setup out for one more monitor at `at` (0 is left of the others, `count()`
    /// right of them), moving the ones from there right, and shows it.
    pub(super) fn add_monitor_at(&mut self, at: usize) {
        let Some(setup) = self.sel.and_then(|i| self.list.get_mut(i)) else { return };
        let at = at.min(setup.monitors);
        for w in setup.workspaces.iter_mut().filter(|w| w.monitor >= at) {
            w.monitor += 1;
        }
        setup.monitors += 1;
        self.mon = at;
        self.tile = None;
    }

    /// Takes the monitor shown out of the setup, with its apps on every workspace (the ones
    /// right of it move down one); a setup keeps at least one. Returns how many apps went.
    pub(super) fn remove_monitor(&mut self) -> usize {
        let m = self.mon;
        let Some(setup) = self.sel.and_then(|i| self.list.get_mut(i)) else { return 0 };
        if setup.monitors <= 1 {
            return 0;
        }
        let gone = setup.workspaces.iter().filter(|w| w.monitor == m).map(|w| w.apps.len()).sum();
        setup.workspaces.retain(|w| w.monitor != m);
        for w in setup.workspaces.iter_mut().filter(|w| w.monitor > m) {
            w.monitor -= 1;
        }
        setup.monitors -= 1;
        self.mon = m.saturating_sub(1);
        self.tile = None;
        gone
    }

    /// Gives the shown workspace's other monitors its layout and master ratio, which lattice
    /// keeps per workspace.
    pub(super) fn sync_siblings(&mut self) {
        let (ws, mon) = (self.ws, self.mon);
        let Some(setup) = self.sel.and_then(|i| self.list.get_mut(i)) else { return };
        let Some((layout, master)) = setup.entry(ws, mon).map(|w| (w.layout, w.master_ratio)) else { return };
        for w in setup.workspaces.iter_mut().filter(|w| w.number == ws) {
            (w.layout, w.master_ratio) = (layout, master);
        }
    }

    /// Workspace numbers with a card: those with apps, and the one shown.
    pub(super) fn cards(&self) -> Vec<usize> {
        let Some(setup) = self.setup() else { return Vec::new() };
        let mut out: Vec<usize> = setup.workspaces.iter().filter(|w| !w.apps.is_empty()).map(|w| w.number).collect();
        if !out.contains(&self.ws) {
            out.push(self.ws);
        }
        // One card a workspace, however many monitors it has apps on.
        out.sort_unstable();
        out.dedup();
        out
    }

    pub(super) fn setup(&self) -> Option<&SetupEdit> {
        self.list.get(self.sel?)
    }

    /// The workspace and monitor on the canvas, if the setup has it.
    pub(super) fn shown(&self) -> Option<&WsEdit> {
        self.setup()?.entry(self.ws, self.mon)
    }

    pub(super) fn shown_mut(&mut self) -> Option<&mut WsEdit> {
        let (ws, mon) = (self.ws, self.mon);
        self.list.get_mut(self.sel?)?.entry_mut(ws, mon)
    }

    /// The workspace and monitor on the canvas, added to the setup if it isn't there yet
    /// (with the layout the workspace has on its other monitors).
    pub(super) fn shown_or_add(&mut self) -> Option<&mut WsEdit> {
        let (ws, mon) = (self.ws, self.mon);
        let setup = self.list.get_mut(self.sel?)?;
        if setup.entry_index(ws, mon).is_none() {
            let (layout, master_ratio) = setup.workspaces.iter().find(|w| w.number == ws).map_or((None, None), |w| (w.layout, w.master_ratio));
            setup.workspaces.push(WsEdit { number: ws, monitor: mon, layout, splits: Vec::new(), master_ratio, apps: Vec::new() });
            setup.workspaces.sort_by_key(|w| (w.number, w.monitor));
        }
        setup.entry_mut(ws, mon)
    }

    /// Adds an app to the shown workspace where lattice would put a new window, and selects it.
    pub(super) fn add(&mut self, exe: &str, run: &str, new_window: NewWindow) {
        let app = self.app(exe, run);
        let sel = self.tile;
        let Some(w) = self.shown_or_add() else { return };
        let at = setups::insert_index(w.apps.len(), sel, new_window);
        w.apps.insert(at, app);
        self.tile = Some(at);
    }

    pub(super) fn remove(&mut self, i: usize) {
        let Some(w) = self.shown_mut() else { return };
        if i < w.apps.len() {
            w.apps.remove(i);
            setups::forget_splits(&mut w.splits, w.apps.len());
        }
        self.tile = None;
    }

    /// Moves app `i` of the shown workspace to the end of workspace `to` on the same monitor,
    /// as move-to-workspace moves a window, and follows it there.
    pub(super) fn move_to(&mut self, i: usize, to: usize) {
        self.move_app(i, to, self.mon);
    }

    /// Moves app `i` to the end of the same workspace on monitor `mon`, and follows it there.
    pub(super) fn move_to_monitor(&mut self, i: usize, mon: usize) {
        self.move_app(i, self.ws, mon);
    }

    fn move_app(&mut self, i: usize, ws: usize, mon: usize) {
        let Some(w) = self.shown_mut() else { return };
        if i >= w.apps.len() || (ws, mon) == (w.number, w.monitor) {
            return;
        }
        let app = w.apps.remove(i);
        setups::forget_splits(&mut w.splits, w.apps.len());
        (self.ws, self.mon) = (ws, mon);
        if let Some(t) = self.shown_or_add() {
            t.apps.push(app);
            let n = t.apps.len();
            self.tile = Some(n - 1);
        }
    }
}

/// A setup as config.
pub(super) fn setup_config(e: &SetupEdit) -> Setup {
    Setup {
        name: e.name.text.trim().to_string(),
        key: None,
        monitors: Some(e.monitors.max(1)),
        workspaces: e.workspaces.iter().filter(|w| !w.apps.is_empty()).map(to_config).collect(),
    }
}

/// An entry as config (its monitor always written out, 1-based).
pub(super) fn to_config(w: &WsEdit) -> SetupWorkspace {
    SetupWorkspace {
        number: w.number,
        monitor: Some(w.monitor + 1),
        layout: w.layout,
        splits: w.splits.clone(),
        master_ratio: w.master_ratio,
        apps: w.apps.iter().map(|a| SetupApp { exe: a.exe.text.trim().to_string(), run: a.run().to_string() }).collect(),
    }
}

pub(super) fn unique_name(list: &[SetupEdit], base: &str) -> String {
    let taken = |n: &str| list.iter().any(|s| s.name.text.trim().eq_ignore_ascii_case(n));
    if !taken(base) {
        return base.to_string();
    }
    (2..).map(|i| format!("{base} {i}")).find(|n| !taken(n)).expect("a free name")
}

#[cfg(test)]
mod tests {
    use toml_edit::DocumentMut;

    use super::super::io::write;
    use super::*;

    #[test]
    fn saved_setups_round_trip_through_the_config() {
        let src = r#"
[[setup]]
name = "Dev"
key = "super+alt+1"
[[setup.workspace]]
number = 1
layout = "master"
splits = [0.6]
master_ratio = 0.65
apps = [{ exe = "Code.exe", run = "code" }, { exe = "WindowsTerminal.exe" }]
[[setup.workspace]]
number = 2
apps = [{ exe = "msedge.exe", run = "\"C:\\Program Files\\Edge\\msedge.exe\"" }]
"#;
        let cfg: config::Config = toml::from_str(src).unwrap();
        // Two monitors, the right one primary: entries without a monitor are on it.
        let mut ed = Editor::load_on(&cfg, two_screens(), 1);
        // No monitors named: a one-monitor setup (the primary).
        assert_eq!((ed.list[0].monitors, ed.list[0].workspaces[0].monitor), (1, 0));
        // An empty workspace isn't written.
        ed.select(0);
        ed.ws = 5;
        ed.shown_or_add();
        let mut doc: DocumentMut = "[general]\nworkspaces = 9\n\n[keybinds]\n\"super+q\" = \"close\"\n".parse().unwrap();
        write(&mut doc, &ed);
        let out = doc.to_string();
        let back: config::Config = toml::from_str(&out).unwrap_or_else(|e| panic!("{e}\n{out}"));
        assert_eq!(back.setups, cfg.setups, "{out}");
        assert!(!out.contains("monitor"), "the primary's entries leave it out:\n{out}");
        assert_eq!(back.keybinds.len(), 1);
        // Writing none removes the tables.
        ed.list.clear();
        write(&mut doc, &ed);
        assert!(!doc.to_string().contains("[[setup"));
    }

    fn two_screens() -> Vec<Screen> {
        vec![Screen { area: Rect::new(-1920, 0, 1920, 1080) }, Screen { area: Rect::new(0, 32, 1920, 1008) }]
    }

    #[test]
    fn apps_go_on_monitors() {
        let cfg: config::Config = toml::from_str("[[setup]]\nname = \"Dev\"\n[[setup.workspace]]\nnumber = 1\nlayout = \"master\"\napps = [{ exe = \"a.exe\" }, { exe = \"b.exe\" }]").unwrap();
        let mut ed = Editor::load_on(&cfg, two_screens(), 1);
        ed.select(0);
        assert_eq!((ed.ws, ed.mon, ed.count()), (1, 0, 1));
        assert_eq!(ed.cards(), vec![1]);
        // A second monitor, shown as it's added; then b moves onto it: a new entry there, with
        // the workspace's layout.
        ed.add_monitor();
        assert_eq!((ed.mon, ed.count()), (1, 2));
        ed.show_mon(0);
        ed.move_to_monitor(1, 1);
        assert_eq!((ed.ws, ed.mon, ed.tile), (1, 1, Some(0)));
        let right = ed.shown().unwrap();
        assert_eq!((right.apps.len(), right.layout), (1, Some(LayoutKind::Master)));
        // Changing the layout on one monitor changes it on the other.
        ed.shown_mut().unwrap().layout = Some(LayoutKind::Dwindle);
        ed.sync_siblings();
        assert!(ed.list[0].workspaces.iter().all(|w| w.layout == Some(LayoutKind::Dwindle)));
        assert_eq!(ed.cards(), vec![1], "one card for both monitors");
        let mut doc = DocumentMut::new();
        write(&mut doc, &ed);
        let back: config::Config = toml::from_str(&doc.to_string()).unwrap();
        assert_eq!(back.setups[0].monitors, Some(2));
        let w = &back.setups[0].workspaces;
        assert_eq!((w[0].monitor, w[0].apps[0].exe.as_str()), (Some(1), "a.exe"));
        assert_eq!((w[1].monitor, w[1].apps[0].exe.as_str()), (Some(2), "b.exe"));
        // A third, empty monitor is kept by the count.
        ed.add_monitor();
        let mut doc = DocumentMut::new();
        write(&mut doc, &ed);
        let back: config::Config = toml::from_str(&doc.to_string()).unwrap();
        assert_eq!(setups::monitor_count(&back.setups[0]), 3);
        // One added on the left moves the others right, apps and all.
        ed.add_monitor_at(0);
        assert_eq!((ed.count(), ed.mon), (4, 0));
        let on: Vec<(usize, &str)> = ed.list[0].workspaces.iter().flat_map(|w| w.apps.iter().map(move |a| (w.monitor, a.exe.text.as_str()))).collect();
        assert_eq!(on, vec![(1, "a.exe"), (2, "b.exe")]);
        ed.remove_monitor();
        assert_eq!((ed.count(), ed.mon), (3, 0));
        // Removing monitor 2 takes b with it and moves 3 down; one monitor stays.
        ed.show_mon(1);
        assert_eq!(ed.remove_monitor(), 1);
        assert_eq!((ed.count(), ed.mon), (2, 0));
        ed.show_mon(1);
        ed.remove_monitor();
        assert_eq!(ed.count(), 1);
        assert_eq!(ed.remove_monitor(), 0);
        assert_eq!(ed.count(), 1);
        let apps: Vec<&str> = ed.list[0].workspaces.iter().flat_map(|w| &w.apps).map(|a| a.exe.text.as_str()).collect();
        assert_eq!(apps, vec!["a.exe"]);
    }

    #[test]
    fn editing_on_fewer_monitors_keeps_the_setup() {
        // Made on two monitors across two workspaces, opened in the editor on a laptop.
        let src = r#"
            [[setup]]
            name = "Dev"
            monitors = 2
            [[setup.workspace]]
            number = 1
            monitor = 1
            splits = [0.7]
            apps = [{ exe = "a.exe" }, { exe = "b.exe" }]
            [[setup.workspace]]
            number = 1
            monitor = 2
            apps = [{ exe = "c.exe" }]
            [[setup.workspace]]
            number = 2
            monitor = 2
            apps = [{ exe = "d.exe" }]
        "#;
        let cfg: config::Config = toml::from_str(src).unwrap();
        let laptop = vec![Screen { area: Rect::new(0, 0, 1536, 824) }];
        let mut ed = Editor::load_on(&cfg, laptop, 0);
        ed.select(0);
        // Both monitors are there to edit; the second is marked, drawn the laptop's size.
        assert_eq!(ed.count(), 2);
        assert!(ed.connected(0) && !ed.connected(1));
        assert_eq!(ed.area(1), Rect::new(1536, 0, 1536, 824));
        // Workspace 2's only monitor is the missing one: that's what it opens on.
        ed.show_ws(2);
        assert_eq!(ed.mon, 1);
        // Saving it there writes back what was loaded: nothing folds into the config.
        let mut doc = DocumentMut::new();
        write(&mut doc, &ed);
        let back: config::Config = toml::from_str(&doc.to_string()).unwrap();
        assert_eq!(back.setups[0].monitors, Some(2));
        let entries: Vec<(usize, Option<usize>, usize, Vec<f32>)> =
            back.setups[0].workspaces.iter().map(|w| (w.number, w.monitor, w.apps.len(), w.splits.clone())).collect();
        assert_eq!(entries, vec![(1, Some(1), 2, vec![0.7]), (1, Some(2), 1, vec![]), (2, Some(2), 1, vec![])]);
        // A new setup made on the laptop is a one-monitor setup.
        let blank = ed.blank("New".into());
        assert_eq!(blank.monitors, 1);
    }

    #[test]
    fn missing_monitors_are_marked() {
        let cfg: config::Config =
            toml::from_str("[[setup]]\nname = \"Big\"\nmonitors = 3\n[[setup.workspace]]\nnumber = 1\nmonitor = 3\napps = [{ exe = \"a.exe\" }]").unwrap();
        let mut ed = Editor::load_on(&cfg, two_screens(), 1);
        ed.select(0);
        assert_eq!((ed.count(), ed.mon), (3, 2));
        assert!(ed.connected(1) && !ed.connected(2));
        assert_eq!(ed.monitor_name(1), "Monitor 2 \u{b7} right \u{b7} primary");
        assert_eq!(ed.monitor_name(2), "Monitor 3 \u{b7} not connected");
        // Drawn past the right edge, the primary's size.
        assert_eq!(ed.area(2), Rect::new(1920, 0, 1920, 1008));
    }

    #[test]
    fn names_stay_unique() {
        let mut ed = Editor::load(&toml::from_str("[[setup]]\nname = \"New workspace\"").unwrap());
        assert_eq!(unique_name(&ed.list, "New workspace"), "New workspace 2");
        assert_eq!(unique_name(&ed.list, "Dev"), "Dev");
        ed.list.clear();
        assert_eq!(unique_name(&ed.list, "New workspace"), "New workspace");
    }

    #[test]
    fn editor_adds_like_lattice_and_moves_apps() {
        let mut ed = Editor::load(&toml::from_str(
            "[[setup]]\nname = \"Dev\"\n[[setup.workspace]]\nnumber = 1\napps = [{ exe = \"a.exe\" }, { exe = \"b.exe\" }]",
        )
        .unwrap());
        // The list shows first; opening one shows its first workspace with apps.
        assert_eq!(ed.sel, None);
        ed.select(0);
        assert_eq!(ed.ws, 1);
        ed.tile = Some(0);
        ed.add("c.exe", "c", NewWindow::AfterFocused);
        let names = |ed: &Editor| ed.shown().unwrap().apps.iter().map(|a| a.exe.text.clone()).collect::<Vec<_>>();
        assert_eq!(names(&ed), ["a.exe", "c.exe", "b.exe"]);
        assert_eq!(ed.tile, Some(1));
        ed.move_to(1, 3);
        assert_eq!(ed.ws, 3);
        assert_eq!(names(&ed), ["c.exe"]);
        ed.ws = 1;
        assert_eq!(names(&ed), ["a.exe", "b.exe"]);
        ed.remove(0);
        assert_eq!(names(&ed), ["b.exe"]);
    }

    #[test]
    fn a_changed_program_forgets_how_the_old_one_started() {
        let mut ed = Editor::load(&toml::from_str("[[setup]]\nname = \"Dev\"\n[[setup.workspace]]\nnumber = 1\napps = [{ exe = \"Code.exe\", run = \"code\" }]").unwrap());
        let app = &mut ed.list[0].workspaces[0].apps[0];
        assert_eq!(app.run(), "code");
        app.exe = TextEdit::new("code.EXE");
        assert_eq!(app.run(), "code", "same program, other case");
        app.exe = TextEdit::new("Cursor.exe");
        assert_eq!(app.run(), "");
    }
}
