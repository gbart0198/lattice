//! The launcher's Rune state and view: a search field, the best matches with their icons, and
//! a footer of key hints. Keyboard first: type, arrows (or Ctrl+N / Ctrl+P), Enter, Escape.
//!
//! Two modes: apps, and files (entered by typing the file prefix and a space, shown by a chip
//! beside the field, left with Backspace on an empty field). In file mode Ctrl+Enter runs the
//! item's second action.
//!
//! Window search is a third mode, opened by the host's window search key or entered with the
//! window prefix: the host's open windows, most recently used first; Enter switches to one.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};

use rune::{
    Color, Cx, El, Image, ImageCache, Key, KeyEvent, Shape, Span, SurfaceFit, TextEdit, TextStyle, Tx, canvas, col, el, row,
    spacer, text, text_input,
};

use super::files::{FileActions, FileHit, Results, Searcher, Source};
use super::rank::{self, History};
use super::system::{App, Icon};
use super::windows::{self, WindowEntry, WindowHit};
use super::{Job, Style, WindowSearch};

const HEADER: f32 = 60.0;
const SECTION: f32 = 30.0;
const ROW: f32 = 46.0;
const LIST_PAD: f32 = 8.0;
const FOOTER: f32 = 40.0;
const ICON: f32 = 28.0;
/// Decoded icons kept beyond the ones on screen.
const ICON_CACHE: usize = 64;

static STYLE: OnceLock<(Style, usize)> = OnceLock::new();

pub fn set_style(style: Style, max_results: usize) {
    let _ = STYLE.set((style, max_results.clamp(1, 20)));
}

pub fn style() -> &'static Style {
    &STYLE.get().expect("launcher style set").0
}

pub(super) fn max_results() -> usize {
    STYLE.get().map_or(8, |s| s.1)
}

/// The window height that fits `rows` results.
pub fn height(rows: usize) -> f32 {
    HEADER + 1.0 + SECTION + rows.clamp(1, 20) as f32 * ROW + 2.0 * LIST_PAD + 1.0 + FOOTER
}

/// One result: an index into `State::apps`, and the name's matched byte offsets.
struct Hit {
    app: usize,
    positions: Vec<usize>,
}

/// What the launcher searches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Apps,
    Files,
    Windows,
}

/// File mode's parts, from the launcher's options.
pub struct Files {
    pub searcher: Searcher,
    /// Shared with the search thread, which ranks with a snapshot of it.
    pub history: Arc<Mutex<History>>,
    pub prefix: String,
    pub actions: FileActions,
}

pub struct State {
    query: TextEdit,
    mode: Mode,
    files: Files,
    file_hits: Vec<FileHit>,
    file_source: Source,
    /// The newest file query sent, and the one `file_hits` answers.
    file_gen: u64,
    file_shown: u64,
    /// The catalog, sorted by name.
    apps: Vec<App>,
    /// The first scan has come back.
    scanned: bool,
    hits: Vec<Hit>,
    selected: usize,
    history: History,
    jobs: Sender<Job>,
    cache: Option<ImageCache>,
    /// Icons by app id: decoding (`icons`), shown (`ready`), asked of the worker (`requested`,
    /// which also remembers apps without an icon so they're not asked for again).
    icons: HashMap<Arc<str>, Image>,
    ready: HashSet<Arc<str>>,
    requested: HashSet<Arc<str>>,
    /// Icons that arrived before the window (and its image cache) existed.
    pending: Vec<(Arc<str>, Icon)>,
    windows: WindowSearch,
    /// The host's windows, most recently used first, once they've arrived (`window_listed`).
    window_list: Vec<WindowEntry>,
    window_listed: bool,
    window_hits: Vec<WindowHit>,
    /// Window search was entered by typing its prefix (Backspace puts the prefix back).
    window_by_prefix: bool,
}

impl State {
    pub(super) fn new(history: History, jobs: Sender<Job>, files: Files, windows: WindowSearch) -> Self {
        State {
            query: focused_edit(),
            mode: Mode::Apps,
            files,
            file_hits: Vec::new(),
            file_source: Source::Idle,
            file_gen: 0,
            file_shown: 0,
            apps: Vec::new(),
            scanned: false,
            hits: Vec::new(),
            selected: 0,
            history,
            jobs,
            cache: None,
            icons: HashMap::new(),
            ready: HashSet::new(),
            requested: HashSet::new(),
            pending: Vec::new(),
            windows,
            window_list: Vec::new(),
            window_listed: false,
            window_hits: Vec::new(),
            window_by_prefix: false,
        }
    }

