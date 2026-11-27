//! The Workspaces tab: saved workspaces (`[[setup]]`) and their layout editor.
//!
//! The editor draws a saved workspace the way lattice will tile it, from the same `layout`
//! code: drag a window onto another to swap them, drag the seam between windows to resize, and
//! an app you add drops in where a new window would (after the selected one, or at the end,
//! per `general.new_window`). Click a window to edit its program, or move or remove it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use rune::tok::{space, type_};
use rune::{Color, Cursor, Drag, El, Phase, TextEdit, TextStyle, Tx, col, el, row, text};
use rune_kit::{Button, ButtonVariant, Combobox, ComboboxOption, ComboboxState, SegmentedControl, TextInput};
use toml_edit::DocumentMut;

use super::{LAYOUTS, NEW_WINDOW, P, S, kit, section, setting_row};
use crate::config::{self, NewWindow, Setup, SetupApp, SetupWorkspace};
use crate::layout::{self, LayoutKind, Rect};
use crate::setups::{self, Defaults};

/// The canvas's width in logical pixels; its height follows the screen's shape.
const CANVAS_W: f32 = 640.0;
/// How wide the grab area of a seam is.
const SEAM: f32 = 14.0;
/// Pointer travel before a press on a window counts as a drag.
const DRAG_SLOP: f32 = 4.0;
const OTHER: &str = "__other__";

/// Tile colours, by position (Catppuccin Mocha accents, like the bar).
const PALETTE: [&str; 8] = ["#89b4fa", "#cba6f7", "#a6e3a1", "#fab387", "#94e2d5", "#f5c2e7", "#f9e2af", "#89dceb"];

pub struct AppEdit {
    pub id: u64,
    pub exe: TextEdit,
    pub run: TextEdit,
}

pub struct WsEdit {
    pub number: usize,
    pub layout: Option<LayoutKind>,
    pub splits: Vec<f32>,
    pub master_ratio: Option<f32>,
    pub apps: Vec<AppEdit>,
}

pub struct SetupEdit {
    pub id: u64,
    pub name: TextEdit,
    pub key: TextEdit,
    pub workspaces: Vec<WsEdit>,
}

/// A window being dragged on the canvas.
#[derive(Clone, Copy, Debug)]
pub struct TileDrag {
    pub from: usize,
    /// The tile's top left on the canvas when the press landed, and where in it.
    pub origin: (f32, f32),
    pub grab: (f32, f32),
    /// The pointer, on the canvas.
    pub pointer: (f32, f32),
    pub moved: bool,
}

pub struct Editor {
    pub list: Vec<SetupEdit>,
    /// The saved workspace being edited.
    pub sel: Option<usize>,
    /// The workspace shown on the canvas (1-based).
    pub ws: usize,
    /// The selected window: the editor's stand-in for the focused one.
    pub tile: Option<usize>,
    pub drag: Option<TileDrag>,
    /// The seam being dragged (its index in `layout::seams`), and the ratios from before, for
    /// a drag that's cancelled.
    pub seam: Option<usize>,
    pub seam_undo: (Vec<f32>, Option<f32>),
    pub picker: ComboboxState,
    /// The primary monitor's tiling area, in pixels: the canvas has its shape.
    pub screen: (i32, i32),
    pub next_id: u64,
    pub capturing: bool,
}

impl Editor {
    pub fn load(cfg: &config::Config) -> Self {
        let mut ed = Editor {
            list: Vec::new(),
            sel: None,
            ws: 1,
            tile: None,
            drag: None,
            seam: None,
            seam_undo: (Vec::new(), None),
            picker: ComboboxState::default(),
            screen: primary_area(cfg),
            next_id: 1,
            capturing: false,
        };
        let list: Vec<SetupEdit> = cfg.setups.iter().map(|s| ed.from_config(s)).collect();
        ed.list = list;
        if !ed.list.is_empty() {
            ed.select(0);
        }
        ed
    }

    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn app(&mut self, exe: &str, run: &str) -> AppEdit {
        AppEdit { id: self.id(), exe: TextEdit::new(exe), run: TextEdit::new(run) }
    }

    fn from_config(&mut self, s: &Setup) -> SetupEdit {
        let workspaces = s
            .workspaces
            .iter()
            .map(|w| WsEdit {
                number: w.number,
                layout: w.layout,
                splits: w.splits.clone(),
                master_ratio: w.master_ratio,
                apps: w.apps.iter().map(|a| self.app(&a.exe, &a.run)).collect(),
            })
            .collect();
        SetupEdit { id: self.id(), name: TextEdit::new(s.name.clone()), key: TextEdit::new(s.key.clone().unwrap_or_default()), workspaces }
    }

    /// Edits saved workspace `i`, showing its first workspace with apps.
    fn select(&mut self, i: usize) {
        self.sel = Some(i);
        self.ws = self.list[i].workspaces.iter().find(|w| !w.apps.is_empty()).map_or(1, |w| w.number);
        self.tile = None;
        self.drag = None;
        self.seam = None;
    }

    fn setup(&self) -> Option<&SetupEdit> {
        self.list.get(self.sel?)
    }

    /// The workspace on the canvas, if the saved workspace has it.
    fn shown(&self) -> Option<&WsEdit> {
        self.setup()?.workspaces.iter().find(|w| w.number == self.ws)
    }

    fn shown_mut(&mut self) -> Option<&mut WsEdit> {
        let ws = self.ws;
        self.list.get_mut(self.sel?)?.workspaces.iter_mut().find(|w| w.number == ws)
    }

    /// The workspace on the canvas, added to the saved workspace if it isn't there yet.
    fn shown_or_add(&mut self) -> Option<&mut WsEdit> {
        let ws = self.ws;
        let setup = self.list.get_mut(self.sel?)?;
        if !setup.workspaces.iter().any(|w| w.number == ws) {
            setup.workspaces.push(WsEdit { number: ws, layout: None, splits: Vec::new(), master_ratio: None, apps: Vec::new() });
            setup.workspaces.sort_by_key(|w| w.number);
        }
        setup.workspaces.iter_mut().find(|w| w.number == ws)
    }

    /// Adds an app to the shown workspace where lattice would put a new window, and selects it.
    fn add(&mut self, exe: &str, run: &str, new_window: NewWindow) {
        let app = self.app(exe, run);
        let sel = self.tile;
        let Some(w) = self.shown_or_add() else { return };
        let at = setups::insert_index(w.apps.len(), sel, new_window);
        w.apps.insert(at, app);
        self.tile = Some(at);
    }

