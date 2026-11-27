//! A Raycast-style app launcher: a search box over every app in the Start menu, ranked by fuzzy
//! match and by how often you launch each one, opened and dismissed with a hotkey. Typing the
//! file prefix ("f") and a space switches it to searching files and folders (see `files`).
//! It also searches the host's open windows ([`toggle_windows`], or the window prefix "w"),
//! and runs commands after the command prefix (">"): the host's, and system ones.
//!
//! Self-contained on purpose: nothing in here imports from the rest of lattice. A host
//!
//! 1. adds [`window`] (built from an [`Options`]) to its Rune event loop, and
//! 2. calls [`toggle`] from any thread when its hotkey fires,
//!
//! so lifting the launcher into its own program is a `main` that does those two things with a
//! global hotkey of its own.
//!
//! - `rank`: fuzzy matching and launch history (pure Rust);
//! - `system`: the Windows parts (app catalog, icons, launching, file actions, focus);
//! - `files`: file search on its own thread, through `everything` (voidtools Everything's IPC)
//!   or, when that isn't running, lattice's own `index`;
//! - `windows`: window search ranking (pure Rust; the host lists and switches windows);
//! - `commands`: command mode's catalog and ranking (pure Rust), and `control`, its Windows
//!   side (power, audio output, Wi-Fi);
//! - `ui`: the Rune state and view.
//!
//! Typing the setups keyword ("workspaces") on its own opens a picker of the host's saved
//! workspaces ([`set_setups`]); [`toggle_setups`] opens it directly.

mod bench;
mod commands;
mod control;
mod everything;
mod files;
mod index;
mod rank;
mod system;
pub(crate) use system::{App as CatalogApp, com_init, scan as scan_apps, style_window, target_exe};
mod ui;
mod windows;

pub use bench::run as bench;
pub use commands::{CommandOptions, HostCommand};
pub use files::{FileAction, FileActions, FileOptions};
pub use windows::{WindowEntry, WindowIcon};

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use rune::{
    Align, App, AppHandle, AppSpec, Color, DisplayTarget, OverlayPlacement, ScreenEdge, WindowOptions, WindowSpec,
    WindowsWindowOptions,
};

/// Colours for the launcher (Rune colours, i.e. linear).
#[derive(Clone, Debug)]
pub struct Style {
    pub background: Color,
    /// Raised surfaces: the selected row, key caps, the icon placeholder.
    pub surface: Color,
    pub foreground: Color,
    pub muted: Color,
    pub accent: Color,
}

pub struct Options {
    pub style: Style,
    /// Rune's (sans, mono) font family lists.
    pub families: (&'static str, &'static str),
    /// Window width in logical pixels.
    pub width: f32,
    /// Result rows shown at once.
    pub max_results: usize,
    /// Where the launch history lives.
    pub data_dir: PathBuf,
    /// Environment variables the host set for itself that launched apps shouldn't inherit.
    pub clean_env: Vec<String>,
    pub files: FileOptions,
    pub windows: WindowSearch,
    pub commands: CommandOptions,
    pub setups: SetupOptions,
}

/// The setup picker's link to the host.
#[derive(Clone)]
pub struct SetupOptions {
    /// Typed on its own (any case) to open the picker; empty turns that off.
    pub keyword: String,
    /// Restores the named setup. Runs on the launcher's UI thread: pass the request on.
    pub restore: Arc<dyn Fn(String) + Send + Sync>,
}

impl Default for SetupOptions {
    fn default() -> Self {
        SetupOptions { keyword: String::new(), restore: Arc::new(|_| {}) }
    }
}

/// A setup, as the picker lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct SetupEntry {
    pub name: String,
    /// The key chord that restores it, as written ("super+alt+1"); empty when none.
    pub key: String,
    /// What's in it ("1: Code, Terminal · 2: msedge").
    pub detail: String,
    /// Its workspaces that hold apps.
    pub workspaces: Vec<SetupThumb>,
}

/// One workspace of a setup, for a thumbnail.
#[derive(Clone, Debug, PartialEq)]
pub struct SetupThumb {
    pub number: usize,
    /// Its monitors and their tiles.
    pub thumb: crate::setups::Thumb,
}

/// The host's setups, kept for the picker.
static SETUPS: Mutex<Vec<SetupEntry>> = Mutex::new(Vec::new());

/// Hands the launcher the setups (at start and on every config reload). Any thread.
pub fn set_setups(list: Vec<SetupEntry>) {
    *SETUPS.lock().unwrap() = list.clone();
    if let Some(l) = LAUNCHER.get() {
        l.app.update(move |s, _| s.setups_listed(list));
    }
}

