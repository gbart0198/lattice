//! The product tour: a guided walk over the real desktop. A dimmed, click-through overlay cuts a
//! glowing spotlight around one thing at a time (the bar's workspaces, the layout button, the
//! focused window, the launcher and its app, file, command and window searches) and a card
//! beside it says what it is. Steps that teach a key wait for you to press it ("Press Win+2")
//! and move on when lattice reports it happened; every step also has a Next button.
//!
//! It runs as its own process (`lattice --tour`), so the GPU device its translucent overlay
//! needs (tens of MB) goes away with it instead of staying in lattice for good. It learns where
//! things are and what you did by asking lattice's "lattice-control" window (`shared::WM_TOUR_*`);
//! lattice's keyboard hook turns Escape into "end the tour" while it runs, whatever has focus.
//! The overlay never takes the keyboard from your apps.
//!
//! lattice starts it once on its own, the first time it runs without the `tour-seen` marker in
//! its config folder (fresh installs and upgrades from before the tour alike). Finishing or
//! skipping writes the marker. `lattice --tour`, the `tour` command (Win+Shift+/ by default)
//! replay it; starting it while it shows ends it.

mod preview;
mod view;

use std::sync::OnceLock;
use std::time::Duration;

use rune::{
    App, AppSpec, Color, DisplayTarget, Key, KeyEvent, KeyPolicy, OverlayHandle, OverlayInput, OverlayLevel, OverlayOptions,
    OverlayPlacement, OverlaySpec, Tx, WindowSpec,
};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, SMTO_ABORTIFHUNG, SendMessageTimeoutW};
use windows::core::w;

use crate::config;
use crate::shared::{Anchor, TourStatus, WM_TOUR_RECT, WM_TOUR_STATUS, unpack_rect};

/// A rectangle in overlay points.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct R {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl R {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        R { x, y, w, h }
    }

    fn inflate(self, d: f32) -> R {
        R::new(self.x - d, self.y - d, self.w + 2.0 * d, self.h + 2.0 * d)
    }

    fn bottom(self) -> f32 {
        self.y + self.h
    }

    fn center(self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

/// What moves a step on, besides its Next button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wait {
    Next,
    /// The active workspace changes.
    Workspace,
    /// The active workspace's layout changes.
    Layout,
    LauncherOpen,
    LauncherClosed,
}

pub struct Step {
    pub anchor: Option<Anchor>,
    pub eyebrow: &'static str,
    pub title: &'static str,
    pub body: &'static str,
    /// Keycaps for the step's shortcut, and what it does.
    pub keys: &'static [&'static str],
    pub keys_label: &'static str,
    pub wait: Wait,
    /// Said once you've done what a waiting step asks.
    pub done: &'static str,
}

