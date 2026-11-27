//! `lattice --tour --tour-preview`: the tour over a drawn stand-in desktop (wallpaper, bar,
//! tiled windows, the launcher), in an ordinary window, with nothing to ask lattice. Each step
//! is a scene, so a debug build renders them headless: `--snap out.png --scene workspaces`.

use rune::{App, AppSpec, Color, Cx, El, Shadow, TextStyle, col, el, row, text};

use super::{Palette, Phase, R, Screen, Tour, view};
use crate::config;
use crate::shared::Anchor;

const W: f32 = 1600.0;
const H: f32 = 900.0;
const BAR_H: f32 = 30.0;
const GAPS: f32 = 10.0;

// Stand-in geometry, in points, that the backdrop draws and the anchors point at.
const WORKSPACES: R = R::new(8.0, 4.0, 196.0, 22.0);
const LAYOUT: R = R::new(1335.0, 4.0, 31.0, 22.0);
const STATS: R = R::new(1380.0, 4.0, 212.0, 22.0);
const FOCUSED: R = R::new(GAPS, BAR_H + GAPS, (W - 3.0 * GAPS) / 2.0, H - BAR_H - 2.0 * GAPS);
const LAUNCHER: R = R::new((W - 640.0) / 2.0, 170.0, 640.0, 392.0);

fn tour() -> Tour {
    let cfg = config::load();
    let mut t = Tour::new(Palette::from_config(&cfg.bar), Screen { x: 0, y: 0, scale: 1.0, w: W, h: H }, true);
    t.preview = true;
    t.phase = Phase::Showing;
    t.anchors[Anchor::Bar as usize] = Some(R::new(0.0, 0.0, W, BAR_H));
    t.anchors[Anchor::Workspaces as usize] = Some(WORKSPACES);
    t.anchors[Anchor::Layout as usize] = Some(LAYOUT);
    t.anchors[Anchor::Stats as usize] = Some(STATS);
    t.anchors[Anchor::Focused as usize] = Some(FOCUSED);
    t
}


