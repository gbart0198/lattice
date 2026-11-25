//! Building blocks the onboarding slides draw with: keycaps, a miniature desktop with mock
//! windows, the bar, and the layout maths that places windows the way lattice does.

use rune::{Color, El, Spring, TextStyle, Transition, col, el, row, text};

use super::Pal;

/// A rectangle as (x, y, width, height), in points within its parent.
pub type R = (f32, f32, f32, f32);

/// How mock windows glide to a new tile.
pub const TILE: Spring = Spring::new(0.55, 0.12);
/// Glow and highlights that follow focus.
pub const GLIDE: Spring = Spring::new(0.45, 0.08);

/// A pretend app: its name and the colour of its badge.
#[derive(Clone, Copy)]
pub struct App {
    pub name: &'static str,
    pub title: &'static str,
    pub tint: &'static str,
}

pub const APPS: [App; 6] = [
    App { name: "Terminal", title: "Terminal - pwsh", tint: "#a6e3a1" },
    App { name: "Code", title: "main.rs - lattice - Visual Studio Code", tint: "#89b4fa" },
    App { name: "Browser", title: "lattice - GitHub", tint: "#fab387" },
    App { name: "Notes", title: "Ideas.md - Notes", tint: "#f9e2af" },
    App { name: "Music", title: "Now playing - Music", tint: "#f5c2e7" },
    App { name: "Files", title: "Downloads - File Explorer", tint: "#94e2d5" },
];

pub fn tint(p: &Pal, app: &App) -> Color {
    crate::bar::color(app.tint, p.accent)
}

// ---------------------------------------------------------------------------------------------
// Layout maths (the same splits lattice makes, scaled down)

/// Hyprland's dwindle: each window takes half of what's left, split along the longer side.
pub fn dwindle(n: usize, area: R, gap: f32) -> Vec<R> {
    let mut out = Vec::with_capacity(n);
    let (mut x, mut y, mut w, mut h) = area;
    for i in 0..n {
        if i + 1 == n {
            out.push((x, y, w, h));
            break;
        }
        // As layout.rs: side by side only when wider than tall.
        if w > h {
            let a = ((w - gap) / 2.0).floor();
            out.push((x, y, a, h));
            x += a + gap;
            w -= a + gap;
        } else {
            let a = ((h - gap) / 2.0).floor();
            out.push((x, y, w, a));
            y += a + gap;
            h -= a + gap;
        }
    }
    out
}

