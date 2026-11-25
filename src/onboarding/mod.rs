//! The welcome tour (`lattice.exe --onboarding`): a short walkthrough of what lattice does and
//! the keys that do it, as a sequence of animated slides.
//!
//! It runs as its own process, like the settings window, so it floats above everything untiled.
//! lattice opens it once, on the first start without the `onboarded` marker in the config
//! folder; finishing, skipping or closing the tour writes the marker. `--onboarding`, the
//! `onboarding` command and the settings window's "Show tour" button open it at any time.

mod parts;
mod scenes;

use std::sync::OnceLock;

use rune::{App, AppSpec, Color, ColorScheme, Cx, El, Key, KeyEvent, Spring, TextStyle, Titlebar, Transition, Tx, WindowOptions, col, el, row, text};

use crate::config;

const TITLE: &str = "Welcome to lattice";
/// How often a slide's illustration takes its next step, in seconds.
const TICK: f64 = 0.45;
const WIN_W: f32 = 960.0;
const WIN_H: f32 = 690.0;
const BAND: f32 = 48.0;

/// Colours, from the bar's (the launcher and settings wear them too).
pub struct Pal {
    pub bg: Color,
    pub fg: Color,
    pub muted: Color,
    pub accent: Color,
    pub accent2: Color,
    pub occupied: Color,
    /// The window's own background, a shade under the bar's.
    pub base: Color,
    /// Mock windows and cards.
    pub win: Color,
    pub key: Color,
    pub accent_hex: String,
}

static PAL: OnceLock<Pal> = OnceLock::new();

fn pal() -> &'static Pal {
    PAL.get_or_init(|| palette(&config::load().bar))
}

fn palette(bar: &config::Bar) -> Pal {
    let fallback = config::Bar::default();
    let pick = |set: &str, default: &str| crate::bar::color(set, crate::bar::color(default, Color::default()));
    let bg = pick(&bar.background, &fallback.background);
    let fg = pick(&bar.foreground, &fallback.foreground);
    let accent = pick(&bar.accent, &fallback.accent);
    let occupied = pick(&bar.occupied, &fallback.occupied);
    Pal {
        bg,
        fg,
        muted: pick(&bar.muted, &fallback.muted),
        accent,
        // A second hue for gradients: the accent turned toward violet.
        accent2: accent.mix(crate::bar::color("#cba6f7", accent), 0.7),
        occupied,
        base: bg.mix(Color::BLACK, 0.35),
        win: bg.mix(occupied, 0.45),
        key: occupied.mix(fg, 0.06),
        accent_hex: if crate::bar::color(bar.accent.trim(), Color::TRANSPARENT) == Color::TRANSPARENT {
            fallback.accent
        } else {
            bar.accent.trim().to_string()
        },
    }
}

// ---------------------------------------------------------------------------------------------
// Slides

struct Slide {
    eyebrow: &'static str,
    title: &'static str,
    body: &'static str,
    hints: &'static [(&'static [&'static str], &'static str)],
}