pub const STEPS: &[Step] = &[
    Step {
        anchor: None,
        eyebrow: "Welcome",
        title: "Welcome to lattice",
        body: "lattice tiles your windows, gives you ten workspaces and puts everything a keystroke away. \
               This tour points things out on your real desktop. It takes about a minute.",
        keys: &[],
        keys_label: "",
        wait: Wait::Next,
        done: "",
    },
    Step {
        anchor: Some(Anchor::Bar),
        eyebrow: "The bar",
        title: "Your desktop at a glance",
        body: "Workspaces on the left, the focused window's title in the middle, your system on the right. \
               It stays above everything.",
        keys: &[],
        keys_label: "",
        wait: Wait::Next,
        done: "",
    },
    Step {
        anchor: Some(Anchor::Workspaces),
        eyebrow: "Workspaces",
        title: "Ten desktops, one key away",
        body: "Each pill is a workspace, with icons for the apps on it. Click one, or hold Win and press its number. \
               Add Shift to take the focused window with you.",
        keys: &["Win", "2"],
        keys_label: "switch to workspace 2",
        wait: Wait::Workspace,
        done: "That's it. Win+1 brings you back.",
    },
    Step {
        anchor: Some(Anchor::Focused),
        eyebrow: "Tiling",
        title: "Windows arrange themselves",
        body: "New windows snap into place, and the glow marks the one with focus. Move focus with Win and the arrows; \
               add Shift to swap windows. Dragging one onto another swaps them too.",
        keys: &["Win", "←", "→"],
        keys_label: "move focus",
        wait: Wait::Next,
        done: "",
    },
    Step {
        anchor: Some(Anchor::Layout),
        eyebrow: "Layouts",
        title: "Three ways to tile",
        body: "Dwindle spirals windows inward, master keeps one large window beside a stack, \
               and monocle shows one at a time. Each workspace keeps its own.",
        keys: &["Win", "\\"],
        keys_label: "cycle the layout",
        wait: Wait::Layout,
        done: "Changed. Press it again to keep cycling.",
    },
    Step {
        anchor: Some(Anchor::Stats),
        eyebrow: "System",
        title: "Vitals without a widget",
        body: "CPU, memory, battery and the clock. Choose what shows, the colours and the font in Settings.",
        keys: &["Win", "Alt", ","],
        keys_label: "open Settings",
        wait: Wait::Next,
        done: "",
    },
    Step {
        anchor: None,
        eyebrow: "Launcher",
        title: "Open anything",
        body: "One search box for your apps, files, open windows and commands. \
               The apps you use most rise to the top.",
        keys: &["Win", "Space"],
        keys_label: "open the launcher",
        wait: Wait::LauncherOpen,
        done: "",
    },
    Step {
        anchor: Some(Anchor::Launcher),
        eyebrow: "Apps",
        title: "Type a little, press Enter",
        body: "A few letters are enough: vsc finds Visual Studio Code. The arrows move the selection.",
        keys: &["Enter"],
        keys_label: "open the highlighted app",
        wait: Wait::Next,
        done: "",
    },
    Step {
        anchor: Some(Anchor::Launcher),
        eyebrow: "Files",
        title: "Find any file or folder",
        body: "Type f and a space to search files and folders instead, through Everything when it's running \
               or lattice's own index of your user folder. Backspace on an empty field goes back to apps.",
        keys: &["F", "Space"],
        keys_label: "search files and folders",
        wait: Wait::Next,
        done: "",
    },
    Step {
        anchor: Some(Anchor::Launcher),
        eyebrow: "Commands",
        title: "Run commands by name",
        body: "Type > and a space for layouts, workspaces, floating and system commands: lock, sleep, \
               audio output, Wi-Fi. Arguments are part of the name, so > move 3 finds Move window to workspace 3.",
        keys: &[">", "Space"],
        keys_label: "enter command mode",
        wait: Wait::Next,
        done: "",
    },
    Step {
        anchor: Some(Anchor::Launcher),
        eyebrow: "Windows",
        title: "Jump to any window",
        body: "Win+J searches every open window on every workspace and monitor, most recent first. \
               Enter switches to it, so Win+J, Enter flips back. Here, w and a space does the same.",
        keys: &["Win", "J"],
        keys_label: "search open windows",
        wait: Wait::Next,
        done: "",
    },
    Step {
        anchor: Some(Anchor::Launcher),
        eyebrow: "Launcher",
        title: "Escape puts it away",
        body: "Whatever it's searching, Escape closes the launcher and you're back where you were.",
        keys: &["Esc"],
        keys_label: "close the launcher",
        wait: Wait::LauncherClosed,
        done: "",
    },
    Step {
        anchor: None,
        eyebrow: "All set",
        title: "You're ready",
        body: "Everything lives in Settings and in config.toml. A few keys worth remembering:",
        keys: &[],
        keys_label: "",
        wait: Wait::Next,
        done: "",
    },
];

/// The last step's cheat sheet.
pub const CHEATS: &[(&[&str], &str)] = &[
    (&["Win", "Space"], "Launcher"),
    (&["Win", "J"], "Find a window"),
    (&["Win", "1-9"], "Workspaces"),
    (&["Win", "\\"], "Cycle layout"),
    (&["Win", "Alt", ","], "Settings"),
    (&["Win", "Shift", "/"], "This tour"),
];

/// Colours, from the bar's.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub bg: Color,
    pub fg: Color,
    pub muted: Color,
    pub accent: Color,
    pub surface: Color,
}

impl Palette {
    pub fn from_config(bar: &config::Bar) -> Self {
        let d = config::Bar::default();
        let pick = |set: &str, default: &str| crate::bar::color(set, crate::bar::color(default, Color::default()));
        Palette {
            bg: pick(&bar.background, &d.background),
            fg: pick(&bar.foreground, &d.foreground),
            muted: pick(&bar.muted, &d.muted),
            accent: pick(&bar.accent, &d.accent),
            surface: pick(&bar.occupied, &d.occupied),
        }
    }
}

