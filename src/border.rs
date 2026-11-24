//! The focus border: a transparent, click-through, always-on-top Rune window that draws a
//! rounded, glowing frame. Rune only draws it; the WM thread positions it around the focused
//! window with Win32 (`win::place_overlay`), so it follows focus and drags with no round trip.

use std::sync::OnceLock;

use rune::{App, AppSpec, Cx, El, WindowOptions, WindowSpec, WindowsWindowOptions, el};

use crate::config;

/// Window title the WM thread uses to find the overlay's HWND.
pub const TITLE: &str = "lattice-border";

static CFG: OnceLock<config::Border> = OnceLock::new();

pub struct State;

fn view(_: &State, _: &mut Cx<State>) -> El {
    let c = CFG.get().expect("border config set");
    let color = crate::bar::color(&c.color, rune::Color { r: 0.25, g: 0.45, b: 0.95, a: 1.0 });
    let m = c.margin();
    let mut root = el().w_full().h_full();
    // The glow: concentric 1px rings outside the frame, fading with distance. (A box shadow
    // would also fill the inside of the frame and tint the focused window.)
    if c.glow > 0.0 && c.glow_opacity > 0.0 {
        let n = c.glow.ceil() as i32;
        for d in (1..=n).rev() {
            let t = d as f32 / (n as f32 + 1.0);
            let a = c.glow_opacity * (1.0 - t).powi(2);
            let inset = m - d as f32;
            root = root.child(ring(inset, c.radius + d as f32, 1.0, color.alpha(a)));
        }
        // A faint inner falloff makes the edge read as lit rather than outlined.
        let inner = (c.glow * 0.3).ceil() as i32;
        for d in 1..=inner {
            let a = c.glow_opacity * 0.4 * (1.0 - d as f32 / (inner as f32 + 1.0)).powi(2);
            let inset = m + c.width + (d - 1) as f32;
            root = root.child(ring(inset, (c.radius - c.width - (d - 1) as f32).max(0.0), 1.0, color.alpha(a)));
        }
    }
    // The frame line sits exactly on the window's edge.
    if c.width > 0.0 {
        root = root.child(ring(m, c.radius, c.width, color));
    }
    root
}

/// A rounded, unfilled rectangle `inset` points in from every edge of the overlay.
fn ring(inset: f32, radius: f32, width: f32, color: rune::Color) -> El {
    el().pin(Some(inset), Some(inset), Some(inset), Some(inset)).radius(radius).border(width, color)
}

pub fn spec(cfg: config::Border) -> WindowSpec<State> {
    let _ = CFG.set(cfg);
    let options = WindowOptions {
        transparent: true,
        borderless: true,
        click_through: true,
        always_on_top: true,
        resizable: false,
        // Parked as a 1x1 window until the WM thread moves it onto the focused window.
        position: Some((0, 0)),
        physical_size: Some((1, 1)),
        min_size: Some((0.0, 0.0)),
        ..Default::default()
    };
    let app = App::new(AppSpec { min_size: Some((0.0, 0.0)), ..AppSpec::new(TITLE, (1.0, 1.0), || State, view) })
        .window_options(options)
        .windows_window(WindowsWindowOptions::default().tool_window(true).no_activate(true).exclude_from_capture(false))
        .open_inactive();
    WindowSpec::from_app("border", app)
}