const SLIDES: [Slide; 8] = [
    Slide {
        eyebrow: "Welcome",
        title: "Welcome to lattice",
        body: "A tiling window manager for Windows 11 that you drive from the keyboard. This tour shows you the essentials in about a minute.",
        hints: &[(&["→"], "Next"), (&["Esc"], "Skip the tour")],
    },
    Slide {
        eyebrow: "Auto-tiling",
        title: "Windows arrange themselves",
        body: "Open an app and it snaps into place, no dragging. Dwindle spirals inward, master keeps one big window beside a stack, and monocle gives every window the whole screen.",
        hints: &[(&["Win", "\\"], "Cycle layout"), (&["Win", "Shift", "Space"], "Float a window")],
    },
    Slide {
        eyebrow: "Workspaces",
        title: "Ten desktops, one keystroke away",
        body: "Workspaces span all your monitors and switch instantly. Keep chat on 3 and music on 4, out of the way until you need them.",
        hints: &[(&["Win", "1…9"], "Switch"), (&["Win", "Shift", "1…9"], "Send the window there")],
    },
    Slide {
        eyebrow: "Keybinds",
        title: "Your hands stay on the keyboard",
        body: "Every action has a shortcut on the Windows key, and every shortcut can be changed in Settings.",
        hints: &[(&["Win", "Shift", "R"], "Reload the config"), (&["Win", "Shift", "E"], "Quit lattice")],
    },
    Slide {
        eyebrow: "Launcher",
        title: "Open anything in a few letters",
        body: "Type part of an app's name and press Enter. The apps you use most rise to the top. Start with f and a space to search files and folders instead.",
        hints: &[(&["Win", "Space"], "Open the launcher"), (&["f", "Space"], "Search files")],
    },
    Slide {
        eyebrow: "Bar & focus",
        title: "Always know where you are",
        body: "The bar shows each workspace with its apps, the focused window's title, CPU, memory and the time. A soft glow follows the window you're typing into.",
        hints: &[(&["Win", "←", "→", "↑", "↓"], "Move focus"), (&["Win", "F"], "Fullscreen")],
    },
    Slide {
        eyebrow: "Settings",
        title: "Make it yours",
        body: "Layouts, gaps, colours, fonts, the focus glow, app workspaces and every keybind live in one window. Saving applies changes right away.",
        hints: &[(&["Win", "Alt", ","], "Open settings")],
    },
    Slide {
        eyebrow: "Ready",
        title: "You're all set",
        body: "lattice is already tiling your windows. Replay this tour any time from Settings, or by running lattice --onboarding.",
        hints: &[],
    },
];

const LAST: usize = SLIDES.len() - 1;

// ---------------------------------------------------------------------------------------------
// State

struct S {
    slide: usize,
    /// +1 after moving forward, -1 after moving back: which way the text slides.
    dir: f32,
    /// Beats since this slide appeared; drives its illustration.
    tick: u32,
    next_tick: Option<f64>,
}

fn init() -> S {
    let _ = pal();
    S { slide: 0, dir: 1.0, tick: 0, next_tick: None }
}

fn go(s: &mut S, to: usize) {
    let to = to.min(LAST);
    if to == s.slide {
        return;
    }
    s.dir = if to > s.slide { 1.0 } else { -1.0 };
    s.slide = to;
    s.tick = 0;
    s.next_tick = None;
}

/// Ends the tour (finished or skipped): remember it, start lattice if it isn't running yet
/// (the tour was opened on its own, e.g. `lattice.exe --onboarding`), and close.
fn done(tx: &mut Tx) {
    mark_done();
    start_lattice_if_stopped();
    tx.quit();
}

/// Starts the window manager unless one is already running (its control window exists).
fn start_lattice_if_stopped() {
    if crate::settings::control_window().is_some() {
        return;
    }
    match std::env::current_exe() {
        Ok(exe) => {
            if let Err(e) = std::process::Command::new(exe).env_remove(crate::CPU_FIRST_ENV).spawn() {
                log::error!("start lattice after the tour: {e}");
            }
        }
        Err(e) => log::error!("start lattice after the tour: {e}"),
    }
}

fn next(s: &mut S, tx: &mut Tx) {
    if s.slide == LAST {
        done(tx);
    } else {
        go(s, s.slide + 1);
    }
}

fn poll(s: &mut S, clock: f64, tx: &mut Tx) -> bool {
    let due = *s.next_tick.get_or_insert(clock + TICK);
    if clock + 1e-6 < due {
        tx.wake_at(due);
        return false;
    }
    s.tick = s.tick.saturating_add(1);
    let next = if due + TICK > clock { due + TICK } else { clock + TICK };
    s.next_tick = Some(next);
    tx.wake_at(next);
    // Ticks come from the clock, not from input: ask for the springs anyway.
    tx.animated();
    true
}