    fn remove(&mut self, i: usize) {
        let Some(w) = self.shown_mut() else { return };
        if i < w.apps.len() {
            w.apps.remove(i);
            setups::forget_splits(&mut w.splits, w.apps.len());
        }
        self.tile = None;
    }

    /// Moves app `i` of the shown workspace to the end of workspace `to`, as move-to-workspace
    /// moves a window, and follows it there.
    fn move_to(&mut self, i: usize, to: usize) {
        let Some(w) = self.shown_mut() else { return };
        if i >= w.apps.len() || to == w.number {
            return;
        }
        let app = w.apps.remove(i);
        setups::forget_splits(&mut w.splits, w.apps.len());
        self.ws = to;
        if let Some(t) = self.shown_or_add() {
            t.apps.push(app);
            let n = t.apps.len();
            self.tile = Some(n - 1);
        }
    }
}

fn primary_area(cfg: &config::Config) -> (i32, i32) {
    let monitors = crate::win::monitors();
    let m = monitors.iter().find(|m| m.primary).or(monitors.first());
    match m {
        Some(m) => {
            let bar = if cfg.bar.enabled { (cfg.bar.height * m.scale).round() as i32 } else { 0 };
            (m.work.w.max(1), (m.work.h - bar).max(1))
        }
        None => (1920, 1040),
    }
}

/// The tiling settings the canvas uses: the General tab's, as they are now.
fn defaults(s: &S) -> Defaults {
    Defaults {
        layout: LayoutKind::parse(LAYOUTS[s.layout]).unwrap_or(LayoutKind::Dwindle),
        split_ratio: s.split_ratio,
        master_ratio: s.master_ratio,
        gaps_inner: s.gaps_inner.round() as i32,
        gaps_outer: s.gaps_outer.round() as i32,
        new_window: if NEW_WINDOW[s.new_window] == "end" { NewWindow::End } else { NewWindow::AfterFocused },
    }
}

fn to_config(w: &WsEdit) -> SetupWorkspace {
    SetupWorkspace {
        number: w.number,
        layout: w.layout,
        splits: w.splits.clone(),
        master_ratio: w.master_ratio,
        apps: w.apps.iter().map(|a| SetupApp { exe: a.exe.text.trim().to_string(), run: a.run.text.trim().to_string() }).collect(),
    }
}

/// The canvas: its size, and pixels on screen to points on it.
#[derive(Clone, Copy, Debug)]
struct Canvas {
    w: f32,
    h: f32,
    scale: f32,
    screen: Rect,
}

impl Canvas {
    fn new(screen: (i32, i32)) -> Self {
        let scale = CANVAS_W / screen.0 as f32;
        Canvas { w: CANVAS_W, h: (screen.1 as f32 * scale).round(), scale, screen: Rect::new(0, 0, screen.0, screen.1) }
    }
}

/// Where each app's tile is drawn on the canvas, as (x, y, w, h). Monocle stacks every window on
/// the whole screen; here they fan out a little so each one shows.
fn tile_rects(w: &SetupWorkspace, d: &Defaults, c: &Canvas) -> Vec<(f32, f32, f32, f32)> {
    let rects = setups::tiles(w, c.screen, d);
    let kind = setups::workspace_layout(w, d.layout);
    let n = rects.len();
    rects
        .into_iter()
        .enumerate()
        .map(|(i, r)| {
            let (x, y, rw, rh) = (r.x as f32 * c.scale, r.y as f32 * c.scale, r.w as f32 * c.scale, r.h as f32 * c.scale);
            if kind == LayoutKind::Monocle && n > 1 {
                let step = 18.0;
                let spread = step * (n - 1) as f32;
                (x + step * i as f32, y + step * i as f32, rw - spread, rh - spread)
            } else {
                (x, y, rw, rh)
            }
        })
        .collect()
}

fn tile_under(rects: &[(f32, f32, f32, f32)], p: (f32, f32), skip: usize) -> Option<usize> {
    rects
        .iter()
        .enumerate()
        .rev()
        .find(|(i, r)| *i != skip && p.0 >= r.0 && p.0 < r.0 + r.2 && p.1 >= r.1 && p.1 < r.1 + r.3)
        .map(|(i, _)| i)
}

fn seams(w: &SetupWorkspace, d: &Defaults, c: &Canvas) -> Vec<layout::Seam> {
    let kind = setups::workspace_layout(w, d.layout);
    layout::seams(kind, c.screen, w.apps.len(), &setups::params(w, d, 1.0))
}

// ---------------------------------------------------------------------------------------------
// Saving

/// Problems that keep the saved workspaces from being saved.
pub fn validate(s: &S) -> Result<(), String> {
    let workspaces = s.workspaces.round() as usize;
    let mut names = std::collections::HashSet::new();
    let mut keys: std::collections::HashMap<(u8, u32), String> = std::collections::HashMap::new();
    for b in &s.binds {
        if let Ok(c) = config::parse_chord(b.chord.text.trim()) {
            keys.insert(c, format!("the keybind for \"{}\"", b.command.text.trim()));
        }
    }
    for setup in &s.ed.list {
        let name = setup.name.text.trim();
        if name.is_empty() {
            return Err("Every saved workspace needs a name".into());
        }
        if !names.insert(name.to_lowercase()) {
            return Err(format!("Two saved workspaces are called \"{name}\""));
        }
        let key = setup.key.text.trim();
        if !key.is_empty() {
            let chord = config::parse_chord(key).map_err(|e| format!("\"{name}\" shortcut: {e}"))?;
            if let Some(other) = keys.insert(chord, format!("\"{name}\"")) {
                return Err(format!("\"{name}\" shortcut {key} is already {other}"));
            }
        }
        for w in &setup.workspaces {
            if w.apps.is_empty() {
                continue;
            }
            if w.number > workspaces {
                return Err(format!("\"{name}\" uses workspace {}, but there are only {workspaces}", w.number));
            }
            for a in &w.apps {
                let exe = a.exe.text.trim();
                if exe.is_empty() {
                    return Err(format!("\"{name}\": an app on workspace {} has no program", w.number));
                }
                if super::bad_exe(exe) {
                    return Err(format!("\"{name}\": use the program's file name, like Code.exe (not \"{exe}\")"));
                }
            }
        }
    }
    Ok(())
}