    /// Back to an empty search with the field focused, as each open starts.
    pub fn reset(&mut self) {
        self.query = focused_edit();
        self.mode = Mode::Apps;
        self.file_hits.clear();
        self.window_list.clear();
        self.refresh();
    }

    /// The field changed: enter file mode on the prefix, then search.
    fn edited(&mut self) {
        if self.mode == Mode::Apps
            && let Some(rest) = strip_mode_prefix(&self.query.text, &self.files.prefix)
        {
            let rest = rest.to_string();
            self.mode = Mode::Files;
            self.file_hits.clear();
            self.file_source = Source::Idle;
            self.query.replace_text(rest.clone());
            self.query.set_caret(rest.len());
            self.files.searcher.warm();
        }
        if self.mode == Mode::Apps
            && let Some(rest) = strip_mode_prefix(&self.query.text, &self.windows.prefix)
        {
            let rest = rest.to_string();
            self.enter_windows(true);
            self.query.replace_text(rest.clone());
            self.query.set_caret(rest.len());
            (self.windows.list)();
        }
        self.refresh();
    }

    /// Window search, waiting for the host's list.
    fn enter_windows(&mut self, by_prefix: bool) {
        self.mode = Mode::Windows;
        self.window_by_prefix = by_prefix;
        self.window_list.clear();
        self.window_hits.clear();
        self.window_listed = false;
    }

    pub fn in_windows(&self) -> bool {
        self.mode == Mode::Windows
    }

    /// Window search over `list`, with an empty field (the host's window search key).
    pub fn show_windows(&mut self, list: Vec<WindowEntry>, by_prefix: bool) {
        self.query = focused_edit();
        self.enter_windows(by_prefix);
        self.windows_listed(list);
    }

    /// The host's windows arrived (they come with their icons).
    pub fn windows_listed(&mut self, list: Vec<WindowEntry>) {
        if self.mode != Mode::Windows {
            return; // asked for by a window search that has since been left
        }
        for icon in list.iter().filter_map(|w| w.icon.as_ref()) {
            let id = icon.id();
            if !self.icons.contains_key(&id) && !self.pending.iter().any(|(p, _)| *p == id) {
                self.pending.push((id, Icon { size: icon.size, rgba: icon.rgba.clone() }));
            }
        }
        self.decode_pending();
        self.window_list = list;
        self.window_listed = true;
        self.refresh();
    }

    /// Back to app search; the field holds the prefix again if that's how it was entered.
    fn leave_windows(&mut self) {
        let text = if self.window_by_prefix { self.windows.prefix.clone() } else { String::new() };
        self.mode = Mode::Apps;
        self.window_list.clear();
        self.query.replace_text(text.clone());
        self.query.set_caret(text.len());
        self.refresh();
    }

    /// Back to app search, the field holding the prefix as if only the space were deleted.
    fn leave_files(&mut self) {
        self.mode = Mode::Apps;
        self.file_hits.clear();
        let prefix = self.files.prefix.clone();
        self.query.replace_text(prefix.clone());
        self.query.set_caret(prefix.len());
        self.refresh();
    }

    pub fn files_loaded(&mut self, r: Results) {
        if self.mode != Mode::Files || r.generation != self.file_gen {
            return; // an answer to something no longer asked
        }
        // A refresh of the same query (the index grew) keeps the highlighted item.
        let keep = (r.generation == self.file_shown).then(|| self.file_hits.get(self.selected).map(|h| h.path.clone()));
        self.file_hits = r.hits;
        self.file_source = r.source;
        self.file_shown = r.generation;
        self.selected = keep.flatten().and_then(|p| self.file_hits.iter().position(|h| h.path == p)).unwrap_or(0);
        self.request_icons();
    }

    fn searching(&self) -> bool {
        self.mode == Mode::Files && self.file_gen != self.file_shown
    }