type Scene = (&'static str, fn(&mut Tour));

static SCENES: &[Scene] = &[
    ("welcome", |t| t.step = 0),
    ("bar", |t| t.step = 1),
    ("workspaces", |t| t.step = 2),
    ("workspaces-done", |t| {
        t.step = 2;
        t.phase = Phase::Done(0.0);
    }),
    ("tiling", |t| t.step = 3),
    ("layouts", |t| t.step = 4),
    ("system", |t| t.step = 5),
    ("launcher", |t| t.step = 6),
    ("launcher-open", |t| open_at(t, 7)),
    ("files", |t| open_at(t, 8)),
    ("commands", |t| open_at(t, 9)),
    ("windows", |t| open_at(t, 10)),
    ("launcher-close", |t| open_at(t, 11)),
    ("finish", |t| t.step = 12),
];

/// A step that points at the launcher, with the launcher drawn open.
fn open_at(t: &mut Tour, step: usize) {
    t.step = step;
    t.anchors[Anchor::Launcher as usize] = Some(LAUNCHER);
}

fn preview_view(s: &Tour, cx: &mut Cx<Tour>) -> El {
    let mut desk = el().w_full().h_full().child(desktop(&s.pal));
    if s.anchor_rect().is_some() && s.current().anchor == Some(Anchor::Launcher) {
        desk = desk.child(launcher_box(&s.pal, s.current().eyebrow));
    }
    desk.child(view::view(s, cx))
}

pub fn run() {
    let cfg = config::load();
    let families = crate::bar::font_families(&cfg.bar.font);
    App::new(AppSpec::<Tour> {
        scenes: SCENES,
        families,
        min_size: Some((800.0, 500.0)),
        ..AppSpec::new("lattice tour preview", (W, H), tour, preview_view)
    })
    .run();
}

// ---------------------------------------------------------------------------------------------
// The stand-in desktop

fn desktop(p: &Palette) -> El {
    let wall = el()
        .absolute(0.0, 0.0)
        .size(W, H)
        .linear_gradient(
            135.0,
            [
                (0.0, Color { r: 0.020, g: 0.025, b: 0.060, a: 1.0 }),
                (0.55, Color { r: 0.045, g: 0.030, b: 0.090, a: 1.0 }),
                (1.0, Color { r: 0.012, g: 0.040, b: 0.070, a: 1.0 }),
            ],
        );
    let right_x = FOCUSED.x + FOCUSED.w + GAPS;
    let right_w = W - right_x - GAPS;
    let half = (FOCUSED.h - GAPS) / 2.0;
    let quarter = (right_w - GAPS) / 2.0;
    el().absolute(0.0, 0.0)
        .size(W, H)
        .child(wall)
        .child(window(p, FOCUSED, "README.md - Visual Studio Code", true))
        .child(window(p, R::new(right_x, FOCUSED.y, right_w, half), "Windows Terminal", false))
        .child(window(p, R::new(right_x, FOCUSED.y + half + GAPS, quarter, half), "Spotify", false))
        .child(window(p, R::new(right_x + quarter + GAPS, FOCUSED.y + half + GAPS, quarter, half), "Discord", false))
        .child(bar(p))
}

fn window(p: &Palette, r: R, title: &str, focused: bool) -> El {
    let body = (0..6).fold(col().gap(9.0).padding(18.0), |c, i| {
        let w = [0.62, 0.48, 0.71, 0.35, 0.56, 0.42][i] * r.w;
        c.child(el().h(7.0).w(w.min(r.w - 36.0)).radius(3.5).fill(p.fg.alpha(0.07)))
    });
    let mut w = col()
        .absolute(r.x, r.y)
        .size(r.w, r.h)
        .radius(8.0)
        .fill(p.bg.mix(Color::BLACK, 0.25))
        .border(1.0, Color::WHITE.alpha(0.06))
        .child(
            row()
                .h(34.0)
                .px(14.0)
                .center_y()
                .fill(p.bg.mix(Color::BLACK, 0.1))
                .child(text(title.to_string()).font(TextStyle::new(12.0, 500.0)).color(p.fg.alpha(0.6))),
        )
        .child(body);
    if focused {
        // lattice's focus border.
        w = w
            .border(2.0, p.accent)
            .box_shadow(Shadow { blur: 22.0, color: p.accent.alpha(0.45), ..Shadow::default() });
    }
    w
}

fn bar(p: &Palette) -> El {
    let pill = |n: usize, focused: bool, apps: &[Color]| {
        let fill = if focused { p.accent } else { p.surface };
        let ink = if focused { p.bg } else { p.fg };
        apps.iter().fold(
            row().h(22.0).px(9.0).gap(6.0).center_y().radius(11.0).fill(fill).child(
                text(n.to_string()).font(TextStyle::new(12.0, 700.0)).color(ink),
            ),
            |r, c| r.child(el().size(14.0, 14.0).radius(3.0).fill(*c)),
        )
    };
    let blue = Color { r: 0.05, g: 0.35, b: 0.85, a: 1.0 };
    let green = Color { r: 0.05, g: 0.6, b: 0.25, a: 1.0 };
    let purple = Color { r: 0.3, g: 0.2, b: 0.85, a: 1.0 };
    let grey = Color { r: 0.25, g: 0.25, b: 0.28, a: 1.0 };
    let spaces = row()
        .absolute(WORKSPACES.x, WORKSPACES.y)
        .gap(6.0)
        .child(pill(1, true, &[blue, grey, green, purple]))
        .child(pill(2, false, &[grey]))
        .child(pill(3, false, &[purple]));
    let stat = |label: &str, value: &str| {
        row().gap(4.0).center_y()
            .child(text(label.to_string()).font(TextStyle::new(11.0, 500.0)).color(p.muted))
            .child(text(value.to_string()).font(TextStyle::new(12.0, 600.0)).color(p.fg))
    };
    let layout = row()
        .absolute(LAYOUT.x, LAYOUT.y)
        .size(LAYOUT.w, LAYOUT.h)
        .center()
        .radius(11.0)
        .fill(p.surface)
        .child(
            el().size(17.0, 12.0)
                .child(el().absolute(0.0, 0.0).size(7.75, 12.0).radius(1.5).border(1.2, p.fg))
                .child(el().absolute(9.25, 0.0).size(7.75, 5.25).radius(1.5).border(1.2, p.fg))
                .child(el().absolute(9.25, 6.75).size(3.1, 5.25).radius(1.5).border(1.2, p.fg))
                .child(el().absolute(13.9, 6.75).size(3.1, 5.25).radius(1.5).border(1.2, p.fg)),
        );
    let stats = row()
        .absolute(STATS.x, STATS.y)
        .size(STATS.w, STATS.h)
        .gap(14.0)
        .center_y()
        .child(stat("CPU", "12%"))
        .child(stat("MEM", "41%"))
        .child(stat("BAT", "87%"))
        .child(text("Wed 14:32").font(TextStyle::new(12.0, 650.0)).color(p.fg));
    el().absolute(0.0, 0.0)
        .size(W, BAR_H)
        .fill(p.bg)
        .child(spaces)
        .child(text("README.md - Visual Studio Code").absolute(640.0, 7.0).w(320.0).font(TextStyle::new(12.0, 500.0)).color(p.fg))
        .child(layout)
        .child(stats)
}

/// The launcher as it looks in the mode a step talks about (by its eyebrow).
fn launcher_box(p: &Palette, mode: &str) -> El {
    let r = LAUNCHER;
    let (query, rows): (&str, [&str; 6]) = match mode {
        "Files" => ("f notes", ["notes.md", "Notes", "meeting-notes.txt", "notes-2026.docx", "release-notes.md", "Notes (Archive)"]),
        "Commands" => ("> layout", ["Layout: Dwindle", "Layout: Master", "Layout: Monocle", "Toggle floating", "Retile", "Reload config"]),
        "Windows" => ("", ["Windows Terminal", "Spotify", "Discord", "Firefox", "Settings", "README.md - Visual Studio Code"]),
        _ => ("vs", ["Visual Studio Code", "Windows Terminal", "Spotify", "Discord", "Firefox", "Settings"]),
    };
    let list = rows.iter().enumerate().fold(col().px(8.0).py(8.0), |c, (i, name)| {
        c.child(
            row()
                .h(46.0)
                .px(12.0)
                .gap(12.0)
                .center_y()
                .radius(8.0)
                .fill(if i == 0 { p.surface } else { Color::TRANSPARENT })
                .child(el().size(26.0, 26.0).radius(6.0).fill(p.accent.alpha(0.25 + 0.1 * i as f32)))
                .child(text(name.to_string()).font(TextStyle::new(14.0, 500.0)).color(p.fg)),
        )
    });
    col()
        .absolute(r.x, r.y)
        .size(r.w, r.h)
        .radius(8.0)
        .fill(p.bg)
        .border(1.0, p.surface)
        .child(
            row().h(60.0).px(20.0).center_y().child(text(query.to_string()).font(TextStyle::new(18.0, 500.0)).color(p.fg)),
        )
        .child(el().h(1.0).fill(p.surface))
        .child(list)
}