/// Writes the saved workspaces as `[[setup]]` tables, replacing the ones there.
pub fn write(doc: &mut DocumentMut, list: &[SetupEdit]) {
    doc.remove("setup");
    doc.remove("setups");
    if list.is_empty() {
        return;
    }
    let mut arr = toml_edit::ArrayOfTables::new();
    for (i, s) in list.iter().enumerate() {
        let mut t = toml_edit::Table::new();
        if i == 0 {
            t.decor_mut().set_prefix("\n# Saved workspaces: apps, their workspaces and layout. Restore one with its key,\n# `lattice msg restore-workspace NAME`, or \"workspaces\" in the launcher. The settings\n# window's Workspaces tab edits them (and rewrites this part of the file).\n");
        }
        t["name"] = toml_edit::value(s.name.text.trim());
        let key = s.key.text.trim();
        if !key.is_empty() {
            t["key"] = toml_edit::value(key);
        }
        let mut wss = toml_edit::ArrayOfTables::new();
        for w in s.workspaces.iter().filter(|w| !w.apps.is_empty()) {
            let mut wt = toml_edit::Table::new();
            wt["number"] = toml_edit::value(w.number as i64);
            if let Some(l) = w.layout {
                wt["layout"] = toml_edit::value(l.name());
            }
            if !w.splits.is_empty() {
                let splits: toml_edit::Array = w.splits.iter().take(w.apps.len().saturating_sub(1)).map(|r| super::round2(*r)).collect();
                wt["splits"] = toml_edit::value(splits);
            }
            if let Some(m) = w.master_ratio {
                wt["master_ratio"] = toml_edit::value(super::round2(m));
            }
            let mut apps = toml_edit::Array::new();
            for a in &w.apps {
                let mut it = toml_edit::InlineTable::new();
                it.insert("exe", a.exe.text.trim().into());
                let run = a.run.text.trim();
                if !run.is_empty() {
                    it.insert("run", run.into());
                }
                apps.push(it);
            }
            // One app a line.
            for v in apps.iter_mut() {
                v.decor_mut().set_prefix("\n  ");
            }
            apps.set_trailing("\n");
            apps.set_trailing_comma(true);
            wt["apps"] = toml_edit::value(apps);
            wss.push(wt);
        }
        if !wss.is_empty() {
            t["workspace"] = toml_edit::Item::ArrayOfTables(wss);
        }
        arr.push(t);
    }
    doc["setup"] = toml_edit::Item::ArrayOfTables(arr);
}

// ---------------------------------------------------------------------------------------------
// Talking to lattice: restoring, and capturing what's on screen

static CAPTURED: Mutex<Option<Result<Vec<CapturedWs>, String>>> = Mutex::new(None);
static RESTORED: Mutex<Option<Result<String, String>>> = Mutex::new(None);
pub static GEN: AtomicU64 = AtomicU64::new(0);
static WAKER: OnceLock<rune::Waker> = OnceLock::new();

pub fn set_waker(w: rune::Waker) {
    let _ = WAKER.set(w);
}

fn wake() {
    GEN.fetch_add(1, Ordering::SeqCst);
    if let Some(w) = WAKER.get() {
        w();
    }
}

struct CapturedWs {
    number: usize,
    layout: Option<LayoutKind>,
    apps: Vec<(String, String)>,
}

/// Asks the running lattice what's open where, off the UI thread (see `poll`).
fn capture() {
    std::thread::spawn(|| {
        let result = crate::ipc::request("query", "state").and_then(|v| parse_capture(&v));
        *CAPTURED.lock().unwrap() = Some(result);
        wake();
    });
}

fn parse_capture(v: &serde_json::Value) -> Result<Vec<CapturedWs>, String> {
    let mut out: Vec<CapturedWs> = Vec::new();
    for w in v["windows"].as_array().ok_or("lattice sent no windows")? {
        if w["floating"].as_bool() == Some(true) || w["minimized"].as_bool() == Some(true) {
            continue;
        }
        let (Some(number), Some(exe)) = (w["workspace"].as_u64(), w["exe"].as_str()) else { continue };
        if exe.is_empty() || exe.eq_ignore_ascii_case("ApplicationFrameHost.exe") {
            continue;
        }
        let path = w["hwnd"].as_i64().map(|h| crate::win::exe_path(h as isize)).unwrap_or_default();
        let run = if path.is_empty() { String::new() } else { setups::run_for_path(&path) };
        let number = number as usize;
        if out.last().is_none_or(|c| c.number != number) {
            let layout = v["workspaces"]
                .as_array()
                .and_then(|l| l.iter().find(|x| x["index"].as_u64() == Some(number as u64)))
                .and_then(|x| x["layout"].as_str())
                .and_then(LayoutKind::parse);
            out.push(CapturedWs { number, layout, apps: Vec::new() });
        }
        out.last_mut().expect("just pushed").apps.push((exe.to_string(), run));
    }
    if out.is_empty() {
        return Err("No tiled windows to save".into());
    }
    Ok(out)
}

/// Saves, then asks the running lattice to restore `name`.
fn restore(s: &mut S, name: String) {
    match super::save(s) {
        Ok(_) => {
            s.status = format!("Restoring \u{201c}{name}\u{201d}…");
            s.status_ok = true;
            std::thread::spawn(move || {
                let line = format!("restore-workspace {name}");
                let r = crate::ipc::request("msg", &line).map(|_| name);
                *RESTORED.lock().unwrap() = Some(r);
                wake();
            });
        }
        Err(e) => {
            s.status = e;
            s.status_ok = false;
        }
    }
}

/// Takes what came back from lattice; true when something changed.
pub fn poll(s: &mut S) -> bool {
    let mut changed = false;
    if let Some(r) = CAPTURED.lock().unwrap().take() {
        s.ed.capturing = false;
        changed = true;
        match r {
            Ok(wss) => {
                let name = unique_name(&s.ed.list, "Current layout");
                let id = s.ed.id();
                let mut setup = SetupEdit { id, name: TextEdit::new(name.clone()), key: TextEdit::new(""), workspaces: Vec::new() };
                for c in wss {
                    let apps = c.apps.iter().map(|(exe, run)| s.ed.app(exe, run)).collect();
                    setup.workspaces.push(WsEdit { number: c.number, layout: c.layout, splits: Vec::new(), master_ratio: None, apps });
                }
                s.ed.list.push(setup);
                let i = s.ed.list.len() - 1;
                s.ed.select(i);
                s.status = format!("Saved what's open as \u{201c}{name}\u{201d}; Save to keep it.");
                s.status_ok = true;
            }
            Err(e) => {
                s.status = format!("Couldn't read the open windows: {e}");
                s.status_ok = false;
            }
        }
    }
    if let Some(r) = RESTORED.lock().unwrap().take() {
        changed = true;
        match r {
            Ok(name) => {
                s.status = format!("Restored \u{201c}{name}\u{201d}.");
                s.status_ok = true;
            }
            Err(e) => {
                s.status = format!("Couldn't restore: {e}");
                s.status_ok = false;
            }
        }
    }
    changed
}