fn on_key(s: &mut S, ev: &KeyEvent, tx: &mut Tx) -> bool {
    match &ev.key {
        Key::Right | Key::Enter | Key::PageDown => next(s, tx),
        Key::Text(t) if t == " " => next(s, tx),
        Key::Left | Key::PageUp | Key::Backspace => go(s, s.slide.saturating_sub(1)),
        Key::Home => go(s, 0),
        Key::End => go(s, LAST),
        Key::Escape => done(tx),
        _ => return false,
    }
    true
}

// ---------------------------------------------------------------------------------------------
// View

fn button(p: &Pal, key: &str, label: &str, primary: bool) -> El {
    let (fill, fg) = if primary { (p.accent, p.bg) } else { (p.fg.alpha(0.06), p.fg) };
    let mut b = row()
        .key(key.to_string())
        .role("button")
        .label(label.to_string())
        .focusable(true)
        .h(40.0)
        .px(20.0)
        .center()
        .radius(20.0)
        .fill(fill)
        .hover_fill(if primary { p.accent.mix(Color::WHITE, 0.12) } else { p.fg.alpha(0.10) })
        .press_scale(0.96)
        .motion(Spring::SNAPPY)
        .child(text(label.to_string()).font(TextStyle::new(14.0, 650.0)).color(fg));
    if primary {
        b = b.shadow(6.0, 22.0, p.accent.alpha(0.35));
    } else {
        b = b.border(1.0, p.fg.alpha(0.08));
    }
    b
}

fn titlebar(p: &Pal) -> El {
    let skip = row()
        .key("skip")
        .role("button")
        .label("Skip the tour")
        .h(30.0)
        .pl(12.0)
        .pr(4.0)
        .gap(8.0)
        .center_y()
        .radius(15.0)
        .hover_fill(p.fg.alpha(0.06))
        .on_press_tx(|_s: &mut S, tx: &mut Tx| done(tx))
        .child(text("Skip tour").font(TextStyle::new(12.5, 600.0)).color(p.muted))
        .child(row().h(22.0).px(7.0).center().radius(6.0).fill(p.fg.alpha(0.07)).child(text("Esc").font(TextStyle::new(10.5, 650.0)).color(p.muted)));
    row()
        .key("band")
        .w_full()
        .h(BAND)
        .px(20.0)
        .gap(10.0)
        .center_y()
        .window_drag()
        .child(parts::logo(p, 20.0, 15.0, 4))
        .child(text("lattice").font(TextStyle::new(14.0, 720.0)).color(p.fg))
        .child(el().grow(1.0))
        .child(skip)
}

fn caption(p: &Pal, s: &S) -> El {
    let sl = &SLIDES[s.slide];
    let mut hints = row().gap(22.0).center_y();
    for (keys, what) in sl.hints {
        hints = hints.child(parts::hint(p, keys, what));
    }
    let mut c = col()
        .key(format!("caption-{}", s.slide))
        .w_full()
        .items_center()
        .gap(10.0)
        .transition(Transition::asymmetric(
            Transition::offset(36.0 * s.dir, 0.0).combined(Transition::opacity()),
            Transition::offset(-36.0 * s.dir, 0.0).combined(Transition::opacity()),
        ))
        .child(
            text(format!("{:02}  ·  {}", s.slide + 1, sl.eyebrow.to_uppercase()))
                .font(TextStyle::new(11.5, 720.0).tracking(1.6))
                .color(p.accent),
        )
        .child(text(sl.title.to_string()).font(TextStyle::new(30.0, 760.0).lh(38.0).tracking(-0.4)).color(p.fg))
        .child(
            text(sl.body.to_string())
                .wrap()
                .max_w(600.0)
                .align(rune::Align::Center)
                .font(TextStyle::new(15.0, 450.0).lh(23.0))
                .color(p.muted),
        );
    if s.slide == 6 {
        c = c.child(
            row().pt(6.0).gap(18.0).center_y().child(parts::hint(p, &["Win", "Alt", ","], "Open settings")).child(
                button(p, "open-settings", "Open settings now", false).h(32.0).px(14.0).on_press(|_s: &mut S| open_settings()),
            ),
        );
    } else if !sl.hints.is_empty() {
        c = c.child(hints.pt(6.0));
    }
    c
}

