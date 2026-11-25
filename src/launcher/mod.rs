//! A Raycast-style app launcher: a search box over every app in the Start menu, ranked by fuzzy
//! match and by how often you launch each one, opened and dismissed with a hotkey.
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
//! - `system`: the Windows parts (app catalog, icons, launching, focus);
//! - `ui`: the Rune state and view.

mod rank;
mod system;
mod ui;

use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, Ordering};
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

/// The launcher's window, hidden until [`toggle`] shows it. Call once; starts the background
/// thread, which reads the app list straight away so the first open is instant.
pub fn window(opts: Options) -> rune::AnyWindowSpec {
    let (jobs, rx) = mpsc::channel();
    let height = ui::height(opts.max_results);
    ui::set_style(opts.style, opts.max_results);
    let history = rank::History::load(opts.data_dir.join("launcher-history.tsv"));

    let spec = AppSpec::<ui::State> {
        setup: Some(ui::setup),
        poll: Some(ui::poll),
        families: opts.families,
        min_size: Some((1.0, 1.0)),
        // `init` below builds the state: it needs the job queue.
        ..AppSpec::new("lattice launcher", (opts.width, height), || unreachable!(), ui::view)
    };
    let state_jobs = jobs.clone();
    let app = App::new(spec)
        .windowless()
        .init(move || ui::State::new(history, state_jobs))
        .on_focus(ui::on_focus)
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

/// Shows the launcher (centred near the top of `on`, else of the monitor under the pointer),
/// or hides it if it's showing. Any thread.
pub fn toggle(on: Option<Area>) {
    let Some(l) = LAUNCHER.get() else { return };
    if l.app.is_visible() {
        hide();
    } else {
        show(l, on);
    }
}

/// Hides the launcher, forgetting what was typed. Any thread.
pub fn hide() {
    let Some(l) = LAUNCHER.get() else { return };
    l.app.update(|s, _| s.reset());
    l.app.hide();
}

fn show(l: &Launcher, on: Option<Area>) {
    {
        let mut last = l.last_scan.lock().unwrap();
        if last.elapsed() > RESCAN_AFTER {
            *last = Instant::now();
            l.send(Job::Scan);
        }
    }
    l.app.update(|s, _| s.reset());
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
        }
    }
}

/// A linear Rune colour as 8-bit sRGB.
fn srgb(c: Color) -> [u8; 3] {
    let enc = |v: f32| {
        let s = if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
        (s.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    [enc(c.r), enc(c.g), enc(c.b)]
}