fn unique_name(list: &[SetupEdit], base: &str) -> String {
    let taken = |n: &str| list.iter().any(|s| s.name.text.trim().eq_ignore_ascii_case(n));
    if !taken(base) {
        return base.to_string();
    }
    (2..).map(|i| format!("{base} {i}")).find(|n| !taken(n)).expect("a free name")
}

// ---------------------------------------------------------------------------------------------
// View

fn color(i: usize) -> Color {
    crate::bar::color(PALETTE[i % PALETTE.len()], Color::WHITE)
}

fn small(t: &str, c: Color) -> El {
    text(t.to_string()).font(TextStyle::new(11.5, 400.0)).color(c)
}

pub fn tab(s: &S, p: &P, overlays: &mut Vec<El>) -> Vec<El> {
    let mut out = vec![
        col().key("setups-help").gap(4.0).mx(space::S3).w_full().child(
            small(
                "Save a set of apps with their workspaces and layout. Restoring one starts what isn't running and moves everything into place: with its shortcut, by typing \u{201c}workspaces\u{201d} in the launcher, or `lattice msg restore-workspace NAME`.",
                p.t.text_secondary,
            )
            .w_full()
            .wrap(),
        ),
        list_section(s, p),
    ];
    if let Some(i) = s.ed.sel.filter(|i| *i < s.ed.list.len()) {
        out.push(details_section(s, p, i));
        out.push(layout_section(s, p, overlays));
        if let Some(t) = s.ed.tile.filter(|t| s.ed.shown().is_some_and(|w| *t < w.apps.len())) {
            out.push(app_section(s, p, t));
        }
    }
    out
}

fn list_section(s: &S, p: &P) -> El {
    let k = kit(p);
    let mut rows = Vec::new();
    for (i, setup) in s.ed.list.iter().enumerate() {
        let on = s.ed.sel == Some(i);
        let preview = SetupPreview::of(setup);
        let name = setup.name.text.trim();
        let title = text(if name.is_empty() { "Untitled".to_string() } else { name.to_string() })
            .font(TextStyle::new(13.5, if on { 650.0 } else { 550.0 }))
            .color(p.t.text);
        let detail = small(&setups::summary(&preview.setup), p.t.text_secondary).truncate();
        let key = setup.key.text.trim();
        let mut right = row().gap(space::S2).center_y();
        if !key.is_empty() {
            right = right.child(key_caps(key, p));
        }
        let restore_name = name.to_string();
        right = right.child(
            Button::new("Restore")
                .variant(ButtonVariant::Outline)
                .disabled(name.is_empty())
                .on_press(move |s: &mut S| restore(s, restore_name.clone()))
                .build(&k),
        );
        rows.push(
            row()
                .key(format!("setup-{}", setup.id))
                .hit()
                .cursor(Cursor::Pointer)
                .px(space::S3)
                .py(space::S2)
                .gap(space::S3)
                .center_y()
                .radius(8.0)
                .fill(if on { p.accent.alpha(0.12) } else { Color::TRANSPARENT })
                .on_press(move |s: &mut S| s.ed.select(i))
                .child(mini_screens(&preview, s, p))
                .child(col().gap(2.0).grow(1.0).shrink().child(title).child(detail))
                .child(right),
        );
    }
    if rows.is_empty() {
        rows.push(
            col()
                .key("setups-empty")
                .px(space::S3)
                .py(16.0)
                .gap(4.0)
                .child(text("No saved workspaces yet").font(type_::BODY).color(p.t.text))
                .child(small("Make one from scratch, or save the windows you have open now.", p.t.text_secondary)),
        );
    }
    let new = Button::new("New saved workspace").on_press(|s: &mut S| {
        let name = unique_name(&s.ed.list, "New workspace");
        let id = s.ed.id();
        s.ed.list.push(SetupEdit { id, name: TextEdit::new(name), key: TextEdit::new(""), workspaces: Vec::new() });
        let i = s.ed.list.len() - 1;
        s.ed.select(i);
    });
    let capture_label = if s.ed.capturing { "Reading windows…" } else { "Save what's open now" };
    let cap = Button::new(capture_label).variant(ButtonVariant::Outline).disabled(s.ed.capturing).on_press(|s: &mut S| {
        s.ed.capturing = true;
        capture();
    });
    rows.push(row().key("setups-add").px(space::S3).py(space::S2).gap(space::S2).child(new.build(&k)).child(cap.build(&k)));
    section(p, "setups-list", &format!("Saved workspaces ({})", s.ed.list.len()), rows)
}

/// A saved workspace in the form `setups` previews take.
struct SetupPreview {
    setup: Setup,
}

impl SetupPreview {
    fn of(e: &SetupEdit) -> Self {
        SetupPreview {
            setup: Setup {
                name: e.name.text.trim().to_string(),
                key: None,
                workspaces: e.workspaces.iter().filter(|w| !w.apps.is_empty()).map(to_config).collect(),
            },
        }
    }
}

/// Up to three little screens with each workspace's tiles.
fn mini_screens(pv: &SetupPreview, s: &S, p: &P) -> El {
    let d = defaults(s);
    let c = Canvas::new(s.ed.screen);
    let (mw, mh) = (40.0, (40.0 * c.h / c.w).round());
    let mut r = row().gap(4.0).center_y().w(3.0 * 40.0 + 8.0);
    for w in pv.setup.workspaces.iter().take(3) {
        let mut screen = el().size(mw, mh).radius(4.0).fill(p.t.bg).border(p.hair, p.t.card_border);
        for (i, t) in tile_rects(w, &d, &c).into_iter().enumerate() {
            let f = mw / c.w;
            screen = screen.child(el().absolute(t.0 * f, t.1 * f).size((t.2 * f).max(2.0), (t.3 * f).max(2.0)).radius(1.5).fill(color(i).alpha(0.7)));
        }
        r = r.child(screen);
    }
    if pv.setup.workspaces.is_empty() {
        r = r.child(el().size(mw, mh).radius(4.0).border(p.hair, p.t.card_border));
    }
    r
}