/// Where the overlay sits on screen: its top-left in physical pixels and its scale.
#[derive(Clone, Copy, Debug)]
pub struct Screen {
    pub x: i32,
    pub y: i32,
    pub scale: f32,
    /// In points.
    pub w: f32,
    pub h: f32,
}

impl Screen {
    fn to_points(self, r: crate::layout::Rect) -> R {
        let s = self.scale;
        R::new((r.x - self.x) as f32 / s, (r.y - self.y) as f32 / s, r.w as f32 / s, r.h as f32 / s)
    }

    /// Mostly on this screen (the bar is on the primary monitor; a window may not be).
    fn shows(self, r: R) -> bool {
        let (cx, cy) = r.center();
        r.w > 0.0 && r.h > 0.0 && cx >= 0.0 && cy >= 0.0 && cx <= self.w && cy <= self.h
    }
}

/// How the tour is going.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    /// The first frame, before the dim fades in.
    Entering,
    Showing,
    /// The step's action happened at this clock. The card shows what happened and stays until
    /// Next, so a step's follow-up ("now go back with Win+1") can't be missed.
    Done(f64),
    /// Fading out since this clock, then the process quits.
    Leaving(f64),
}

pub struct Tour {
    pub step: usize,
    pub phase: Phase,
    pub pal: Palette,
    pub screen: Screen,
    /// lattice answered: interactive steps can wait for real actions.
    pub live: bool,
    /// Where each anchor is now, in points (index: `Anchor as usize`).
    pub anchors: [Option<R>; 6],
    /// What lattice said when the current step began, and most recently.
    base: TourStatus,
    last: TourStatus,
    next_query: f64,
    /// The card's laid-out height, to place it above or below its target.
    pub card_h: f32,
    /// Headless preview: no lattice, no timers.
    pub preview: bool,
}

const QUERY_EVERY: f64 = 0.1;
const LEAVE_FOR: f64 = 0.35;

impl Tour {
    pub fn new(pal: Palette, screen: Screen, live: bool) -> Self {
        Tour {
            step: 0,
            phase: Phase::Entering,
            pal,
            screen,
            live,
            anchors: [None; 6],
            base: TourStatus::default(),
            last: TourStatus::default(),
            next_query: 0.0,
            card_h: 250.0,
            preview: false,
        }
    }

    pub fn current(&self) -> &'static Step {
        &STEPS[self.step.min(STEPS.len() - 1)]
    }

    pub fn anchor_rect(&self) -> Option<R> {
        let a = self.current().anchor?;
        self.anchors[a as usize].filter(|r| self.screen.shows(*r))
    }

    pub fn leaving(&self) -> bool {
        matches!(self.phase, Phase::Leaving(_))
    }

    pub fn go(&mut self, to: usize, tx: &mut Tx) {
        if self.leaving() {
            return;
        }
        let mut to = to;
        // "Open the launcher" only while it's closed, "type in it" only while it's open: step
        // past whichever doesn't fit, the way you were going.
        if self.live && to < STEPS.len() {
            match STEPS[to].wait {
                Wait::LauncherOpen if self.last.launcher => to += 1,
                Wait::LauncherClosed if !self.last.launcher => {
                    if to > self.step {
                        to += 1;
                    } else {
                        to -= 1;
                    }
                }
                _ => {}
            }
        }
        if to >= STEPS.len() {
            self.leave(tx);
            return;
        }
        self.step = to;
        self.base = self.last;
        self.phase = Phase::Showing;
        tx.animated();
    }

    pub fn next(&mut self, tx: &mut Tx) {
        let to = self.step + 1;
        self.go(to, tx);
    }

    pub fn back(&mut self, tx: &mut Tx) {
        if self.step > 0 {
            let to = self.step - 1;
            self.go(to, tx);
        }
    }

    pub fn leave(&mut self, tx: &mut Tx) {
        if !self.leaving() {
            self.phase = Phase::Leaving(tx.clock());
            tx.animated();
            tx.wake_after(Duration::from_secs_f64(LEAVE_FOR));
        }
    }

    /// Asks lattice what changed; true when the view should rebuild.
    fn query(&mut self, tx: &mut Tx) -> bool {
        let Some(h) = control_window() else {
            // lattice quit under us: there's nothing left to point at.
            if self.live {
                self.live = false;
                self.leave(tx);
                return true;
            }
            return false;
        };
        let Some(status) = send(h, WM_TOUR_STATUS, 1).map(TourStatus::unpack) else { return false };
        let mut changed = false;
        if status.escapes != self.last.escapes && self.phase != Phase::Entering {
            self.last = status;
            self.leave(tx);
            return true;
        }
        if self.phase == Phase::Entering {
            self.base = status;
        }
        self.last = status;
        for (i, slot) in self.anchors.iter_mut().enumerate() {
            let Some(a) = Anchor::from_index(i) else { continue };
            let r = send(h, WM_TOUR_RECT, a as usize).and_then(unpack_rect).map(|r| self.screen.to_points(r));
            let r = r.map(|r| R::new(r.x.round(), r.y.round(), r.w.round(), r.h.round()));
            if *slot != r {
                *slot = r;
                changed = true;
            }
        }
        if self.phase == Phase::Showing {
            let did = match self.current().wait {
                Wait::Next => false,
                Wait::Workspace => status.workspace != self.base.workspace,
                Wait::Layout => status.layout != self.base.layout,
                Wait::LauncherOpen => status.launcher,
                Wait::LauncherClosed => !status.launcher && self.base.launcher,
            };
            if did {
                match self.current().wait {
                    // The launcher took the screen: point at it straight away.
                    Wait::LauncherOpen | Wait::LauncherClosed => self.next(tx),
                    _ => {
                        self.phase = Phase::Done(tx.clock());
                        tx.animated();
                    }
                }
                changed = true;
            }
        }
        changed
    }
}