    pub fn set_apps(&mut self, apps: Vec<App>) {
        // Keep the highlighted app highlighted across a rescan.
        let keep = self.hits.get(self.selected).map(|h| self.apps[h.app].id.clone());
        self.apps = apps;
        self.scanned = true;
        super::bench::SCANNED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.refresh();
        if let Some(id) = keep {
            self.selected = self.hits.iter().position(|h| self.apps[h.app].id == id).unwrap_or(0);
        }
    }

    pub fn icon_loaded(&mut self, id: Arc<str>, icon: Option<Icon>) {
        if let Some(icon) = icon {
            self.pending.push((id, icon));
            self.decode_pending();
        }
    }

    /// Hands arrived icons to the image cache, once there is one.
    fn decode_pending(&mut self) {
        let Some(cache) = &self.cache else { return };
        for (id, icon) in self.pending.drain(..) {
            self.icons.insert(id, cache.rgba(icon.size, icon.size, icon.rgba));
        }
    }

    /// Re-ranks (apps) or re-asks (files) for the current query.
    fn refresh(&mut self) {
        match self.mode {
            Mode::Apps => self.refresh_apps(),
            Mode::Files => self.refresh_files(),
            Mode::Windows => self.refresh_windows(),
        }
    }

    fn refresh_windows(&mut self) {
        self.window_hits = windows::rank(self.query.text.trim(), &self.window_list, max_results());
        self.selected = 0;
        self.request_icons();
    }

    fn refresh_files(&mut self) {
        self.file_gen += 1;
        let text = self.query.text.trim().to_string();
        if text.is_empty() {
            self.file_hits.clear();
            self.file_shown = self.file_gen;
            self.selected = 0;
            return;
        }
        // The old results stay up until the new ones land, so typing doesn't flicker.
        self.files.searcher.query(self.file_gen, text);
    }

    /// Re-ranks apps for the current query and asks for the icons the new results need.
    fn refresh_apps(&mut self) {
        let query = self.query.text.trim();
        let mut scored: Vec<(i32, usize, Vec<usize>)> = self
            .apps
            .iter()
            .enumerate()
            .filter_map(|(i, app)| {
                let m = rank::fuzzy(query, &app.name)?;
                Some((m.score + self.history.boost(&app.id), i, m.positions))
            })
            .collect();
        // Stable: ties stay alphabetical (an empty query with no history lists A to Z).
        scored.sort_by_key(|&(score, ..)| std::cmp::Reverse(score));
        scored.truncate(max_results());
        self.hits = scored.into_iter().map(|(_, app, positions)| Hit { app, positions }).collect();
        self.selected = 0;
        self.request_icons();
    }

    fn request_icons(&mut self) {
        let on_screen: HashSet<Arc<str>> = match self.mode {
            Mode::Apps => self.hits.iter().map(|h| self.apps[h.app].id.clone()).collect(),
            Mode::Files => self.file_hits.iter().map(|h| h.path.clone()).collect(),
            // All of them, not just the shown rows: they arrive with the list, never fetched again.
            Mode::Windows => self.window_list.iter().filter_map(|w| Some(w.icon.as_ref()?.id())).collect(),
        };
        for id in &on_screen {
            if self.mode != Mode::Windows && self.requested.insert(id.clone()) {
                let job = match self.mode {
                    Mode::Apps => Job::Icon(id.clone()),
                    Mode::Files | Mode::Windows => Job::FileIcon(id.clone()),
                };
                let _ = self.jobs.send(job);
            }
        }
        if self.icons.len() > ICON_CACHE {
            self.icons.retain(|id, _| on_screen.contains(id));
            self.ready.retain(|id| on_screen.contains(id));
            // Dropped icons are fetched again when they're next on screen.
            let kept = &self.icons;
            self.requested.retain(|id| kept.contains_key(id) || on_screen.contains(id));
        }
    }

    fn rows(&self) -> usize {
        match self.mode {
            Mode::Apps => self.hits.len(),
            Mode::Files => self.file_hits.len(),
            Mode::Windows => self.window_hits.len(),
        }
    }

    fn step(&mut self, by: isize) {
        let n = self.rows() as isize;
        if n > 0 {
            self.selected = (self.selected as isize + by).rem_euclid(n) as usize;
        }
    }