fn key_caps(chord: &str, p: &P) -> El {
    chord.split('+').map(str::trim).filter(|k| !k.is_empty()).fold(row().gap(3.0).center_y(), |r, k| {
        let label = match k.to_ascii_lowercase().as_str() {
            "super" | "win" | "mod4" | "meta" => "Win".to_string(),
            "ctrl" | "control" => "Ctrl".to_string(),
            "alt" => "Alt".to_string(),
            "shift" => "Shift".to_string(),
            other if other.len() == 1 => other.to_uppercase(),
            other => other.to_string(),
        };
        r.child(
            row()
                .h(20.0)
                .min_w(20.0)
                .px(6.0)
                .radius(5.0)
                .center()
                .fill(p.t.bg)
                .border(p.hair, p.t.card_border)
                .child(text(label).font(TextStyle::new(11.0, 600.0)).color(p.t.text_secondary)),
        )
    })
}

fn details_section(s: &S, p: &P, i: usize) -> El {
    let k = kit(p);
    let setup = &s.ed.list[i];
    let name = TextInput::new(&setup.name)
        .label("Name")
        .placeholder("Dev")
        .width(Some(240.0))
        .invalid(setup.name.text.trim().is_empty())
        .on_edit(move |s: &mut S| &mut s.ed.list[i].name)
        .build(&k);
    let key_text = setup.key.text.trim();
    let key_err = (!key_text.is_empty()).then(|| config::parse_chord(key_text).err()).flatten();
    let key = TextInput::new(&setup.key)
        .label("Shortcut")
        .placeholder("super+alt+1")
        .width(Some(240.0))
        .invalid(key_err.is_some())
        .on_edit(move |s: &mut S| &mut s.ed.list[i].key)
        .build(&k);
    let hint = key_err.unwrap_or_else(|| "Restores it from anywhere; empty for none".into());
    let delete = Button::new("Delete").variant(ButtonVariant::Danger).on_press(move |s: &mut S| {
        if i < s.ed.list.len() {
            s.ed.list.remove(i);
            s.ed.sel = None;
            if !s.ed.list.is_empty() {
                s.ed.select(i.min(s.ed.list.len() - 1));
            }
        }
    });
    section(p, "setup-details", "Saved workspace", vec![
        setting_row(p, "setup-name", "Name", Some("Type it in the launcher's picker to find it"), name),
        setting_row(p, "setup-key", "Shortcut", Some(&hint), key),
        setting_row(p, "setup-delete", "Delete this saved workspace", Some("Its apps stay open; only the saved layout goes"), delete.build(&k)),
    ])
}

fn layout_section(s: &S, p: &P, overlays: &mut Vec<El>) -> El {
    let k = kit(p);
    let d = defaults(s);
    let workspaces = (s.workspaces.round() as usize).max(1);
    let setup = s.ed.setup().expect("a saved workspace is selected");
    // Workspace picker: "2 · 3" for a workspace holding three apps.
    let labels: Vec<String> = (1..=workspaces)
        .map(|n| match setup.workspaces.iter().find(|w| w.number == n).map_or(0, |w| w.apps.len()) {
            0 => n.to_string(),
            c => format!("{n} \u{b7} {c}"),
        })
        .collect();
    let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let ws_pick = SegmentedControl::new(&label_refs, Some(s.ed.ws.clamp(1, workspaces) - 1))
        .label("Workspace")
        .fit()
        .on_pick(|s: &mut S, i, _| {
            s.ed.ws = i + 1;
            s.ed.tile = None;
        })
        .build(&k);
    let shown = s.ed.shown().map(to_config).unwrap_or(SetupWorkspace {
        number: s.ed.ws,
        layout: None,
        splits: Vec::new(),
        master_ratio: None,
        apps: Vec::new(),
    });
    let kind = setups::workspace_layout(&shown, d.layout);
    let layout_pick = SegmentedControl::new(&LAYOUTS, Some(LAYOUTS.iter().position(|l| *l == kind.name()).unwrap_or(0)))
        .label("Layout")
        .fit()
        .on_pick(|s: &mut S, i, _| {
            if let Some(w) = s.ed.shown_or_add() {
                w.layout = LayoutKind::parse(LAYOUTS[i]);
            }
        })
        .build(&k);

    let canvas = canvas(s, p, &shown, &d);
    let help = if shown.apps.is_empty() {
        "Add apps below. Each one tiles in where a new window would."
    } else {
        match kind {
            LayoutKind::Monocle => "Monocle: every window fills the screen. Drag one onto another to reorder.",
            _ => "Drag a window onto another to swap them. Drag the gap between windows to resize.",
        }
    };
    let add = add_picker(s, p, overlays);
    let hint = match d.new_window {
        NewWindow::AfterFocused => "Goes after the selected window, as new windows do",
        NewWindow::End => "Goes at the end, as new windows do",
    };
    section(p, "setup-layout", "Layout", vec![
        setting_row(p, "setup-ws", "Workspace", None, ws_pick),
        setting_row(p, "setup-kind", "Tiling", None, layout_pick),
        col().key("canvas-wrap").px(space::S3).py(space::S3).gap(space::S2).items_center().child(canvas).child(small(help, p.t.text_secondary)),
        setting_row(p, "setup-add", "Add an app", Some(hint), add),
    ])
}

fn add_picker(s: &S, p: &P, overlays: &mut Vec<El>) -> El {
    let k = kit(p);
    let mut options = Vec::new();
    if s.ed.picker.is_open() {
        for (exe, path) in running_apps() {
            let mut o = ComboboxOption::new(path.clone(), setups::app_label(&exe));
            o.detail = exe;
            options.push(o);
        }
    }
    let mut other = ComboboxOption::new(OTHER, "Other program…");
    other.detail = "type it in".into();
    options.push(other);
    Combobox::new("setup-add-app", &s.ed.picker, &options, |s: &mut S| &mut s.ed.picker)
        .label("Add an app")
        .placeholder("Choose an app…")
        .search_placeholder("Search open apps…")
        .width(280.0)
        .placement(rune_kit::Placement::TopEnd)
        .on_choose(|s: &mut S, chosen: &str, _tx: &mut Tx| {
            s.ed.picker.close();
            let nw = defaults(s).new_window;
            if chosen == OTHER {
                s.ed.add("", "", nw);
            } else {
                let exe = std::path::Path::new(chosen).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                s.ed.add(&exe, &setups::run_for_path(chosen), nw);
            }
        })
        .build(&k, overlays)
}