fn control_window() -> Option<HWND> {
    unsafe { FindWindowW(w!("lattice-display-listener"), w!("lattice-control")).ok().filter(|h| !h.is_invalid()) }
}

/// One question to lattice's control window; None when it doesn't answer in time.
fn send(h: HWND, msg: u32, wp: usize) -> Option<u64> {
    let mut result = 0usize;
    let ok = unsafe { SendMessageTimeoutW(h, msg, WPARAM(wp), LPARAM(0), SMTO_ABORTIFHUNG, 250, Some(&mut result)) };
    (ok.0 != 0).then_some(result as u64)
}

fn poll(s: &mut Tour, t: f64, tx: &mut Tx) -> bool {
    if s.preview {
        return false;
    }
    let mut changed = false;
    if QUIT.get().is_some_and(|q| q.signaled()) {
        s.leave(tx);
        changed = true;
    }
    match s.phase {
        Phase::Leaving(at) => {
            if t - at >= LEAVE_FOR {
                finish(s.live);
                // The overlay belongs to a windowless host app that would keep running; nothing
                // else here needs a clean shutdown.
                std::process::exit(0);
            } else {
                tx.wake_after(Duration::from_secs_f64(LEAVE_FOR - (t - at)));
            }
            return changed;
        }
        _ => {}
    }
    if t >= s.next_query {
        s.next_query = t + QUERY_EVERY;
        if s.live || s.phase == Phase::Entering {
            changed |= s.query(tx);
        }
    }
    if s.phase == Phase::Entering {
        // Fade in from the second frame, so the first frame lands clear.
        s.phase = Phase::Showing;
        tx.animated();
        changed = true;
    }
    tx.wake_after(Duration::from_secs_f64(QUERY_EVERY));
    changed
}

fn on_key(s: &mut Tour, ev: &KeyEvent, tx: &mut Tx) -> bool {
    match ev.key {
        Key::Escape => s.leave(tx),
        Key::Enter | Key::Right => s.next(tx),
        Key::Left => s.back(tx),
        _ => return false,
    }
    true
}

// ---------------------------------------------------------------------------------------------
// The process

/// The marker that says the tour has been seen (finished or skipped).
fn marker() -> std::path::PathBuf {
    config::config_dir().join("tour-seen")
}

/// Whether lattice should start the tour by itself.
pub fn seen() -> bool {
    marker().exists()
}