    /// Enter (or Ctrl+Enter, `second`) on row `index`.
    fn activate(&mut self, index: usize, second: bool, tx: &mut Tx) {
        match self.mode {
            Mode::Apps => self.launch(index, tx),
            Mode::Files => {
                let Some(hit) = self.file_hits.get(index) else { return };
                let action = self.files.actions.pick(hit.dir, second).clone();
                let path = hit.path.clone();
                self.files.history.lock().unwrap().record(&path);
                let _ = self.jobs.send(Job::File(action, path));
                self.reset();
                tx.close_window();
            }
            Mode::Windows => {
                let Some(hit) = self.window_hits.get(index) else { return };
                (self.windows.focus)(self.window_list[hit.window].hwnd);
                self.reset();
                tx.close_window();
            }
        }
    }

    fn launch(&mut self, index: usize, tx: &mut Tx) {
        let Some(hit) = self.hits.get(index) else { return };
        let id = self.apps[hit.app].id.clone();
        self.history.record(&id);
        let _ = self.jobs.send(Job::Launch(id));
        self.reset();
        tx.close_window();
    }
}

/// The text after "<prefix> " if `text` starts with it (prefix case-insensitive).
fn strip_mode_prefix<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    if prefix.is_empty() {
        return None;
    }
    let head = text.get(..prefix.len())?;
    let rest = text[prefix.len()..].strip_prefix(' ')?;
    head.eq_ignore_ascii_case(prefix).then_some(rest)
}

fn focused_edit() -> TextEdit {
    let mut edit = TextEdit::new("");
    edit.focused = true;
    edit
}

pub fn setup(s: &mut State, wake: rune::Waker) {
    s.cache = Some(ImageCache::new(wake));
    s.decode_pending();
}

pub fn poll(s: &mut State, _t: f64, _tx: &mut Tx) -> bool {
    s.decode_pending();
    // Icons decode off-thread; redraw as each one lands.
    let mut changed = false;
    for (id, img) in &s.icons {
        if img.size().is_some() && s.ready.insert(id.clone()) {
            changed = true;
        }
    }
    changed
}

/// Clicking away dismisses the launcher, as Escape does.
pub fn on_focus(s: &mut State, focused: bool, tx: &mut Tx) {
    if !focused {
        s.reset();
        tx.close_window();
    }
}

fn on_key(s: &mut State, ev: &KeyEvent, tx: &mut Tx) -> bool {
    match ev.key {
        Key::Down => s.step(1),
        Key::Up => s.step(-1),
        Key::Tab => s.step(if ev.mods.shift { -1 } else { 1 }),
        _ if ev.is_command('n') => s.step(1),
        _ if ev.is_command('p') => s.step(-1),
        Key::Enter if ev.mods.ctrl => {
            let i = s.selected;
            s.activate(i, true, tx);
        }
        Key::Backspace if s.mode == Mode::Files && s.query.text.is_empty() => s.leave_files(),
        Key::Backspace if s.mode == Mode::Windows && s.query.text.is_empty() => s.leave_windows(),
        Key::Escape if s.mode == Mode::Windows && !s.query.text.is_empty() => {
            s.query = focused_edit();
            s.refresh();
        }
        Key::Escape if !s.query.text.is_empty() || s.mode == Mode::Files => s.reset(),
        Key::Escape => {
            s.reset();
            tx.close_window();
        }
        _ => return false,
    }
    true
}

// ---------------------------------------------------------------------------------------------
// View

pub fn view(s: &State, cx: &mut Cx<State>) -> El {
    use std::sync::atomic::Ordering;
    let started = std::time::Instant::now();
    let el = build(s, cx);
    super::bench::VIEW_NANOS.fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
    super::bench::VIEWS.fetch_add(1, Ordering::Relaxed);
    el
}