/// Programs with a window lattice would tile, by full path (one entry a program).
fn running_apps() -> Vec<(String, String)> {
    if SAMPLE.load(Ordering::Relaxed) {
        return ["Discord.exe", "figma.exe", "msedge.exe", "Notion.exe", "slack.exe", "Spotify.exe"]
            .iter()
            .map(|e| (e.to_string(), format!(r"C:\Program Files\{}\{e}", setups::app_label(e))))
            .collect();
    }
    let mut out: Vec<(String, String)> = Vec::new();
    for h in crate::win::enum_windows() {
        let Some(c) = crate::win::candidate(h) else { continue };
        if c.exe.eq_ignore_ascii_case("lattice.exe") || c.exe.eq_ignore_ascii_case("ApplicationFrameHost.exe") {
            continue;
        }
        let path = crate::win::exe_path(h);
        if path.is_empty() || out.iter().any(|(_, p)| p.eq_ignore_ascii_case(&path)) {
            continue;
        }
        out.push((c.exe, path));
    }
    out.sort_by_key(|(exe, _)| exe.to_lowercase());
    out
}

/// The screen, its tiles and the seams between them.
fn canvas(s: &S, p: &P, w: &SetupWorkspace, d: &Defaults) -> El {
    let c = Canvas::new(s.ed.screen);
    let rects = tile_rects(w, d, &c);
    let drag = s.ed.drag.filter(|dr| dr.moved && dr.from < rects.len());
    let target = drag.and_then(|dr| tile_under(&rects, dr.pointer, dr.from));
    let mut screen = el()
        .key(format!("canvas-{}", s.ed.ws))
        .size(c.w, c.h)
        .radius(12.0)
        .fill(crate::bar::color("#11111b", Color::BLACK))
        .border(p.hair * 2.0, p.t.card_border)
        .clip_rounded(12.0);
    if w.apps.is_empty() {
        screen = screen.child(
            col()
                .absolute(0.0, 0.0)
                .size(c.w, c.h)
                .center()
                .gap(6.0)
                .child(text(format!("Workspace {} is empty", w.number)).font(TextStyle::new(15.0, 600.0)).color(p.t.text_secondary))
                .child(small("Add an app below to place it here", p.t.text_secondary.alpha(0.7))),
        );
    }
    let names: Vec<String> = w.apps.iter().map(|a| a.exe.clone()).collect();
    for (i, r) in rects.iter().enumerate() {
        let dragged = drag.is_some_and(|dr| dr.from == i);
        let state = if dragged {
            TileLook::Placeholder
        } else if target == Some(i) {
            TileLook::Target
        } else if s.ed.tile == Some(i) {
            TileLook::Selected
        } else {
            TileLook::Normal
        };
        let origin = (r.0, r.1);
        let tile = tile_el(&names[i], i, r.2, r.3, state, p)
            .key(format!("tile-{}", s.ed.shown().map_or(i as u64, |ws| ws.apps.get(i).map_or(i as u64, |a| a.id))))
            .absolute(r.0, r.1)
            .hit()
            .cursor(if dragged { Cursor::Grabbing } else { Cursor::Grab })
            .label(setups::app_label(&names[i]))
            .on_drag(move |s: &mut S, d: &mut Drag, _tx: &mut Tx| tile_drag(s, i, origin, d));
        screen = screen.child(tile);
    }
    // Seams: drawn over the tiles, grabbed anywhere along their length.
    if drag.is_none() {
        for (k, seam) in seams(w, d, &c).into_iter().enumerate() {
            let pos = seam.pos() * c.scale;
            let reg = seam.region;
            let (x, y, sw, sh) = if seam.vertical {
                (pos - SEAM / 2.0, reg.y as f32 * c.scale, SEAM, reg.h as f32 * c.scale)
            } else {
                (reg.x as f32 * c.scale, pos - SEAM / 2.0, reg.w as f32 * c.scale, SEAM)
            };
            let active = s.ed.seam == Some(k);
            let grip_len = 34.0_f32.min(if seam.vertical { sh } else { sw } * 0.5);
            let grip = if seam.vertical { el().size(4.0, grip_len) } else { el().size(grip_len, 4.0) }
                .radius(2.0)
                .fill(if active { p.accent } else { Color::WHITE.alpha(0.55) });
            let mut handle = el()
                .key(format!("seam-{k}"))
                .absolute(x, y)
                .size(sw, sh)
                .hit()
                .center()
                .cursor(if seam.vertical { Cursor::ResizeEw } else { Cursor::ResizeNs })
                .label("Resize")
                .on_drag(move |s: &mut S, d: &mut Drag, _tx: &mut Tx| seam_drag(s, k, (x, y), d))
                .child(grip);
            if active {
                handle = handle.fill(p.accent.alpha(0.18)).radius(4.0);
            }
            screen = screen.child(handle);
            if active {
                let pct = format!("{:.0}%", seam.ratio * 100.0);
                let (lx, ly) = if seam.vertical { (pos + 8.0, y + sh / 2.0 - 10.0) } else { (x + sw / 2.0 - 18.0, pos + 8.0) };
                screen = screen.child(
                    row()
                        .absolute(lx, ly)
                        .h(20.0)
                        .px(6.0)
                        .radius(5.0)
                        .center()
                        .fill(p.accent)
                        .child(text(pct).font(TextStyle::new(11.0, 700.0)).color(Color::BLACK)),
                );
            }
        }
    }
    // The window being dragged follows the pointer, over everything.
    if let Some(dr) = drag {
        let r = rects[dr.from];
        let (gx, gy) = (dr.pointer.0 - dr.grab.0, dr.pointer.1 - dr.grab.1);
        screen = screen.child(tile_el(&names[dr.from], dr.from, r.2, r.3, TileLook::Ghost, p).absolute(gx, gy).shadow(10.0, 28.0, Color::BLACK.alpha(0.5)));
    }
    screen
}