fn dots(p: &Pal, s: &S) -> El {
    let mut r = row().gap(6.0).center_y();
    for (i, slide) in SLIDES.iter().enumerate() {
        let on = i == s.slide;
        r = r.child(
            row()
                .key(format!("dot-{i}"))
                .role("button")
                .label(format!("Slide {}: {}", i + 1, slide.eyebrow))
                .h(16.0)
                .w(if on { 28.0 } else { 12.0 })
                .center()
                .on_press(move |s: &mut S| go(s, i))
                .motion(Spring::SNAPPY)
                .child(
                    el().h(6.0)
                        .w(if on { 28.0 } else { 6.0 })
                        .radius(3.0)
                        .fill(if on { p.accent } else if i < s.slide { p.fg.alpha(0.35) } else { p.fg.alpha(0.14) })
                        .motion(Spring::SNAPPY),
                ),
        );
    }
    r
}

fn footer(p: &Pal, s: &S) -> El {
    let back = button(p, "back", "Back", false).on_press(|s: &mut S| go(s, s.slide.saturating_sub(1)));
    let back = if s.slide == 0 { back.opacity(0.0).inert() } else { back };
    let label = match s.slide {
        0 => "Take the tour",
        LAST => "Start using lattice",
        _ => "Continue",
    };
    let next_btn = button(p, "next", label, true).on_press_tx(|s: &mut S, tx: &mut Tx| next(s, tx));
    row()
        .key("footer")
        .w_full()
        .h(84.0)
        .px(40.0)
        .gap(12.0)
        .center_y()
        .child(dots(p, s))
        .child(el().grow(1.0))
        .child(back)
        .child(next_btn)
}

fn view(s: &S, _cx: &mut Cx<S>) -> El {
    let p = pal();
    let stage = el()
        .key("stage")
        .size(scenes::W, scenes::H)
        .radius(20.0)
        .clip_rounded(20.0)
        .border(1.0, p.fg.alpha(0.08))
        .shadow(20.0, 60.0, Color::BLACK.alpha(0.45))
        .child(
            el().key(format!("scene-{}", s.slide))
                .absolute(0.0, 0.0)
                .size(scenes::W, scenes::H)
                .transition(Transition::opacity().combined(Transition::scale(0.98)))
                .child(scenes::scene(p, s.slide, s.tick)),
        );
    let body = col()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .items_center()
        .gap(30.0)
        .pt(4.0)
        .child(stage)
        .child(el().w_full().child(caption(p, s)));
    col()
        .key("window")
        .w_full()
        .h_full()
        .fill(p.base)
        .radial_gradient([0.5, 0.0], [WIN_W * 0.75, WIN_H * 0.7], [(0.0, p.accent.alpha(0.10)), (1.0, p.accent.alpha(0.0))])
        .child(titlebar(p))
        .child(body)
        .child(footer(p, s))
}

// ---------------------------------------------------------------------------------------------
// First run and opening it

/// The marker that says this user has seen (or skipped) the tour.
pub fn marker_path() -> std::path::PathBuf {
    config::config_dir().join("onboarded")
}

fn mark_done() {
    let path = marker_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&path, env!("CARGO_PKG_VERSION")) {
        log::warn!("could not write {}: {e}", path.display());
    }
}

/// Called as lattice starts: opens the tour once, for someone who hasn't seen it, after the
/// windows they already have open are tiled.
pub fn open_if_new() {
    if marker_path().exists() {
        return;
    }
    log::info!("no {} yet: opening the welcome tour", marker_path().display());
    let _ = std::thread::Builder::new().name("lattice-onboarding".into()).spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(1200));
        open();
    });
}