fn build(s: &State, _cx: &mut Cx<State>) -> El {
    let t = style();
    let files = s.mode == Mode::Files;
    let (label, placeholder) = match s.mode {
        Mode::Files => ("Search files and folders", "Search files and folders…"),
        Mode::Windows => ("Search windows", "Search open windows…"),
        Mode::Apps => ("Search apps", "Search apps…"),
    };
    let field = text_input(&s.query, TextStyle::new(19.0, 400.0))
        .key("query")
        .label(label)
        .single_line()
        .grow(1.0)
        .color(t.foreground)
        .placeholder(placeholder, t.muted)
        .caret(t.accent, t.accent.alpha(0.35))
        .on_edit(|s: &mut State| &mut s.query)
        .on_input_key(on_key)
        .on_input_change(|s: &mut State, _| s.edited())
        .on_submit(|s: &mut State, tx| {
            let i = s.selected;
            s.activate(i, false, tx);
        });
    let mut header = row().w_full().h(HEADER).px(20.0).gap(14.0).center_y().child(search_glyph(t.muted));
    if files {
        header = header.child(mode_chip("Files", t));
    }
    if s.mode == Mode::Windows {
        header = header.child(chip("Windows", windows_glyph(t.accent), t));
    }
    let header = header.child(field);

    let body = if files {
        file_body(s, t)
    } else if s.mode == Mode::Windows {
        window_body(s, t)
    } else if !s.scanned {
        message("Finding your apps…", t)
    } else if s.hits.is_empty() {
        message(&format!("No apps match \u{201c}{}\u{201d}", s.query.text.trim()), t)
    } else {
        results(s, t)
    };

    col()
        .w_full()
        .h_full()
        .fill(t.background)
        .child(header)
        .child(hairline(t))
        .child(body)
        .child(hairline(t))
        .child(footer(s, t))
}

fn results(s: &State, t: &Style) -> El {
    let label = if s.query.text.trim().is_empty() { "Suggestions" } else { "Applications" };
    let section = row()
        .w_full()
        .h(SECTION)
        .px(20.0)
        .center_y()
        .child(text(label).font(TextStyle::new(12.0, 600.0)).color(t.muted));
    let list = s
        .hits
        .iter()
        .enumerate()
        .fold(col().w_full().px(LIST_PAD).gap(0.0), |list, (i, hit)| list.child(result_row(s, i, hit, t)));
    col().w_full().grow(1.0).pt(LIST_PAD).child(section).child(list)
}

/// The file mode indicator beside the field.
fn mode_chip(label: &str, t: &Style) -> El {
    chip(label, folder_glyph(t.accent), t)
}

/// A mode indicator: a glyph and a label on an accent tint.
fn chip(label: &str, glyph: El, t: &Style) -> El {
    row()
        .key("mode-chip")
        .label(format!("{label} mode"))
        .h(26.0)
        .px(10.0)
        .gap(6.0)
        .radius(7.0)
        .center_y()
        .fill(t.accent.alpha(0.18))
        .child(glyph)
        .child(text(label.to_string()).font(TextStyle::new(12.5, 650.0)).color(t.accent))
}

fn file_body(s: &State, t: &Style) -> El {
    let query = s.query.text.trim();
    if query.is_empty() {
        return message("Type to search files and folders  ·  Backspace for apps", t);
    }
    if s.file_hits.is_empty() {
        let msg = if s.searching() || is_walking(s.file_source) {
            "Searching…".to_string()
        } else {
            format!("No files match \u{201c}{query}\u{201d}")
        };
        return message(&msg, t);
    }
    let section = row()
        .w_full()
        .h(SECTION)
        .px(20.0)
        .center_y()
        .child(text("Files & Folders").font(TextStyle::new(12.0, 600.0)).color(t.muted))
        .child(spacer())
        .child(text(source_label(s.file_source)).font(TextStyle::new(11.5, 500.0)).color(t.muted.alpha(0.8)));
    let list = s
        .file_hits
        .iter()
        .enumerate()
        .fold(col().w_full().px(LIST_PAD).gap(0.0), |list, (i, hit)| list.child(file_row(s, i, hit, t)));
    col().w_full().grow(1.0).pt(LIST_PAD).child(section).child(list)
}