#[derive(Clone, Copy, PartialEq)]
enum TileLook {
    Normal,
    Selected,
    /// Where the dragged window will land.
    Target,
    /// The dragged window's empty spot.
    Placeholder,
    /// The dragged window itself.
    Ghost,
}

fn tile_el(exe: &str, i: usize, w: f32, h: f32, look: TileLook, p: &P) -> El {
    let c = color(i);
    let name = setups::app_label(exe);
    let pad = 4.0;
    let (fill, border, bw) = match look {
        TileLook::Normal => (c.alpha(0.16), c.alpha(0.5), 1.0),
        TileLook::Selected => (c.alpha(0.28), c, 2.0),
        TileLook::Target => (p.accent.alpha(0.3), p.accent, 2.5),
        TileLook::Placeholder => (Color::WHITE.alpha(0.03), c.alpha(0.35), 1.0),
        // Opaque, so the window under it doesn't show through.
        TileLook::Ghost => (crate::bar::color("#1e1e2e", Color::BLACK).mix(c, 0.4), c, 2.0),
    };
    let mut inner = col().w_full().h_full().center().gap(4.0).px(6.0);
    if look != TileLook::Placeholder {
        let initial = name.chars().next().map(|ch| ch.to_uppercase().to_string()).unwrap_or_default();
        if h > 64.0 && w > 50.0 {
            inner = inner.child(
                el().size(30.0, 30.0).radius(9.0).fill(c).center().child(text(initial).font(TextStyle::new(15.0, 750.0)).color(crate::bar::color("#11111b", Color::BLACK))),
            );
        }
        let title = if name.is_empty() || exe.trim().is_empty() { "New app".to_string() } else { name };
        inner = inner.child(text(title).font(TextStyle::new(if w > 120.0 { 14.0 } else { 12.0 }, 650.0)).color(Color::WHITE.alpha(0.95)).truncate());
        if look == TileLook::Target {
            inner = inner.child(text("Swap").font(TextStyle::new(11.0, 700.0)).color(p.accent));
        } else if h > 90.0 && !exe.trim().is_empty() {
            inner = inner.child(text(exe.trim().to_string()).font(TextStyle::new(11.0, 450.0)).color(Color::WHITE.alpha(0.55)).truncate());
        }
    }
    el().size((w - 2.0 * pad).max(4.0), (h - 2.0 * pad).max(4.0))
        .ml(pad)
        .mt(pad)
        .radius(9.0)
        .fill(fill)
        .border(bw, border)
        .child(inner)
}

fn tile_drag(s: &mut S, i: usize, origin: (f32, f32), d: &mut Drag) {
    match d.phase {
        Phase::Down => {
            s.ed.tile = Some(i);
            s.ed.drag = Some(TileDrag {
                from: i,
                origin,
                grab: (d.start_x, d.start_y),
                pointer: (origin.0 + d.start_x, origin.1 + d.start_y),
                moved: false,
            });
        }
        Phase::Move => {
            if let Some(dr) = s.ed.drag.as_mut() {
                dr.pointer = (dr.origin.0 + dr.grab.0 + d.dx, dr.origin.1 + dr.grab.1 + d.dy);
                dr.moved |= d.dx.abs() + d.dy.abs() > DRAG_SLOP;
            }
        }
        Phase::Up => {
            let Some(dr) = s.ed.drag.take() else { return };
            if d.cancelled() || !dr.moved {
                return;
            }
            let dflt = defaults(s);
            let c = Canvas::new(s.ed.screen);
            let Some(w) = s.ed.shown().map(to_config) else { return };
            let rects = tile_rects(&w, &dflt, &c);
            if let Some(t) = tile_under(&rects, dr.pointer, dr.from)
                && let Some(w) = s.ed.shown_mut()
                && t < w.apps.len()
                && dr.from < w.apps.len()
            {
                w.apps.swap(dr.from, t);
                s.ed.tile = Some(t);
            }
        }
    }
}

fn seam_drag(s: &mut S, k: usize, handle: (f32, f32), d: &mut Drag) {
    let dflt = defaults(s);
    let c = Canvas::new(s.ed.screen);
    let Some(w) = s.ed.shown().map(to_config) else { return };
    let Some(seam) = seams(&w, &dflt, &c).into_iter().nth(k) else { return };
    match d.phase {
        Phase::Down => {
            s.ed.seam = Some(k);
            s.ed.seam_undo = (w.splits.clone(), w.master_ratio);
            // The handle moves with the seam: keep where it was when the press landed.
            d.data = if seam.vertical { handle.0 } else { handle.1 };
            return;
        }
        Phase::Up => {
            s.ed.seam = None;
            if d.cancelled() {
                let (splits, master) = std::mem::take(&mut s.ed.seam_undo);
                if let Some(w) = s.ed.shown_mut() {
                    (w.splits, w.master_ratio) = (splits, master);
                }
                return;
            }
        }
        Phase::Move => {}
    }
    let along = if seam.vertical { d.data + d.start_x + d.dx } else { d.data + d.start_y + d.dy };
    let ratio = seam.ratio_at(along / c.scale);
    if let Some(w) = s.ed.shown_mut() {
        setups::set_ratio(&mut w.splits, &mut w.master_ratio, seam.target, ratio, dflt.split_ratio);
    }
}

