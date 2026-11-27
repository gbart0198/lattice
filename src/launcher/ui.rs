//! The launcher's Rune state and view: a search field, the best matches with their icons, and
//! a footer of key hints. Keyboard first: type, arrows (or Ctrl+N / Ctrl+P), Enter, Escape.
//!
//! Two modes: apps, and files (entered by typing the file prefix and a space, shown by a chip
//! beside the field, left with Backspace on an empty field). In file mode Ctrl+Enter runs the
//! item's second action.
//!
//! Window search is a third mode, opened by the host's window search key or entered with the
//! window prefix: the host's open windows, most recently used first; Enter switches to one.
//!
//! Command mode, entered with the command prefix (">"), lists commands to run. The ones that
//! end the session take a second Enter, the row asking for it in between.
//!
//! Typing the setups keyword ("workspaces") opens the saved-workspace picker: each one with a
//! thumbnail of its layouts and its key; Enter restores it.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};

use rune::{
    Color, Cx, El, Image, ImageCache, Key, KeyEvent, Shape, Span, SurfaceFit, TextEdit, TextStyle, Tx, canvas, col, el, row,
    spacer, text, text_input,
};

use super::commands::{self, CommandOptions, Entry, Snapshot};
use super::files::{FileActions, FileHit, Results, Searcher, Source};
use super::rank::{self, History};
use super::system::{App, Icon};
use super::windows::{self, WindowEntry, WindowHit};
use super::{Job, SetupEntry, SetupOptions, Style, WindowSearch};

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
    Commands,
    Setups,
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
    commands: CommandOptions,
    /// The last system snapshot, kept so a reopen lists the same rows while a new one is read.
    system: Snapshot,
    catalog: Vec<Entry>,
    /// (index into `catalog`, matched byte offsets in its title).
    command_hits: Vec<(usize, Vec<usize>)>,
    /// The key of a command that was asked to confirm and waits for its second Enter.
    armed: Option<String>,
    setups: SetupOptions,
    setup_list: Vec<SetupEntry>,
    /// (index into `setup_list`, matched byte offsets in its name).
    setup_hits: Vec<(usize, Vec<usize>)>,
    /// The picker was entered by typing the keyword (Backspace takes its last letter back).
    setups_by_keyword: bool,
}