fn window_body(s: &State, t: &Style) -> El {
    let query = s.query.text.trim();
    if !s.window_listed {
        return message("Listing windows…", t);
    }
    if s.window_list.is_empty() {
        return message("No open windows", t);
    }
    if s.window_hits.is_empty() {
        return message(&format!("No windows match \u{201c}{query}\u{201d}"), t);
    }
    let n = s.window_list.len();
    let section = row()
        .w_full()
        .h(SECTION)
        .px(20.0)
        .center_y()
        .child(text("Windows").font(TextStyle::new(12.0, 600.0)).color(t.muted))
        .child(spacer())
        .child(text(format!("{n} open")).font(TextStyle::new(11.5, 500.0)).color(t.muted.alpha(0.8)));
    let monitors = windows::many_monitors(&s.window_list);
    let list = s
        .window_hits
        .iter()
        .enumerate()
        .fold(col().w_full().px(LIST_PAD).gap(0.0), |list, (i, hit)| list.child(window_row(s, i, hit, monitors, t)));
    col().w_full().grow(1.0).pt(LIST_PAD).child(section).child(list)
}

fn window_row(s: &State, i: usize, hit: &WindowHit, monitors: bool, t: &Style) -> El {
    let w = &s.window_list[hit.window];
    let on = i == s.selected;
    // A window without a title goes by its app's name, which then carries the app highlight.
    let untitled = w.title.trim().is_empty();
    let (name, positions) = if untitled { (w.app.as_str(), &hit.app) } else { (w.title.as_str(), &hit.title) };
    let title = text(name.to_string())
        .spans(highlight(name, positions, t.accent))
        .font(TextStyle::new(14.0, if on { 550.0 } else { 450.0 }))
        .color(t.foreground)
        .truncate();
    // The detail line starts with the app name, so its match offsets apply as they are.
    let detail = windows::detail(w, monitors);
    let app_spans = if untitled { Vec::new() } else { highlight(&detail, &hit.app, t.accent) };
    let detail = text(detail.clone()).spans(app_spans).font(TextStyle::new(11.5, 400.0)).color(t.muted).truncate();
    let kind = if on { "Switch".to_string() } else { format!("Workspace {}", w.workspace) };
    let kind = text(kind).font(TextStyle::new(12.5, 500.0)).color(if on { t.foreground.alpha(0.7) } else { t.muted });
    let initial = w.app.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
    let icon_id = w.icon.as_ref().map(|i| i.id());
    row()
        .key(format!("window-{:x}", w.hwnd))
        .role("button")
        .label(name.to_string())
        .hit()
        .w_full()
        .h(ROW)
        .px(12.0)
        .gap(14.0)
        .radius(10.0)
        .center_y()
        .fill(if on { t.surface } else { Color::TRANSPARENT })
        .on_hover(move |s: &mut State| s.selected = i)
        .on_press_tx(move |s: &mut State, tx| s.activate(i, false, tx))
        .child(icon_or_tile(s, icon_id.as_deref().unwrap_or(""), initial, t))
        .child(col().gap(1.0).grow(1.0).shrink().child(title).child(detail))
        .child(kind)
}

fn is_walking(source: Source) -> bool {
    matches!(source, Source::Index(st) if st.walking)
}

/// Where results come from, for the section header.
fn source_label(source: Source) -> String {
    match source {
        Source::Idle => String::new(),
        Source::Everything => "Everything".into(),
        Source::Index(st) if st.walking => format!("Indexing… {}", thousands(st.found.max(st.entries))),
        Source::Index(st) => format!("{} indexed", thousands(st.entries)),
    }
}

fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn file_row(s: &State, i: usize, hit: &FileHit, t: &Style) -> El {
    let on = i == s.selected;
    let name = hit.name();
    let title = text(name.to_string())
        .spans(highlight(name, &hit.positions, t.accent))
        .font(TextStyle::new(14.0, if on { 550.0 } else { 450.0 }))
        .color(t.foreground)
        .truncate();
    let folder = text(display_dir(&hit.path[..hit.name_at]))
        .font(TextStyle::new(11.5, 400.0))
        .color(t.muted)
        .truncate();
    let kind = if on { s.files.actions.pick(hit.dir, false).label() } else if hit.dir { "Folder" } else { "File" };
    let kind = text(kind).font(TextStyle::new(12.5, 500.0)).color(if on { t.foreground.alpha(0.7) } else { t.muted });
    let initial = if hit.dir { "▸".to_string() } else { name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default() };
    row()
        .key(format!("file-{}", hit.path))
        .role("button")
        .label(name.to_string())
        .hit()
        .w_full()
        .h(ROW)
        .px(12.0)
        .gap(14.0)
        .radius(10.0)
        .center_y()
        .fill(if on { t.surface } else { Color::TRANSPARENT })
        .on_hover(move |s: &mut State| s.selected = i)
        .on_press_tx(move |s: &mut State, tx| s.activate(i, false, tx))
        .child(icon_or_tile(s, &hit.path, initial, t))
        .child(col().gap(1.0).grow(1.0).shrink().child(title).child(folder))
        .child(kind)
}