fn app_section(s: &S, p: &P, t: usize) -> El {
    let k = kit(p);
    let ws = s.ed.ws;
    let workspaces = (s.workspaces.round() as usize).max(1);
    let (si, wi) = {
        let si = s.ed.sel.expect("selected");
        let wi = s.ed.list[si].workspaces.iter().position(|w| w.number == ws).expect("shown workspace");
        (si, wi)
    };
    let app = &s.ed.list[si].workspaces[wi].apps[t];
    let exe = TextInput::new(&app.exe)
        .label("Program")
        .placeholder("Code.exe")
        .width(Some(240.0))
        .invalid(app.exe.text.trim().is_empty() || super::bad_exe(&app.exe.text))
        .on_edit(move |s: &mut S| &mut s.ed.list[si].workspaces[wi].apps[t].exe)
        .build(&k);
    let run = TextInput::new(&app.run)
        .label("Start with")
        .placeholder(if app.exe.text.trim().is_empty() { "code" } else { app.exe.text.trim() })
        .width(Some(320.0))
        .on_edit(move |s: &mut S| &mut s.ed.list[si].workspaces[wi].apps[t].run)
        .build(&k);
    let step = |label: &str, to: usize, disabled: bool| {
        Button::new(label).variant(ButtonVariant::Ghost).disabled(disabled).on_press(move |s: &mut S| s.ed.move_to(t, to)).build(&k)
    };
    let mover = row()
        .gap(2.0)
        .center_y()
        .child(step("-", ws.saturating_sub(1).max(1), ws <= 1))
        .child(text(ws.to_string()).w(20.0).align(rune::Align::Center).font(type_::BODY).color(p.t.text))
        .child(step("+", (ws + 1).min(workspaces), ws >= workspaces));
    let remove = Button::new("Remove").variant(ButtonVariant::GhostMuted).on_press(move |s: &mut S| s.ed.remove(t)).build(&k);
    let label = setups::app_label(&app.exe.text);
    section(p, "setup-app", &format!("Selected: {}", if app.exe.text.trim().is_empty() { "new app" } else { &label }), vec![
        setting_row(p, "app-exe", "Program", Some("Its file name: windows are matched by it"), exe),
        setting_row(p, "app-run", "Start with", Some("Command line, when it isn't running"), run),
        setting_row(p, "app-ws", "Workspace", Some("Moves it to the end of that workspace"), mover),
        setting_row(p, "app-remove", "Remove from this saved workspace", None, remove),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_groups_windows_by_workspace() {
        let v = serde_json::json!({
            "workspaces": [{"index": 1, "layout": "master"}, {"index": 2, "layout": "dwindle"}],
            "windows": [
                {"hwnd": 0, "exe": "Code.exe", "workspace": 1, "floating": false, "minimized": false},
                {"hwnd": 0, "exe": "WindowsTerminal.exe", "workspace": 1, "floating": false, "minimized": false},
                {"hwnd": 0, "exe": "Calculator.exe", "workspace": 1, "floating": true, "minimized": false},
                {"hwnd": 0, "exe": "msedge.exe", "workspace": 2, "floating": false, "minimized": false},
            ]
        });
        let got = parse_capture(&v).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].layout, Some(LayoutKind::Master));
        assert_eq!(got[0].apps.iter().map(|a| a.0.as_str()).collect::<Vec<_>>(), ["Code.exe", "WindowsTerminal.exe"]);
        assert_eq!(got[1].number, 2);
        assert!(parse_capture(&serde_json::json!({"windows": []})).is_err());
    }

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
        let mut ed = Editor::load(&cfg);
        // An empty workspace isn't written.
        ed.ws = 5;
        ed.shown_or_add();
        let mut doc: DocumentMut = "[general]\nworkspaces = 9\n\n[keybinds]\n\"super+q\" = \"close\"\n".parse().unwrap();
        write(&mut doc, &ed.list);
        let out = doc.to_string();
        let back: config::Config = toml::from_str(&out).unwrap_or_else(|e| panic!("{e}\n{out}"));
        assert_eq!(back.setups, cfg.setups, "{out}");
        assert_eq!(back.keybinds.len(), 1);
        // Writing none removes the tables.
        write(&mut doc, &[]);
        assert!(!doc.to_string().contains("[[setup"));
    }

    #[test]
    fn tiles_under_the_pointer() {
        let rects = [(0.0, 0.0, 100.0, 100.0), (100.0, 0.0, 100.0, 100.0)];
        assert_eq!(tile_under(&rects, (150.0, 50.0), 0), Some(1));
        assert_eq!(tile_under(&rects, (50.0, 50.0), 0), None, "not onto itself");
        assert_eq!(tile_under(&rects, (250.0, 50.0), 0), None);
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
        assert_eq!(ed.sel, Some(0));
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
}

// ---------------------------------------------------------------------------------------------
// Sample data for headless previews (`--settings --snap out.png --scene workspaces`)

/// Previews list made-up open apps in the add picker, never what's really running.
static SAMPLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn sample(s: &mut S) {
    SAMPLE.store(true, Ordering::Relaxed);
    let cfg: config::Config = toml::from_str(
        r#"
        [[setup]]
        name = "Dev"
        key = "super+alt+1"
        [[setup.workspace]]
        number = 1
        splits = [0.62]
        apps = [{ exe = "Code.exe", run = "code" }, { exe = "WindowsTerminal.exe", run = "wt" }, { exe = "Postman.exe" }]
        [[setup.workspace]]
        number = 2
        apps = [{ exe = "msedge.exe" }]
        [[setup.workspace]]
        number = 3
        apps = [{ exe = "slack.exe" }]

        [[setup]]
        name = "Deep work"
        key = "super+alt+2"
        [[setup.workspace]]
        number = 1
        layout = "master"
        master_ratio = 0.6
        apps = [{ exe = "Obsidian.exe" }, { exe = "Spotify.exe" }, { exe = "Todoist.exe" }]

        [[setup]]
        name = "Writing"
        [[setup.workspace]]
        number = 4
        layout = "monocle"
        apps = [{ exe = "WINWORD.EXE" }, { exe = "msedge.exe" }]
        "#,
    )
    .expect("sample setups parse");
    let mut ed = Editor::load(&cfg);
    ed.screen = (1920, 1050);
    ed.tile = Some(0);
    s.ed = ed;
}

/// Terminal being dragged onto Code.
pub fn sample_drag(s: &mut S) {
    let d = defaults(s);
    let c = Canvas::new(s.ed.screen);
    let w = s.ed.shown().map(to_config).expect("sample workspace");
    let rects = tile_rects(&w, &d, &c);
    let (from, to) = (rects[1], rects[0]);
    let grab = (from.2 * 0.5, from.3 * 0.4);
    s.ed.tile = Some(1);
    s.ed.drag = Some(TileDrag {
        from: 1,
        origin: (from.0, from.1),
        grab,
        pointer: (to.0 + to.2 * 0.55, to.1 + to.3 * 0.45),
        moved: true,
    });
}

/// The seam between Code and the rest, mid-drag.
pub fn sample_resize(s: &mut S) {
    if let Some(w) = s.ed.shown_mut() {
        w.splits = vec![0.55, 0.6];
    }
    s.ed.seam = Some(0);
    s.ed.tile = None;
}

pub fn sample_master(s: &mut S) {
    s.ed.select(1);
    s.ed.tile = Some(0);
}

/// The add picker open.
pub fn sample_add(s: &mut S) {
    s.ed.tile = Some(1);
    s.ed.picker.step(rune_kit::ComboboxEvent::Toggle { current: None });
}