pub(crate) fn setups() -> Vec<SetupEntry> {
    SETUPS.lock().unwrap().clone()
}

/// Shows the launcher picking a setup, or hides it if it's already doing that.
/// Any thread.
pub fn toggle_setups(on: Option<Area>) {
    let Some(l) = LAUNCHER.get() else { return };
    if !l.app.is_visible() {
        show(l, on, None);
        l.app.update(|s, _| s.show_setups(false));
    } else {
        l.app.update(|s, tx| {
            if s.in_setups() {
                s.reset();
                tx.close_window();
            } else {
                s.show_setups(false);
            }
        });
    }
}

/// Window search's link to the host, which owns the windows. Both callbacks run on the
/// launcher's UI thread, so they should only pass the request on.
#[derive(Clone)]
pub struct WindowSearch {
    /// Typed before a space to search open windows; empty turns the prefix off (the host's
    /// [`toggle_windows`] still works).
    pub prefix: String,
    /// Asks the host for its windows, which it hands to [`set_windows`].
    pub list: Arc<dyn Fn() + Send + Sync>,
    /// Asks the host to bring this window forward, switching workspace or monitor as needed.
    pub focus: Arc<dyn Fn(isize) + Send + Sync>,
}

impl Default for WindowSearch {
    fn default() -> Self {
        WindowSearch { prefix: String::new(), list: Arc::new(|| {}), focus: Arc::new(|_| {}) }
    }
}

/// A monitor's work area in physical pixels, to centre the launcher on.
#[derive(Clone, Copy, Debug)]
pub struct Area {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub scale: f32,
}

/// Work for the launcher's background thread, which owns the shell's COM objects.
enum Job {
    Scan,
    Icon(Arc<str>),
    Launch(Arc<str>),
    FileIcon(Arc<str>),
    /// Do this with the file or folder at that path.
    File(FileAction, Arc<str>),
    /// Read what command mode can offer (sleep states, audio outputs, Wi-Fi).
    Snapshot,
    Command(commands::Action),
}

/// Icon pixels fetched per app: sharp at 2x for the 30-point icons rows show.
const ICON_PX: u32 = 64;
/// Re-read the Start menu on open when the list is older than this (new installs show up).
const RESCAN_AFTER: Duration = Duration::from_secs(60);

struct Launcher {
    app: AppHandle<ui::State>,
    jobs: Mutex<Sender<Job>>,
    width: f32,
    last_scan: Mutex<Instant>,
}

static LAUNCHER: OnceLock<Launcher> = OnceLock::new();
/// The launcher's native window, once Rune has made it (0 before).
static HWND: AtomicIsize = AtomicIsize::new(0);
/// The window that had the focus before the launcher opened, for commands that act on it.
static PREVIOUS: AtomicIsize = AtomicIsize::new(0);
/// A reopen is waiting for its first drawn frame to come out of DWM's cloak (see `reveal`).
static REVEALING: AtomicBool = AtomicBool::new(false);