/// A folder path for display: your user folder as "~", no trailing separator.
fn display_dir(dir: &str) -> String {
    static HOME: OnceLock<Option<String>> = OnceLock::new();
    let dir = dir.strip_suffix('\\').filter(|d| !d.ends_with(':')).unwrap_or(dir);
    let home = HOME.get_or_init(|| std::env::var("USERPROFILE").ok().filter(|h| !h.is_empty()));
    if let Some(home) = home
        && dir.len() >= home.len()
        && dir[..home.len()].eq_ignore_ascii_case(home)
        && (dir.len() == home.len() || dir[home.len()..].starts_with('\\'))
    {
        return format!("~{}", &dir[home.len()..]);
    }
    dir.to_string()
}

fn result_row(s: &State, i: usize, hit: &Hit, t: &Style) -> El {
    let app = &s.apps[hit.app];
    let on = i == s.selected;
    let name = text(app.name.to_string())
        .spans(highlight(&app.name, &hit.positions, t.accent))
        .font(TextStyle::new(14.5, if on { 550.0 } else { 450.0 }))
        .color(t.foreground)
        .truncate()
        .grow(1.0)
        .shrink();
    let kind = text(if on { "Open" } else { "Application" })
        .font(TextStyle::new(12.5, 500.0))
        .color(if on { t.foreground.alpha(0.7) } else { t.muted });
    row()
        .key(format!("app-{}", app.id))
        .role("button")
        .label(app.name.to_string())
        .hit()
        .w_full()
        .h(ROW)
        .px(12.0)
        .gap(14.0)
        .radius(10.0)
        .center_y()
        .fill(if on { t.surface } else { Color::TRANSPARENT })
        .on_hover(move |s: &mut State| s.selected = i)
        .on_press_tx(move |s: &mut State, tx| s.launch(i, tx))
        .child(icon_or_tile(s, &app.id, app.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default(), t))
        .child(name)
        .child(kind)
}

/// The icon for `id` (an app id or a path), or `initial` on a tile while there's none (yet).
fn icon_or_tile(s: &State, id: &str, initial: String, t: &Style) -> El {
    if let Some(img) = s.icons.get(id).filter(|img| img.size().is_some()) {
        return el().size(ICON, ICON).pixels_fit(img.pixels(), SurfaceFit::Fit);
    }
    el().size(ICON, ICON)
        .radius(8.0)
        .fill(t.accent.alpha(0.18))
        .center()
        .child(text(initial).font(TextStyle::new(13.0, 700.0)).color(t.accent))
}

/// Accent spans over the matched characters, runs merged.
fn highlight(name: &str, positions: &[usize], accent: Color) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    for &p in positions {
        let end = p + name[p..].chars().next().map_or(1, char::len_utf8);
        match spans.last_mut() {
            Some(last) if last.end as usize == p => last.end = end as u32,
            _ => spans.push(Span::new(p, end).color(accent).weight(650.0)),
        }
    }
    spans
}

fn message(msg: &str, t: &Style) -> El {
    col()
        .w_full()
        .grow(1.0)
        .center()
        .child(text(msg.to_string()).font(TextStyle::new(14.0, 450.0)).color(t.muted))
}