/// One master on the left, the rest stacked on the right.
pub fn master(n: usize, area: R, gap: f32) -> Vec<R> {
    let (x, y, w, h) = area;
    if n <= 1 {
        return vec![area; n];
    }
    let mw = ((w - gap) * 0.55).floor();
    let sx = x + mw + gap;
    let sw = w - mw - gap;
    let k = (n - 1) as f32;
    let sh = (h - gap * (k - 1.0)) / k;
    let mut out = vec![(x, y, mw, h)];
    for i in 0..n - 1 {
        out.push((sx, y + i as f32 * (sh + gap), sw, sh));
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Keycaps

/// One key, drawn like a real keycap: a lit top edge, a darker lip below.
pub fn key(p: &Pal, label: &str) -> El {
    keycap(p, text(label.to_string()).font(TextStyle::new(12.5, 620.0)).color(p.fg))
}

/// The Windows key: its four-pane logo and "Win".
pub fn win_key(p: &Pal) -> El {
    let pane = |x: f32, y: f32| el().absolute(x, y).size(4.5, 4.5).radius(1.0).fill(p.fg);
    let logo = el().size(10.0, 10.0).child(pane(0.0, 0.0)).child(pane(5.5, 0.0)).child(pane(0.0, 5.5)).child(pane(5.5, 5.5));
    keycap(
        p,
        row().gap(6.0).center_y().child(logo).child(text("Win").font(TextStyle::new(12.5, 620.0)).color(p.fg)),
    )
}

fn keycap(p: &Pal, content: El) -> El {
    row()
        .h(28.0)
        .min_w(28.0)
        .px(9.0)
        .center()
        .radius(7.0)
        .fill(p.key)
        .border_gradient(1.0, p.fg.alpha(0.16), p.fg.alpha(0.04))
        .shadow(2.0, 0.0, Color::BLACK.alpha(0.45))
        .child(content)
}

/// A chord: "Win" draws the Windows key, everything else a plain key.
pub fn chord(p: &Pal, keys: &[&str]) -> El {
    keys.iter().fold(row().gap(5.0).center_y(), |r, k| r.child(if *k == "Win" { win_key(p) } else { key(p, k) }))
}

/// A chord with what it does, for a slide's caption row.
pub fn hint(p: &Pal, keys: &[&str], what: &str) -> El {
    row()
        .gap(10.0)
        .center_y()
        .child(chord(p, keys))
        .child(text(what.to_string()).font(TextStyle::new(13.0, 500.0)).color(p.muted))
}

// ---------------------------------------------------------------------------------------------
// A miniature desktop

/// The stage's wallpaper: the bar's background lifted toward the accent, with a soft glow.
pub fn wallpaper(p: &Pal, w: f32, h: f32) -> El {
    el().key("wallpaper")
        .absolute(0.0, 0.0)
        .size(w, h)
        .fill(p.bg)
        .radial_gradient([0.18, 0.0], [w * 0.7, h * 1.1], [(0.0, p.accent.alpha(0.20)), (1.0, p.accent.alpha(0.0))])
}

/// A mock app window at `r`. It glides when `r` changes, and scales in when it first appears.
pub fn window(p: &Pal, key: impl Into<String>, r: R, app: &App, focused: bool) -> El {
    let c = tint(p, app);
    let (x, y, w, h) = r;
    let small = w < 150.0 || h < 90.0;
    let head = row()
        .h(22.0)
        .px(8.0)
        .gap(6.0)
        .center_y()
        .fill(p.fg.alpha(0.04))
        .child(el().size(10.0, 10.0).radius(3.0).fill(c))
        .child(
            text(if small { app.name } else { app.title })
                .grow(1.0)
                .shrink()
                .truncate()
                .font(TextStyle::new(10.5, 560.0))
                .color(if focused { p.fg } else { p.muted }),
        )
        .child(row().gap(4.0).children((0..3).map(|_| el().size(5.0, 5.0).radius(2.5).fill(p.muted.alpha(0.35))).collect::<Vec<_>>()));
    let line = |frac: f32, a: f32| el().h(6.0).w_frac(frac).radius(3.0).fill(p.fg.alpha(a));
    let mut body = col().padding(10.0).gap(7.0).grow(1.0).min_h(0.0).child(line(0.42, 0.0).fill(c.alpha(0.55)));
    for (i, f) in [0.86, 0.64, 0.78, 0.5, 0.7].iter().enumerate() {
        if 40.0 + i as f32 * 13.0 < h - 22.0 {
            body = body.child(line(*f, 0.07));
        }
    }
    col()
        .key(key.into())
        .absolute(x, y)
        .size(w, h)
        .radius(8.0)
        .fill(p.win)
        .border(1.0, if focused { p.accent.alpha(0.0) } else { p.fg.alpha(0.07) })
        .clip_rounded(8.0)
        .shadow(8.0, 24.0, Color::BLACK.alpha(0.35))
        .motion(TILE)
        .transition(Transition::scale(0.9))
        .child(head)
        .child(body)
}

/// The focus border's soft light, drawn under the windows so it only shows around the
/// focused one. It glides to whichever window has focus.
pub fn glow_halo(p: &Pal, r: R) -> El {
    let (x, y, w, h) = r;
    el().key("focus-halo")
        .absolute(x + 4.0, y + 4.0)
        .size(w - 8.0, h - 8.0)
        .radius(9.0)
        .fill(p.accent.alpha(0.5))
        .shadow(0.0, 34.0, p.accent.alpha(0.7))
        .motion(GLIDE)
}

/// The focus border's line, drawn over the windows.
pub fn glow_ring(p: &Pal, r: R) -> El {
    let (x, y, w, h) = r;
    el().key("focus-ring")
        .absolute(x - 1.0, y - 1.0)
        .size(w + 2.0, h + 2.0)
        .radius(9.0)
        .border(2.0, p.accent)
        .motion(GLIDE)
}

/// lattice's dwindle mark: four tiles, the logo on the welcome slide and in the title band.
pub fn logo(p: &Pal, w: f32, h: f32, shown: usize) -> El {
    let gap = (w * 0.06).max(1.5);
    let r = (w * 0.08).max(2.0);
    let tiles = dwindle(4, (0.0, 0.0, w, h), gap);
    let mut out = el().size(w, h);
    for (i, t) in tiles.into_iter().enumerate() {
        let tile = el()
            .key(format!("logo-{i}"))
            .absolute(t.0, t.1)
            .size(t.2, t.3)
            .radius(r)
            .linear_gradient(2.2, [(0.0, p.accent), (1.0, p.accent2)])
            .transition(Transition::scale(0.6))
            .motion(Spring::BOUNCY);
        if i < shown {
            out = out.child(tile);
        }
    }
    out
}

/// A small drawing of a layout, as the bar's layout button shows it.
pub fn layout_glyph(p: &Pal, layout: &str, color: Color) -> El {
    let (w, h) = (17.0_f32, 12.0_f32);
    let area = (0.0, 0.0, w, h);
    let tiles = match layout {
        "master" => master(3, area, 1.5),
        "monocle" => vec![area],
        _ => dwindle(4, area, 1.5),
    };
    let _ = p;
    tiles.into_iter().fold(el().size(w, h), |d, (x, y, tw, th)| d.child(el().absolute(x, y).size(tw, th).radius(1.5).border(1.2, color)))
}

/// A workspace pill as the bar draws it: its number and a dot per app.
pub fn ws_pill(p: &Pal, n: usize, apps: &[&App], active: bool) -> El {
    let (fill, fg) = if active { (p.accent, p.bg) } else { (p.occupied, p.fg) };
    let mut pill = row()
        .key(format!("ws-{n}"))
        .h(20.0)
        .px(8.0)
        .gap(5.0)
        .center_y()
        .radius(10.0)
        .fill(fill)
        .motion(GLIDE)
        .child(text(n.to_string()).font(TextStyle::new(11.0, 700.0)).color(fg));
    for a in apps {
        pill = pill.child(el().size(8.0, 8.0).radius(2.5).fill(if active { p.bg.alpha(0.55) } else { tint(p, a) }));
    }
    pill
}

/// The top bar across a mock desktop.
pub fn bar(p: &Pal, w: f32, workspaces: &[Vec<&App>], active: usize, title: &str, layout: &str) -> El {
    let mut spaces = row().gap(5.0).center_y();
    for (i, apps) in workspaces.iter().enumerate() {
        spaces = spaces.child(ws_pill(p, i + 1, apps, i == active));
    }
    let stat = |label: &str, v: &str| {
        row()
            .gap(4.0)
            .center_y()
            .child(text(label.to_string()).font(TextStyle::new(10.0, 560.0)).color(p.muted))
            .child(text(v.to_string()).font(TextStyle::new(10.5, 640.0)).color(p.fg))
    };
    row()
        .key("bar")
        .absolute(0.0, 0.0)
        .size(w, 30.0)
        .px(6.0)
        .gap(8.0)
        .center_y()
        .fill(p.bg.alpha(0.92))
        .child(spaces)
        .child(
            text(title.to_string())
                .key("bar-title")
                .grow(1.0)
                .shrink()
                .truncate()
                .align(rune::Align::Center)
                .font(TextStyle::new(10.5, 520.0))
                .color(p.fg)
                .transition(Transition::opacity()),
        )
        .child(row().h(20.0).px(6.0).center().radius(10.0).fill(p.occupied).child(layout_glyph(p, layout, p.fg)))
        .child(stat("CPU", "7%"))
        .child(stat("MEM", "41%"))
        .child(text("14:32").font(TextStyle::new(10.5, 680.0)).color(p.fg))
}

/// A label in the corner of a stage, e.g. the layout in use.
pub fn tag(p: &Pal, content: El) -> El {
    row()
        .h(28.0)
        .px(10.0)
        .gap(8.0)
        .center_y()
        .radius(14.0)
        .fill(p.bg.alpha(0.85))
        .border(1.0, p.fg.alpha(0.08))
        .child(content)
}