/// The launcher's window, hidden until [`toggle`] shows it. Call once; starts the background
/// thread, which reads the app list straight away so the first open is instant.
pub fn window(opts: Options) -> rune::AnyWindowSpec {
    let (jobs, rx) = mpsc::channel();
    let height = ui::height(opts.max_results);
    ui::set_style(opts.style, opts.max_results);
    let history = rank::History::load(opts.data_dir.join("launcher-history.tsv"));
    let file_history = Arc::new(Mutex::new(rank::History::load(opts.data_dir.join("launcher-file-history.tsv"))));
    let (searcher, search_rx) = files::searcher();

    let spec = AppSpec::<ui::State> {
        setup: Some(ui::setup),
        poll: Some(ui::poll),
        families: opts.families,
        min_size: Some((1.0, 1.0)),
        // `init` below builds the state: it needs the job queue.
        ..AppSpec::new("lattice launcher", (opts.width, height), || unreachable!(), ui::view)
    };
    let state_jobs = jobs.clone();
    let ui_files = ui::Files {
        searcher: searcher.clone(),
        history: file_history.clone(),
        prefix: opts.files.prefix.clone(),
        actions: opts.files.actions.clone(),
    };
    let ui_windows = opts.windows;
    let ui_commands = opts.commands;
    let ui_setups = opts.setups;
    let app = App::new(spec)
        .windowless()
        .init(move || ui::State::new(history, state_jobs, ui_files, ui_windows, ui_commands, ui_setups))
        .on_focus(ui::on_focus)
        .on_frame_drawn(|_| reveal())
        .on_window(|w| {
            use rune::raw_window_handle::RawWindowHandle;
            let hwnd = w.with_raw_handle(|h| match h.as_raw() {
                RawWindowHandle::Win32(h) => h.hwnd.get(),
                _ => 0,
            });
            if let Ok(hwnd) = hwnd
                && hwnd != 0
            {
                HWND.store(hwnd, Ordering::SeqCst);
                system::style_window(hwnd, srgb(ui::style().surface));
            }
        });
    let handle = app.handle();

    let worker_app = handle.clone();
    let clean_env = opts.clean_env;
    std::thread::Builder::new()
        .name("lattice-launcher".into())
        .spawn(move || work(rx, worker_app, clean_env))
        .expect("spawn launcher thread");
    let _ = jobs.send(Job::Scan);
    files::start(&searcher, search_rx, handle.clone(), &opts.files, opts.data_dir.clone(), file_history, ui::max_results());

    let _ = LAUNCHER.set(Launcher {
        app: handle,
        jobs: Mutex::new(jobs),
        width: opts.width,
        last_scan: Mutex::new(Instant::now()),
    });

    let options = WindowOptions {
        visible: false,
        borderless: true,
        always_on_top: true,
        resizable: false,
        min_size: Some((0.0, 0.0)),
        // Opaque on purpose: Rune paints opaque windows on the CPU, and a transparent one
        // would start a GPU device (tens of MB) for a search box. DWM rounds the corners.
        ..Default::default()
    }
    .windows(WindowsWindowOptions::default().tool_window(true).undecorated_shadow(true));
    WindowSpec::from_app("launcher", app).configured_options(options).into()
}

/// The setup picker over sample setups, in a window of its own: for headless
/// previews (`lattice --launcher-preview --snap out.png --scene setups`). Debug builds.
pub fn preview(opts: Options) {
    let (jobs, _rx) = mpsc::channel();
    let height = ui::height(opts.max_results);
    ui::set_style(opts.style, opts.max_results);
    let history = rank::History::load(std::env::temp_dir().join("lattice-preview-history.tsv"));
    let file_history = Arc::new(Mutex::new(rank::History::load(std::env::temp_dir().join("lattice-preview-files.tsv"))));
    let (searcher, _search_rx) = files::searcher();
    let ui_files = ui::Files { searcher, history: file_history, prefix: opts.files.prefix.clone(), actions: opts.files.actions.clone() };
    let (w, c, st) = (opts.windows, opts.commands, opts.setups);
    set_setups(sample_setups());
    let spec = AppSpec::<ui::State> {
        setup: Some(ui::setup),
        poll: Some(ui::poll),
        families: opts.families,
        scenes: &[
            ("setups", |s: &mut ui::State| s.show_setups(true)),
            ("setups-filter", |s: &mut ui::State| {
                s.show_setups(true);
                s.set_query("deep");
            }),
            ("setups-empty", |s: &mut ui::State| {
                s.setups_listed(Vec::new());
                s.show_setups(true);
            }),
        ],
        ..AppSpec::new("lattice launcher", (opts.width, height), || unreachable!(), ui::view)
    };
    App::new(spec).init(move || ui::State::new(history, jobs, ui_files, w, c, st)).color_scheme(rune::ColorScheme::Dark).run();
}

fn sample_setups() -> Vec<SetupEntry> {
    // One screen, or (Streaming) two side by side.
    let one = [[0.0, 0.0, 1.0, 1.0]];
    let t = |v: &[[f32; 4]]| crate::setups::Thumb { aspect: 1.9, screens: one.to_vec(), tiles: v.to_vec() };
    let two = |v: &[[f32; 4]]| crate::setups::Thumb { aspect: 3.8, screens: vec![[0.0, 0.0, 0.5, 1.0], [0.5, 0.0, 0.5, 1.0]], tiles: v.to_vec() };
    vec![
        SetupEntry {
            name: "Dev".into(),
            key: "super+alt+1".into(),
            detail: "1: Code, WindowsTerminal, Postman  ·  2: msedge  ·  3: slack".into(),
            workspaces: vec![
                SetupThumb { number: 1, thumb: t(&[[0.0, 0.0, 0.62, 1.0], [0.62, 0.0, 0.38, 0.5], [0.62, 0.5, 0.38, 0.5]]) },
                SetupThumb { number: 2, thumb: t(&[[0.0, 0.0, 1.0, 1.0]]) },
                SetupThumb { number: 3, thumb: t(&[[0.0, 0.0, 1.0, 1.0]]) },
            ],
        },
        SetupEntry {
            name: "Deep work".into(),
            key: "super+alt+2".into(),
            detail: "1: Obsidian, Spotify, Todoist".into(),
            workspaces: vec![SetupThumb { number: 1, thumb: t(&[[0.0, 0.0, 0.6, 1.0], [0.6, 0.0, 0.4, 0.5], [0.6, 0.5, 0.4, 0.5]]) }],
        },
        SetupEntry {
            name: "Writing".into(),
            key: String::new(),
            detail: "4: WINWORD, msedge".into(),
            workspaces: vec![SetupThumb { number: 4, thumb: t(&[[0.0, 0.0, 0.5, 1.0], [0.5, 0.0, 0.5, 1.0]]) }],
        },
        SetupEntry {
            name: "Streaming".into(),
            key: "super+alt+3".into(),
            detail: "1: obs64, Chrome, Discord  ·  2: Spotify".into(),
            workspaces: vec![
                SetupThumb { number: 1, thumb: two(&[[0.0, 0.0, 0.35, 1.0], [0.35, 0.0, 0.15, 1.0], [0.5, 0.0, 0.5, 1.0]]) },
                SetupThumb { number: 2, thumb: t(&[[0.0, 0.0, 1.0, 1.0]]) },
            ],
        },
    ]
}