/// Opens the tour in its own process (it brings an open tour to the front instead).
pub fn open() {
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(windows::Win32::UI::WindowsAndMessaging::ASFW_ANY);
    }
    match std::env::current_exe() {
        Ok(exe) => {
            if let Err(e) = std::process::Command::new(exe).arg("--onboarding").env_remove(crate::CPU_FIRST_ENV).spawn() {
                log::error!("open onboarding: {e}");
            }
        }
        Err(e) => log::error!("open onboarding: {e}"),
    }
}

fn open_settings() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).arg("--settings").env_remove(crate::CPU_FIRST_ENV).spawn();
    }
}

/// `lattice.exe --onboarding`: shows the tour, whether or not it has been seen.
pub fn run() {
    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, SW_RESTORE, SetForegroundWindow, ShowWindow};
    use windows::core::{HSTRING, w};
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let guard = CreateMutexW(None, true, w!("Local\\lattice-onboarding"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if let Ok(h) = FindWindowW(None, &HSTRING::from(TITLE))
                && !h.is_invalid()
            {
                let _ = ShowWindow(h, SW_RESTORE);
                let _ = SetForegroundWindow(h);
            }
            return;
        }
        std::mem::forget(guard);
    }
    let cfg = config::load();
    let _ = PAL.set(palette(&cfg.bar));

    let monitors = crate::win::monitors();
    let primary = monitors.iter().find(|m| m.primary).or(monitors.first());
    let position = primary.map(|m| {
        let (pw, ph) = ((WIN_W * m.scale) as i32, (WIN_H * m.scale) as i32);
        (m.work.x + (m.work.w - pw).max(0) / 2, m.work.y + (m.work.h - ph).max(0) / 2)
    });
    App::new(AppSpec {
        poll: Some(poll),
        on_key: Some(on_key),
        families: crate::bar::font_families(&cfg.bar.font),
        // Headless previews: `--snap x.png --scene launcher --at 4`.
        scenes: &[
            ("welcome", |s: &mut S| s.slide = 0),
            ("tiling", |s: &mut S| s.slide = 1),
            ("workspaces", |s: &mut S| s.slide = 2),
            ("keybinds", |s: &mut S| s.slide = 3),
            ("launcher", |s: &mut S| s.slide = 4),
            ("bar", |s: &mut S| s.slide = 5),
            ("settings", |s: &mut S| s.slide = 6),
            ("finish", |s: &mut S| s.slide = 7),
        ],
        ..AppSpec::new(TITLE, (WIN_W, WIN_H), init, view)
    })
    .titlebar(Titlebar::new(BAND).controls(false))
    .window_options(WindowOptions { always_on_top: true, resizable: false, position, ..Default::default() })
    .color_scheme(ColorScheme::Dark)
    // Closing the window counts as skipping.
    .on_close(|_s, _tx| mark_done())
    .run();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dwindle_halves_along_the_longer_side() {
        let r = parts::dwindle(3, (0.0, 0.0, 100.0, 50.0), 0.0);
        assert_eq!(r, vec![(0.0, 0.0, 50.0, 50.0), (50.0, 0.0, 50.0, 25.0), (50.0, 25.0, 50.0, 25.0)]);
        assert_eq!(parts::dwindle(1, (0.0, 0.0, 10.0, 10.0), 2.0), vec![(0.0, 0.0, 10.0, 10.0)]);
    }

    #[test]
    fn master_stacks_the_rest_on_the_right() {
        let r = parts::master(3, (0.0, 0.0, 100.0, 40.0), 0.0);
        assert_eq!(r[0], (0.0, 0.0, 55.0, 40.0));
        assert_eq!(r[1], (55.0, 0.0, 45.0, 20.0));
        assert_eq!(r[2], (55.0, 20.0, 45.0, 20.0));
    }

    #[test]
    fn navigation_stays_in_range() {
        let mut s = S { slide: 0, dir: 1.0, tick: 5, next_tick: Some(1.0) };
        go(&mut s, 3);
        assert_eq!((s.slide, s.tick, s.dir), (3, 0, 1.0));
        go(&mut s, 99);
        assert_eq!(s.slide, LAST);
        go(&mut s, 1);
        assert_eq!(s.dir, -1.0);
    }
}
