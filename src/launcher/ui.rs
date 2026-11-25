//! The launcher's Rune state and view: a search field, the best matches with their icons, and
//! a footer of key hints. Keyboard first: type, arrows (or Ctrl+N / Ctrl+P), Enter, Escape.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::Sender;
use std::sync::{Arc, OnceLock};

use rune::{
    Color, Cx, El, Image, ImageCache, Key, KeyEvent, Span, SurfaceFit, TextEdit, TextStyle, Tx, col, el, row, spacer, text,
    text_input,
};

use super::rank::{self, History};
use super::system::{App, Icon};
use super::{Job, Style};

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

fn max_results() -> usize {
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

pub struct State {
    query: TextEdit,
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
}

impl State {
    pub(super) fn new(history: History, jobs: Sender<Job>) -> Self {
        State {
            query: focused_edit(),
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
        }
    }

    /// Back to an empty search with the field focused, as each open starts.
    pub fn reset(&mut self) {
        self.query = focused_edit();
        self.refresh();
    }

    pub fn set_apps(&mut self, apps: Vec<App>) {
        // Keep the highlighted app highlighted across a rescan.
        let keep = self.hits.get(self.selected).map(|h| self.apps[h.app].id.clone());
        self.apps = apps;
        self.scanned = true;
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

    /// Re-ranks for the current query and asks for the icons the new results need.
    fn refresh(&mut self) {
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
        let on_screen: HashSet<Arc<str>> = self.hits.iter().map(|h| self.apps[h.app].id.clone()).collect();
        for id in &on_screen {
            if self.requested.insert(id.clone()) {
                let _ = self.jobs.send(Job::Icon(id.clone()));
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

    fn step(&mut self, by: isize) {
        let n = self.hits.len() as isize;
        if n > 0 {
            self.selected = (self.selected as isize + by).rem_euclid(n) as usize;
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
        Key::Escape if !s.query.text.is_empty() => s.reset(),
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

pub fn view(s: &State, _cx: &mut Cx<State>) -> El {
    let t = style();
    let field = text_input(&s.query, TextStyle::new(19.0, 400.0))
        .key("query")
        .label("Search apps")
        .single_line()
        .grow(1.0)
        .color(t.foreground)
        .placeholder("Search apps…", t.muted)
        .caret(t.accent, t.accent.alpha(0.35))
        .on_edit(|s: &mut State| &mut s.query)
        .on_input_key(on_key)
        .on_input_change(|s: &mut State, _| s.refresh())
        .on_submit(|s: &mut State, tx| {
            let i = s.selected;
            s.launch(i, tx);
        });
    let header = row().w_full().h(HEADER).px(20.0).gap(14.0).center_y().child(search_glyph(t.muted)).child(field);

    let body = if !s.scanned {
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
        .child(app_icon(s, app, t))
        .child(name)
        .child(kind)
}

/// The app's icon, or its initial on a tile while there's none (yet).
fn app_icon(s: &State, app: &App, t: &Style) -> El {
    if let Some(img) = s.icons.get(&app.id).filter(|img| img.size().is_some()) {
        return el().size(ICON, ICON).pixels_fit(img.pixels(), SurfaceFit::Fit);
    }
    let initial = app.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
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
    if !s.hits.is_empty() {
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

fn hairline(t: &Style) -> El {
    el().w_full().h(1.0).fill(t.surface.alpha(0.8))
}

/// A magnifying glass: a ring and a handle at 45 degrees.
fn search_glyph(c: Color) -> El {
    el().size(20.0, 20.0)
        .child(el().absolute(1.0, 1.0).size(13.0, 13.0).radius(6.5).border(2.0, c))
        .child(
            el().absolute(10.75, 13.9)
                .size(8.5, 2.2)
                .radius(1.1)
                .fill(c)
                .rotate(std::f32::consts::FRAC_PI_4),
        )
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
    fn height_fits_rows() {
        assert!(height(8) > height(4));
        assert_eq!(height(0), height(1));
    }
}