/// Shows the launcher (centred near the top of `on`, else of the monitor under the pointer),
/// or hides it if it's showing. Any thread.
pub fn toggle(on: Option<Area>) {
    let Some(l) = LAUNCHER.get() else { return };
    if l.app.is_visible() {
        hide();
    } else {
        show(l, on, None);
    }
}

/// Shows the launcher searching `windows` (most recently used first), or hides it if it's
/// already doing that. Any thread.
pub fn toggle_windows(on: Option<Area>, windows: Vec<WindowEntry>) {
    let Some(l) = LAUNCHER.get() else { return };
    if !l.app.is_visible() {
        show(l, on, Some(windows));
    } else {
        // Already searching windows: dismiss, as the key does for apps. Open on something
        // else: switch it over in place.
        l.app.update(move |s, tx| {
            if s.in_windows() {
                s.reset();
                tx.close_window();
            } else {
                s.show_windows(windows, false);
            }
        });
    }
}

/// The host's answer to [`WindowSearch::list`]. Any thread.
pub fn set_windows(windows: Vec<WindowEntry>) {
    if let Some(l) = LAUNCHER.get() {
        l.app.update(move |s, _| s.windows_listed(windows));
    }
}

/// The launcher is on screen. Any thread.
pub fn is_visible() -> bool {
    LAUNCHER.get().is_some_and(|l| l.app.is_visible())
}

/// Where the launcher is on screen while it shows, in physical pixels: (x, y, width, height).
pub fn screen_rect() -> Option<(i32, i32, i32, i32)> {
    let hwnd = HWND.load(Ordering::SeqCst);
    if hwnd == 0 || !is_visible() {
        return None;
    }
    system::window_rect(hwnd)
}

/// Hides the launcher, forgetting what was typed. Any thread.
pub fn hide() {
    let Some(l) = LAUNCHER.get() else { return };
    l.app.update(|s, _| s.reset());
    l.app.hide();
    warn_if_gpu_started();
}

/// Logs once if some frame needed Rune's GPU renderer (a rotated element, a blur, ...): the
/// device it starts stays for the life of the process and costs ~100 MB.
fn warn_if_gpu_started() {
    static WARNED: AtomicBool = AtomicBool::new(false);
    if WARNED.load(Ordering::Relaxed) {
        return;
    }
    if rune::marks::marks().iter().any(|(name, _)| *name == "cpu_first.needs_gpu") && !WARNED.swap(true, Ordering::Relaxed) {
        log::warn!("a frame needed the GPU renderer, which started a GPU device (~100 MB); draw it with CPU-supported elements");
    }
}