fn footer(s: &State, t: &Style) -> El {
    let brand = row()
        .gap(8.0)
        .center_y()
        .child(el().size(8.0, 8.0).radius(4.0).fill(t.accent))
        .child(text("lattice").font(TextStyle::new(12.5, 600.0)).color(t.muted));
    let mut hints = row().gap(16.0).center_y();
    if s.mode == Mode::Files {
        if let Some(hit) = s.file_hits.get(s.selected) {
            let a = &s.files.actions;
            hints = hints
                .child(hint(a.pick(hit.dir, false).label(), &["↵"], t))
                .child(hint(a.pick(hit.dir, true).label(), &["ctrl", "↵"], t));
        }
    } else if s.mode == Mode::Windows {
        if !s.window_hits.is_empty() {
            hints = hints.child(hint("Switch", &["↵"], t)).child(hint("Navigate", &["↑", "↓"], t));
        }
    } else if !s.hits.is_empty() {
        hints = hints.child(hint("Open", &["↵"], t)).child(hint("Navigate", &["↑", "↓"], t));
    }
    hints = hints.child(hint("Close", &["esc"], t));
    row()
        .w_full()
        .h(FOOTER)
        .px(16.0)
        .center_y()
        .fill(t.background.mix(t.surface, 0.35))
        .child(brand)
        .child(spacer())
        .child(hints)
}

/// "Open ↵": a label and its key caps.
fn hint(label: &str, keys: &[&str], t: &Style) -> El {
    let caps = keys.iter().fold(row().gap(4.0).center_y(), |r, k| {
        r.child(
            row()
                .h(20.0)
                .min_w(20.0)
                .px(6.0)
                .radius(5.0)
                .center()
                .fill(t.surface)
                .child(text(k.to_string()).font(TextStyle::new(11.5, 600.0)).color(t.muted)),
        )
    });
    row()
        .gap(8.0)
        .center_y()
        .child(text(label.to_string()).font(TextStyle::new(12.5, 500.0)).color(t.muted))
        .child(caps)
}

/// A folder: a tab over a body.
fn folder_glyph(c: Color) -> El {
    el().size(14.0, 12.0)
        .child(el().absolute(0.0, 0.0).size(6.0, 4.0).radius(1.5).fill(c))
        .child(el().absolute(0.0, 2.0).size(14.0, 10.0).radius(2.0).fill(c))
}

/// Two windows, one behind the other.
fn windows_glyph(c: Color) -> El {
    el().size(14.0, 12.0)
        .child(el().absolute(4.0, 0.0).size(10.0, 8.0).radius(2.0).border(1.5, c))
        .child(el().absolute(0.0, 4.0).size(10.0, 8.0).radius(2.0).fill(c))
}

fn hairline(t: &Style) -> El {
    el().w_full().h(1.0).fill(t.surface.alpha(0.8))
}

/// A magnifying glass: a ring and a handle at 45 degrees.
///
/// The handle is a stroked line, not a rotated box: `El::rotate` draws through a turned layer,
/// which Rune's CPU renderer can't paint, so the first open would start a GPU device (~100 MB
/// that stays for the life of the process).
fn search_glyph(c: Color) -> El {
    el().size(20.0, 20.0)
        .child(el().absolute(1.0, 1.0).size(13.0, 13.0).radius(6.5).border(2.0, c))
        .child(canvas(Shape::new().stroke(vec![[12.77, 12.77], [17.23, 17.23]], 2.2, c)).absolute(0.0, 0.0).size(20.0, 20.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_merges_runs() {
        let spans = highlight("Visual Studio Code", &[0, 7, 8, 14], Color::default());
        let ranges: Vec<(u32, u32)> = spans.iter().map(|s| (s.start, s.end)).collect();
        assert_eq!(ranges, vec![(0, 1), (7, 9), (14, 15)]);
    }

    #[test]
    fn mode_prefix() {
        assert_eq!(strip_mode_prefix("f ", "f"), Some(""));
        assert_eq!(strip_mode_prefix("F report", "f"), Some("report"));
        assert_eq!(strip_mode_prefix("firefox", "f"), None);
        assert_eq!(strip_mode_prefix("f", "f"), None);
        assert_eq!(strip_mode_prefix("file x", "file"), Some("x"));
        assert_eq!(strip_mode_prefix("f x", ""), None);
    }

    #[test]
    fn display_dirs() {
        assert_eq!(display_dir("D:\\"), "D:\\");
        assert_eq!(display_dir("D:\\Projects\\"), "D:\\Projects");
        assert_eq!(thousands(1234567), "1,234,567");
        assert_eq!(thousands(12), "12");
    }

    #[test]
    fn height_fits_rows() {
        assert!(height(8) > height(4));
        assert_eq!(height(0), height(1));
    }
}
