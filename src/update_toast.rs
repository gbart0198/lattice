//! The "update available" toast: a small always-on-top window at the bottom right of the
//! primary monitor (above the taskbar), with **Ignore** and **Update & restart**. It shows
//! itself when `update` finds a newer release and hides again once that version is ignored.
//!
//! Ignoring remembers the version (`update-ignored.txt` next to the config), so the daily
//! check doesn't bring the same toast back; the next release shows again, and settings'
//! Updates section can still install an ignored one.

use std::sync::{Arc, OnceLock};

use rune::{Align, App, AppHandle, AppSpec, Color, Cx, DisplayTarget, El, OverlayPlacement, ScreenEdge, TextStyle, WindowOptions, WindowSpec, WindowsWindowOptions, col, row, text};

use crate::launcher::Style;
use crate::update::{self, Status};

const W: f32 = 430.0;
const H: f32 = 84.0;
/// Gap to the work area's right and bottom edges, in points.
const MARGIN: f32 = 16.0;

static STYLE: OnceLock<Style> = OnceLock::new();
static APP: OnceLock<AppHandle<State>> = OnceLock::new();

#[derive(Default)]
pub struct State {
    status: Status,
}

fn ignore_file() -> std::path::PathBuf {
    crate::config::config_dir().join("update-ignored.txt")
}

fn ignored() -> Option<update::Version> {
    std::fs::read_to_string(ignore_file()).ok().and_then(|s| update::parse_version(s.trim()))
}

/// Whether the toast has something to say about `status`.
fn wanted(status: &Status) -> bool {
    match status {
        Status::Available(r) => ignored() != Some(r.version),
        Status::Downloading(_) | Status::Installing(_) | Status::InstallFailed(_, _) => true,
        _ => false,
    }
}

/// Copies the update status in and shows or hides the toast to match. Any thread.
fn sync() {
    let Some(app) = APP.get() else { return };
    let status = update::status();
    let show = wanted(&status);
    app.update(move |s, _| s.status = status);
    if show && !app.is_visible() {
        app.place(
            DisplayTarget::Primary,
            OverlayPlacement::Edge { edge: ScreenEdge::Bottom, margin: MARGIN, align: Align::End, inset: MARGIN },
        );
        // Shown without taking focus from what you're doing.
        app.show();
    } else if !show && app.is_visible() {
        app.hide();
    }
}

fn ignore(s: &mut State) {
    if let Status::Available(r) | Status::InstallFailed(r, _) = &s.status
        && !update::is_preview()
    {
        let dir = crate::config::config_dir();
        let _ = std::fs::create_dir_all(&dir);
        if let Err(e) = std::fs::write(ignore_file(), r.version_str()) {
            log::warn!("couldn't remember the ignored update: {e}");
        }
        log::info!("update {} ignored", r.version_str());
    }
    if let Some(app) = APP.get() {
        app.hide();
    }
}

fn style() -> &'static Style {
    STYLE.get().expect("toast style set")
}

fn button(key: &str, label: &str, fill: Color, color: Color, on_press: fn(&mut State)) -> El {
    row()
        .key(key.to_string())
        .role("button")
        .label(label.to_string())
        .hit()
        .h(30.0)
        .px(14.0)
        .center()
        .radius(15.0)
        .fill(fill)
        .on_press(on_press)
        .child(text(label.to_string()).font(TextStyle::new(12.5, 650.0)).color(color))
}

fn view(s: &State, _cx: &mut Cx<State>) -> El {
    let t = style();
    let (title, detail, buttons): (String, String, Vec<El>) = match &s.status {
        Status::Available(r) => (
            "Update available".into(),
            format!("lattice {}", r.version_str()),
            vec![
                button("ignore", "Ignore", t.surface, t.foreground, ignore),
                button("install", "Update & restart", t.accent, t.background, |_| update::install_async()),
            ],
        ),
        Status::Downloading(r) => (format!("Downloading lattice {}…", r.version_str()), "Checking it, then installing".into(), vec![]),
        Status::Installing(r) => (format!("Installing lattice {}", r.version_str()), "lattice restarts in a moment".into(), vec![]),
        Status::InstallFailed(_, e) => (
            "Update failed".into(),
            e.clone(),
            vec![
                button("ignore", "Ignore", t.surface, t.foreground, ignore),
                button("retry", "Try again", t.accent, t.background, |_| update::install_async()),
            ],
        ),
        _ => (String::new(), String::new(), vec![]),
    };
    let words = col()
        .grow(1.0)
        .shrink()
        .gap(3.0)
        .child(text(title).truncate().font(TextStyle::new(14.0, 650.0)).color(t.foreground))
        .child(text(detail).truncate().font(TextStyle::new(12.0, 450.0)).color(t.muted));
    row()
        .key("toast")
        .w_full()
        .h_full()
        .px(16.0)
        .gap(10.0)
        .center_y()
        .fill(t.background)
        .child(words)
        .children(buttons)
}

/// The toast's window, hidden until an update turns up. Call once, in the WM process.
pub fn window(style: Style, families: (&'static str, &'static str)) -> rune::AnyWindowSpec {
    let border = crate::launcher::srgb(style.surface);
    let _ = STYLE.set(style);
    let app = App::new(AppSpec::<State> { families, min_size: Some((1.0, 1.0)), ..AppSpec::new("lattice update", (W, H), State::default, view) })
        .windowless()
        .on_window(move |w| {
            use rune::raw_window_handle::RawWindowHandle;
            if let Ok(hwnd) = w.with_raw_handle(|h| match h.as_raw() {
                RawWindowHandle::Win32(h) => h.hwnd.get(),
                _ => 0,
            }) && hwnd != 0
            {
                crate::launcher::style_window(hwnd, border);
            }
        });
    let _ = APP.set(app.handle());
    update::set_waker(Arc::new(sync));
    let options = WindowOptions {
        visible: false,
        borderless: true,
        always_on_top: true,
        resizable: false,
        min_size: Some((0.0, 0.0)),
        ..Default::default()
    }
    .windows(WindowsWindowOptions::default().tool_window(true).no_activate(true).undecorated_shadow(true));
    WindowSpec::from_app("update-toast", app).configured_options(options).into()
}

/// `--update-preview`: the toast with a made-up 9.9.9 release (Update does nothing here).
pub fn preview(style: Style, families: (&'static str, &'static str)) {
    let spec = window(style, families);
    update::preview(Status::Available(update::Release::preview((9, 9, 9))));
    rune::run_windows(move |_displays| vec![spec]);
}