impl State {
    pub(super) fn new(
        history: History,
        jobs: Sender<Job>,
        files: Files,
        windows: WindowSearch,
        commands: CommandOptions,
        setups: SetupOptions,
    ) -> Self {
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
            commands,
            system: Snapshot::default(),
            catalog: Vec::new(),
            command_hits: Vec::new(),
            armed: None,
            setups,
            setup_list: super::setups(),
            setup_hits: Vec::new(),
            setups_by_keyword: false,
        }
    }

    /// Back to an empty search with the field focused, as each open starts.
    pub fn reset(&mut self) {
        self.query = focused_edit();
        self.mode = Mode::Apps;
        self.file_hits.clear();
        self.window_list.clear();
        self.armed = None;
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
        if self.mode == Mode::Apps
            && let Some(rest) = strip_keyword(&self.query.text, &self.setups.keyword)
        {
            let rest = rest.to_string();
            self.enter_setups(true);
            self.query.replace_text(rest.clone());
            self.query.set_caret(rest.len());
        }
        if self.mode == Mode::Apps
            && let Some(rest) = strip_mode_prefix(&self.query.text, &self.commands.prefix)
        {
            let rest = rest.to_string();
            self.mode = Mode::Commands;
            self.catalog = commands::catalog(&self.commands.host, &self.system);
            self.query.replace_text(rest.clone());
            self.query.set_caret(rest.len());
            let _ = self.jobs.send(Job::Snapshot);
        }
        self.armed = None;
        self.refresh();
    }

    /// What the system offers arrived: rebuild the commands, keeping the highlighted one.
    pub fn commands_loaded(&mut self, snapshot: Snapshot) {
        self.system = snapshot;
        if self.mode != Mode::Commands {
            return;
        }
        let keep = self.command_hits.get(self.selected).map(|h| self.catalog[h.0].key.clone());
        let armed = self.armed.take();
        self.catalog = commands::catalog(&self.commands.host, &self.system);
        self.refresh();
        if let Some(key) = keep {
            self.selected = self.command_hits.iter().position(|h| self.catalog[h.0].key == key).unwrap_or(0);
        }
        self.armed = armed.filter(|k| self.catalog.iter().any(|e| &e.key == k));
    }

    /// Back to app search, the field holding the prefix as if only the space were deleted.
    fn leave_commands(&mut self) {
        self.mode = Mode::Apps;
        self.armed = None;
        let prefix = self.commands.prefix.clone();
        self.query.replace_text(prefix.clone());
        self.query.set_caret(prefix.len());
        self.refresh();
    }

    fn enter_setups(&mut self, by_keyword: bool) {
        self.mode = Mode::Setups;
        self.setups_by_keyword = by_keyword;
        self.setup_list = super::setups();
    }

    /// Types `text` into the field (previews).
    pub fn set_query(&mut self, text: &str) {
        self.query.replace_text(text.to_string());
        self.query.set_caret(text.len());
        self.refresh();
    }

    pub fn in_setups(&self) -> bool {
        self.mode == Mode::Setups
    }

    /// The saved-workspace picker with an empty field (the host's picker command).
    pub fn show_setups(&mut self, by_keyword: bool) {
        self.query = focused_edit();
        self.enter_setups(by_keyword);
        self.refresh();
    }

    /// The host's saved workspaces changed (a config reload).
    pub fn setups_listed(&mut self, list: Vec<SetupEntry>) {
        self.setup_list = list;
        if self.mode == Mode::Setups {
            self.refresh();
        }
    }

    /// Back to app search, the field holding the keyword less its last letter, as if
    /// Backspace took it.
    fn leave_setups(&mut self) {
        let mut text = if self.setups_by_keyword { self.setups.keyword.clone() } else { String::new() };
        text.pop();
        self.mode = Mode::Apps;
        self.query.replace_text(text.clone());
        self.query.set_caret(text.len());
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
            Mode::Commands => {
                self.command_hits = commands::rank(self.query.text.trim(), &self.catalog, max_results());
                self.selected = 0;
            }
            Mode::Setups => {
                self.setup_hits = rank_setups(self.query.text.trim(), &self.setup_list, max_results());
                self.selected = 0;
            }
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
            Mode::Commands | Mode::Setups => HashSet::new(),
        };
        for id in &on_screen {
            if self.mode != Mode::Windows && self.requested.insert(id.clone()) {
                let job = match self.mode {
                    Mode::Apps => Job::Icon(id.clone()),
                    Mode::Files | Mode::Windows | Mode::Commands | Mode::Setups => Job::FileIcon(id.clone()),
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
            Mode::Commands => self.command_hits.len(),
            Mode::Setups => self.setup_hits.len(),
        }
    }

    fn step(&mut self, by: isize) {
        let n = self.rows() as isize;
        if n > 0 {
            self.selected = (self.selected as isize + by).rem_euclid(n) as usize;
            self.armed = None;
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
            Mode::Commands => {
                let Some(entry) = self.command_hits.get(index).map(|h| &self.catalog[h.0]) else { return };
                if entry.needs_confirm() && self.armed.as_ref() != Some(&entry.key) {
                    // The first Enter (or click) only asks; the row now says what the next does.
                    self.armed = Some(entry.key.clone());
                    self.selected = index;
                    return;
                }
                let _ = self.jobs.send(Job::Command(entry.action.clone()));
                self.reset();
                tx.close_window();
            }
            Mode::Setups => {
                let Some(&(i, _)) = self.setup_hits.get(index) else { return };
                (self.setups.restore)(self.setup_list[i].name.clone());
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

/// The filter after the setups keyword: "" for the keyword alone, the rest after "keyword ".
fn strip_keyword<'a>(text: &'a str, keyword: &str) -> Option<&'a str> {
    if keyword.is_empty() {
        return None;
    }
    if text.eq_ignore_ascii_case(keyword) {
        return Some("");
    }
    strip_mode_prefix(text, keyword)
}

/// Saved workspaces matching `query` by name (best first), else by what's in them; an empty
/// query lists them all in order.
fn rank_setups(query: &str, list: &[SetupEntry], limit: usize) -> Vec<(usize, Vec<usize>)> {
    let mut scored: Vec<(i32, usize, Vec<usize>)> = list
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            if let Some(m) = rank::fuzzy(query, &e.name) {
                return Some((m.score, i, m.positions));
            }
            rank::fuzzy(query, &e.detail).map(|m| (m.score - 40, i, Vec::new()))
        })
        .collect();
    scored.sort_by_key(|&(score, ..)| std::cmp::Reverse(score));
    scored.truncate(limit);
    scored.into_iter().map(|(_, i, p)| (i, p)).collect()
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
        Key::Backspace if s.mode == Mode::Commands && s.query.text.is_empty() => s.leave_commands(),
        Key::Backspace if s.mode == Mode::Setups && s.query.text.is_empty() => s.leave_setups(),
        Key::Escape if s.armed.is_some() => s.armed = None,
        Key::Escape if matches!(s.mode, Mode::Windows | Mode::Setups) && !s.query.text.is_empty() => {
            s.query = focused_edit();
            s.refresh();
        }
        Key::Escape if !s.query.text.is_empty() || matches!(s.mode, Mode::Files | Mode::Commands) => s.reset(),
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
        Mode::Commands => ("Run a command", "Run a command…"),
        Mode::Setups => ("Pick a saved workspace", "Restore a saved workspace…"),
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
    if s.mode == Mode::Commands {
        header = header.child(chip("Commands", command_glyph(t.accent), t));
    }
    if s.mode == Mode::Setups {
        header = header.child(chip("Workspaces", setups_glyph(t.accent), t));
    }
    let header = header.child(field);

    let body = if files {
        file_body(s, t)
    } else if s.mode == Mode::Windows {
        window_body(s, t)
    } else if s.mode == Mode::Commands {
        command_body(s, t)
    } else if s.mode == Mode::Setups {
        setup_body(s, t)
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

fn command_body(s: &State, t: &Style) -> El {
    if s.command_hits.is_empty() {
        return message(&format!("No commands match \u{201c}{}\u{201d}", s.query.text.trim()), t);
    }
    let section = row()
        .w_full()
        .h(SECTION)
        .px(20.0)
        .center_y()
        .child(text("Commands").font(TextStyle::new(12.0, 600.0)).color(t.muted));
    let list = s
        .command_hits
        .iter()
        .enumerate()
        .fold(col().w_full().px(LIST_PAD).gap(0.0), |list, (i, (entry, positions))| {
            list.child(command_row(s, i, &s.catalog[*entry], positions, t))
        });
    col().w_full().grow(1.0).pt(LIST_PAD).child(section).child(list)
}

fn command_row(s: &State, i: usize, entry: &Entry, positions: &[usize], t: &Style) -> El {
    let on = i == s.selected;
    let armed = s.armed.as_ref() == Some(&entry.key);
    let title = text(entry.title.clone())
        .spans(highlight(&entry.title, positions, t.accent))
        .font(TextStyle::new(14.0, if on { 550.0 } else { 450.0 }))
        .color(t.foreground)
        .truncate();
    let detail = if armed {
        text(format!("Press Enter again to {}", entry.title.to_lowercase())).font(TextStyle::new(11.5, 600.0)).color(t.accent)
    } else {
        text(entry.detail.clone()).font(TextStyle::new(11.5, 400.0)).color(t.muted)
    };
    let kind = match (armed, on) {
        (true, _) => "Confirm",
        (false, true) => "Run",
        (false, false) if matches!(entry.action, commands::Action::Host(..)) => "lattice",
        (false, false) => "System",
    };
    let kind = text(kind).font(TextStyle::new(12.5, 500.0)).color(if armed { t.accent } else if on { t.foreground.alpha(0.7) } else { t.muted });
    let initial = entry.title.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
    row()
        .key(format!("command-{}", entry.key))
        .role("button")
        .label(entry.title.clone())
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
        .child(icon_or_tile(s, "", initial, t))
        .child(col().gap(1.0).grow(1.0).shrink().child(title).child(detail.truncate()))
        .child(kind)
}

fn setup_body(s: &State, t: &Style) -> El {
    if s.setup_list.is_empty() {
        return col()
            .w_full()
            .grow(1.0)
            .center()
            .gap(6.0)
            .child(text("No saved workspaces yet").font(TextStyle::new(14.5, 550.0)).color(t.foreground))
            .child(text("Make one in Settings › Workspaces").font(TextStyle::new(12.5, 450.0)).color(t.muted));
    }
    if s.setup_hits.is_empty() {
        return message(&format!("No saved workspaces match \u{201c}{}\u{201d}", s.query.text.trim()), t);
    }
    let n = s.setup_list.len();
    let section = row()
        .w_full()
        .h(SECTION)
        .px(20.0)
        .center_y()
        .child(text("Saved workspaces").font(TextStyle::new(12.0, 600.0)).color(t.muted))
        .child(spacer())
        .child(text(if n == 1 { "1 saved".to_string() } else { format!("{n} saved") }).font(TextStyle::new(11.5, 500.0)).color(t.muted.alpha(0.8)));
    let list = s
        .setup_hits
        .iter()
        .enumerate()
        .fold(col().w_full().px(LIST_PAD).gap(0.0), |list, (i, (entry, positions))| {
            list.child(setup_row(s, i, &s.setup_list[*entry], positions, t))
        });
    col().w_full().grow(1.0).pt(LIST_PAD).child(section).child(list)
}

/// Thumbnail size: a little screen.
const THUMB_W: f32 = 46.0;
const THUMB_H: f32 = 28.0;

/// A workspace's layout in miniature: the screen, its tiles, and the workspace's number.
fn thumb(th: &super::SetupThumb, on: bool, t: &Style) -> El {
    let mut screen = el()
        .size(THUMB_W, THUMB_H)
        .radius(5.0)
        .fill(t.background.mix(t.surface, if on { 0.2 } else { 0.6 }))
        .border(1.0, t.muted.alpha(if on { 0.45 } else { 0.25 }));
    for (k, r) in th.tiles.iter().enumerate() {
        // A pixel of gap around each tile, so neighbours read apart.
        let (x, y) = (r[0] * THUMB_W + 1.5, r[1] * THUMB_H + 1.5);
        let (w, h) = ((r[2] * THUMB_W - 3.0).max(2.0), (r[3] * THUMB_H - 3.0).max(2.0));
        let a = if k == 0 { 0.85 } else { 0.5 };
        screen = screen.child(el().absolute(x, y).size(w, h).radius(2.0).fill(t.accent.alpha(a)));
    }
    let badge = row()
        .h(13.0)
        .min_w(13.0)
        .px(3.0)
        .radius(4.0)
        .center()
        .fill(t.background)
        .child(text(th.number.to_string()).font(TextStyle::new(9.5, 700.0)).color(t.foreground.alpha(0.85)));
    el().size(THUMB_W + 4.0, THUMB_H + 4.0).child(el().absolute(0.0, 0.0).child(screen)).child(el().absolute(THUMB_W - 9.0, THUMB_H - 9.0).child(badge))
}

fn setup_row(s: &State, i: usize, entry: &SetupEntry, positions: &[usize], t: &Style) -> El {
    let on = i == s.selected;
    let title = text(entry.name.clone())
        .spans(highlight(&entry.name, positions, t.accent))
        .font(TextStyle::new(14.0, if on { 550.0 } else { 450.0 }))
        .color(t.foreground)
        .truncate();
    let detail = text(entry.detail.clone()).font(TextStyle::new(11.5, 400.0)).color(t.muted).truncate();
    // Room for three, so names line up down the list.
    let mut thumbs = row().gap(4.0).center_y().w(3.0 * (THUMB_W + 4.0) + 8.0);
    for th in entry.workspaces.iter().take(3) {
        thumbs = thumbs.child(thumb(th, on, t));
    }
    if entry.workspaces.is_empty() {
        thumbs = thumbs.child(el().size(THUMB_W, THUMB_H).radius(5.0).border(1.0, t.muted.alpha(0.3)));
    }
    let right = if on {
        text("Restore").font(TextStyle::new(12.5, 500.0)).color(t.foreground.alpha(0.7))
    } else if !entry.key.trim().is_empty() {
        key_caps(&entry.key, t)
    } else {
        text(match entry.workspaces.len() {
            1 => "1 workspace".to_string(),
            n => format!("{n} workspaces"),
        })
        .font(TextStyle::new(12.5, 500.0))
        .color(t.muted)
    };
    row()
        .key(format!("setup-{}", entry.name))
        .role("button")
        .label(entry.name.clone())
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
        .child(thumbs)
        .child(col().gap(1.0).grow(1.0).shrink().child(title).child(detail))
        .child(right)
}

/// "super+alt+1" as key caps: ⊞ alt 1.
fn key_caps(chord: &str, t: &Style) -> El {
    chord.split('+').map(str::trim).filter(|k| !k.is_empty()).fold(row().gap(4.0).center_y(), |r, k| {
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
                .fill(t.surface)
                .child(text(label).font(TextStyle::new(11.5, 600.0)).color(t.muted)),
        )
    })
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
    } else if s.mode == Mode::Commands {
        if let Some(h) = s.command_hits.get(s.selected) {
            let armed = s.armed.as_ref() == Some(&s.catalog[h.0].key);
            hints = hints.child(hint(if armed { "Confirm" } else { "Run" }, &["↵"], t)).child(hint("Navigate", &["↑", "↓"], t));
        }
    } else if s.mode == Mode::Windows {
        if !s.window_hits.is_empty() {
            hints = hints.child(hint("Switch", &["↵"], t)).child(hint("Navigate", &["↑", "↓"], t));
        }
    } else if s.mode == Mode::Setups {
        if !s.setup_hits.is_empty() {
            hints = hints.child(hint("Restore", &["↵"], t)).child(hint("Navigate", &["↑", "↓"], t));
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

/// Four tiles: a saved layout.
fn setups_glyph(c: Color) -> El {
    el().size(14.0, 12.0)
        .child(el().absolute(0.0, 0.0).size(8.0, 12.0).radius(1.5).fill(c))
        .child(el().absolute(9.5, 0.0).size(4.5, 5.25).radius(1.2).fill(c))
        .child(el().absolute(9.5, 6.75).size(4.5, 5.25).radius(1.2).fill(c))
}

/// A prompt: a chevron and an underscore.
fn command_glyph(c: Color) -> El {
    el().size(14.0, 12.0)
        .child(canvas(Shape::new().stroke(vec![[1.5, 2.0], [6.0, 6.0], [1.5, 10.0]], 1.8, c)).absolute(0.0, 0.0).size(14.0, 12.0))
        .child(el().absolute(7.5, 9.5).size(6.0, 1.8).fill(c))
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
        assert_eq!(strip_mode_prefix("> move 3", ">"), Some("move 3"));
        assert_eq!(strip_mode_prefix(">move", ">"), None);
    }

    #[test]
    fn setups_keyword() {
        assert_eq!(strip_keyword("workspaces", "workspaces"), Some(""));
        assert_eq!(strip_keyword("Workspaces", "workspaces"), Some(""));
        assert_eq!(strip_keyword("workspaces dev", "workspaces"), Some("dev"));
        assert_eq!(strip_keyword("workspace", "workspaces"), None);
        assert_eq!(strip_keyword("workspaces", ""), None);
    }

    #[test]
    fn setups_rank_by_name_then_contents() {
        let e = |name: &str, detail: &str| SetupEntry { name: name.into(), key: String::new(), detail: detail.into(), workspaces: Vec::new() };
        let list = vec![e("Dev", "1: Code, WindowsTerminal  ·  2: msedge"), e("Writing", "1: WINWORD"), e("Deep work", "1: Obsidian")];
        let names = |q: &str| rank_setups(q, &list, 8).into_iter().map(|(i, _)| list[i].name.clone()).collect::<Vec<_>>();
        assert_eq!(names(""), ["Dev", "Writing", "Deep work"]);
        assert_eq!(names("dev")[0], "Dev");
        assert_eq!(names("obsidian"), ["Deep work"]);
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