/// `windows`: open searching these windows instead of apps.
fn show(l: &Launcher, on: Option<Area>, windows: Option<Vec<WindowEntry>>) {
    {
        let mut last = l.last_scan.lock().unwrap();
        if last.elapsed() > RESCAN_AFTER {
            *last = Instant::now();
            l.send(Job::Scan);
        }
    }
    PREVIOUS.store(system::foreground(), Ordering::SeqCst);
    l.app.update(move |s, _| {
        s.reset();
        if let Some(w) = windows {
            s.show_windows(w, false);
        }
    });
    match on {
        Some(a) => {
            let w = (l.width * a.scale).round() as i32;
            l.app.set_position((a.x + (a.w - w) / 2, a.y + (a.h as f32 * 0.18) as i32));
        }
        None => l.app.place(
            DisplayTarget::UnderPointer,
            OverlayPlacement::Edge { edge: ScreenEdge::Top, margin: 160.0, align: Align::Center, inset: 0.0 },
        ),
    }
    REVEALING.store(true, Ordering::SeqCst);
    l.app.open();
    // Rune activates the window as it shows it, but Windows' foreground lock can refuse a
    // background process. Give it a moment, then insist.
    std::thread::spawn(|| {
        let mut shown_at = None;
        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(10));
            let hwnd = HWND.load(Ordering::SeqCst);
            if hwnd == 0 || !LAUNCHER.get().is_some_and(|l| l.app.is_visible()) {
                continue;
            }
            if system::is_foreground(hwnd) {
                return;
            }
            let since = *shown_at.get_or_insert_with(Instant::now);
            if since.elapsed() >= Duration::from_millis(60) {
                system::bring_to_front(hwnd);
                return;
            }
        }
    });
}

/// After a frame drawn since `show`, takes the window out of DWM's cloak.
///
/// Rune (on Windows) cloaks a window it shows again "until it draws", with a 1 s fallback, but
/// its CPU renderer, which the launcher paints with, only lifts the cloak for a window's very
/// first frame. Each later open painted in a few ms and then stayed invisible for the full
/// second. The frame is in the window by the time this hook runs, so lifting it here shows the
/// new frame, never a blank one. Rune's own fallback later finds it already uncloaked.
fn reveal() {
    if !REVEALING.load(Ordering::Relaxed) {
        return;
    }
    let hwnd = HWND.load(Ordering::SeqCst);
    if hwnd != 0 && is_visible() && REVEALING.swap(false, Ordering::SeqCst) {
        system::uncloak(hwnd);
    }
}

impl Launcher {
    fn send(&self, job: Job) {
        let _ = self.jobs.lock().unwrap().send(job);
    }
}

/// The background thread: catalog scans, icons and launches, in order.
fn work(rx: Receiver<Job>, app: AppHandle<ui::State>, clean_env: Vec<String>) {
    system::com_init();
    for job in rx {
        match job {
            Job::Scan => {
                let started = Instant::now();
                let apps = system::scan();
                log::info!("launcher: {} apps in {:?}", apps.len(), started.elapsed());
                app.update(move |s, _| s.set_apps(apps));
            }
            Job::Icon(id) => {
                let icon = system::icon(&id, ICON_PX);
                app.update(move |s, _| s.icon_loaded(id, icon));
            }
            Job::Launch(id) => system::launch(&id, &clean_env),
            Job::FileIcon(path) => {
                let icon = system::file_icon(&path, ICON_PX);
                app.update(move |s, _| s.icon_loaded(path, icon));
            }
            Job::File(action, path) => run_file_action(&action, &path, &clean_env),
            Job::Snapshot => {
                let snapshot = control::snapshot();
                app.update(move |s, _| s.commands_loaded(snapshot));
            }
            Job::Command(commands::Action::Host(run, targets_window)) => {
                // The launcher had the focus; hand it back to the window the command acts on.
                let previous = PREVIOUS.load(Ordering::SeqCst);
                if targets_window && previous != 0 && previous != HWND.load(Ordering::SeqCst) {
                    system::bring_to_front(previous);
                }
                run();
            }
            Job::Command(commands::Action::System(cmd)) => control::run(&cmd),
        }
    }
}

fn run_file_action(action: &FileAction, path: &str, clean_env: &[String]) {
    match action {
        FileAction::Open => system::open_path(path, clean_env),
        FileAction::Reveal => system::reveal(path, clean_env),
        FileAction::CopyPath => system::copy_text(path),
        FileAction::Exec(template) => system::exec(&fill_template(template, path), clean_env),
    }
}

/// `{path}`, `{dir}` and `{name}` in an exec action, filled in for the item at `path`. `{dir}`
/// is the folder holding a file, or a folder itself.
fn fill_template(template: &str, path: &str) -> String {
    let p = std::path::Path::new(path);
    let name = p.file_name().map_or(path.into(), |n| n.to_string_lossy());
    let dir = if p.is_dir() { path.into() } else { p.parent().map_or(path.into(), |d| d.to_string_lossy()) };
    template.replace("{path}", path).replace("{dir}", &dir).replace("{name}", &name)
}

/// A linear Rune colour as 8-bit sRGB.
pub(crate) fn srgb(c: Color) -> [u8; 3] {
    let enc = |v: f32| {
        let s = if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
        (s.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    [enc(c.r), enc(c.g), enc(c.b)]
}