/// The tour ended: remember it, and tell lattice to stop routing Escape to it.
fn finish(live: bool) {
    let _ = std::fs::create_dir_all(config::config_dir());
    let _ = std::fs::write(marker(), "The lattice tour was shown. Delete this file to see it again on the next start.\n");
    if live && let Some(h) = control_window() {
        let _ = send(h, WM_TOUR_STATUS, 0);
    }
}

/// Starts the tour process (lattice's `tour` command, and its first run). Starting it while it
/// shows ends it.
pub fn start() {
    unsafe {
        // We're handling the user's key press: let the tour come to the front.
        let _ = windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(
            windows::Win32::UI::WindowsAndMessaging::ASFW_ANY,
        );
    }
    match std::env::current_exe() {
        Ok(exe) => {
            if let Err(e) = std::process::Command::new(exe).arg("--tour").env_remove(crate::CPU_FIRST_ENV).spawn() {
                log::error!("start tour: {e}");
            }
        }
        Err(e) => log::error!("start tour: {e}"),
    }
}

/// Starts the tour after lattice's own windows have settled, unless it has been seen.
pub fn start_if_new() {
    if seen() {
        return;
    }
    let _ = std::thread::Builder::new().name("lattice-tour-start".into()).spawn(|| {
        std::thread::sleep(Duration::from_millis(1500));
        log::info!("first run: starting the tour");
        start();
    });
}

/// A named event another `--tour` sets to end this one.
struct QuitEvent(windows::Win32::Foundation::HANDLE);

// SAFETY: an event handle may be waited on from any thread.
unsafe impl Send for QuitEvent {}
unsafe impl Sync for QuitEvent {}

impl QuitEvent {
    fn signaled(&self) -> bool {
        use windows::Win32::System::Threading::WaitForSingleObject;
        unsafe { WaitForSingleObject(self.0, 0) }.0 == 0
    }
}

static QUIT: OnceLock<QuitEvent> = OnceLock::new();

/// `lattice --tour`: shows the tour over the desktop until it's finished or skipped.
pub fn run(args: &[String]) {
    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::{CreateEventW, CreateMutexW, SetEvent};
    use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};

    if args.iter().any(|a| a == "--tour-preview") {
        preview::run();
        return;
    }
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let quit = CreateEventW(None, true, false, w!("Local\\lattice-tour-quit"));
        let mutex = CreateMutexW(None, true, w!("Local\\lattice-tour"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            // One is showing: this asks it to end.
            if let Ok(e) = quit {
                let _ = SetEvent(e);
            }
            return;
        }
        if let Ok(e) = quit {
            let _ = QUIT.set(QuitEvent(e));
        }
        std::mem::forget(mutex);
    }

    let cfg = config::load();
    let pal = Palette::from_config(&cfg.bar);
    let families = crate::bar::font_families(&cfg.bar.font);
    let live = control_window().is_some();

    rune::run_windows(move |displays| {
        let display = displays.iter().find(|d| d.primary).or(displays.first());
        let (pos, size, scale) = display.map_or(((0, 0), (1920, 1080), 1.0), |d| (d.position, d.size, d.scale as f32));
        let screen = Screen {
            x: pos.0,
            y: pos.1,
            scale,
            w: size.0 as f32 / scale,
            h: size.1 as f32 / scale,
        };
        let state = Tour::new(pal, screen, live);
        let mut app = App::new(AppSpec::<()> {
            families,
            ..AppSpec::new("lattice tour host", (1.0, 1.0), || (), |_, _| rune::el())
        })
        .windowless();
        // Without lattice there's no hook to hear Escape, so the tour takes the keyboard itself.
        let keys = if live { KeyPolicy::None } else { KeyPolicy::OnDemand };
        let overlay: OverlayHandle<Tour> = app.overlay(
            OverlaySpec::new("lattice tour", (screen.w, screen.h), view::view)
                .options(OverlayOptions {
                    display: DisplayTarget::Primary,
                    placement: Some(OverlayPlacement::FullScreen),
                    level: OverlayLevel::AboveStatus,
                    input: OverlayInput::Regions,
                    keys,
                    exclude_from_capture: false,
                    ..Default::default()
                })
                .poll(poll)
                .on_key(on_key),
            state,
        );
        overlay.show();
        if !live {
            overlay.request_keys();
        }
        vec![WindowSpec::from_app("tour-host", app).options(rune::WindowOptions { transparent: true, ..Default::default() })]
    });
}

#[cfg(test)]
mod tests;
