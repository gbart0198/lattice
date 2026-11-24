//! A ~30 second feature teaser for lattice, rendered with Rune, plus its soundtrack.
//!
//! Everything is a pure function of time, so a headless Rune recording is deterministic and
//! every cut lands on the beat of the synthesized track (120 BPM: a beat is 0.5 s, a bar 2 s).
//!
//! ```sh
//! cargo build --bin teaser
//! target/debug/teaser --music music.wav
//! target/debug/teaser --out frames/ --duration 30 --fps 30 --size 1920x1080 --scale 1
//! ffmpeg -framerate 30 -i frames/frame_%04d.png -i music.wav -c:v h264_nvenc -c:a aac teaser.mp4
//! ```

use rune::{AppSpec, Color, Cx, El, TextStyle, Tx, el, text};

// ---------------------------------------------------------------------------------------------
// Timing & easing

const BEAT: f32 = 0.5;
const W: f32 = 1920.0;
const H: f32 = 1080.0;

fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}
/// 0 before `a`, 1 after `a + d`, eased in between.
fn ramp(t: f32, a: f32, d: f32) -> f32 {
    let x = clamp01((t - a) / d);
    1.0 - (1.0 - x).powi(3)
}
/// Visible between `a` and `b`, with `f`-second fades.
fn window(t: f32, a: f32, b: f32, f: f32) -> f32 {
    clamp01((t - a) / f).min(clamp01((b - t) / f))
}
/// 1 on each beat, decaying quickly: drives beat-synced pulses.
fn pulse(t: f32) -> f32 {
    (-(t.rem_euclid(BEAT)) / 0.12).exp()
}
fn lerp(a: f32, b: f32, x: f32) -> f32 {
    a + (b - a) * x
}

// ---------------------------------------------------------------------------------------------
// Colours (sRGB hex -> Rune's linear colour)

fn hex(s: &str) -> Color {
    let h = s.trim_start_matches('#');
    let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0) as f32 / 255.0;
    let lin = |c: f32| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
    Color { r: lin(p(0)), g: lin(p(2)), b: lin(p(4)), a: 1.0 }
}

struct Pal {
    bg0: Color,
    bg1: Color,
    bar: Color,
    panel: Color,
    panel2: Color,
    fg: Color,
    muted: Color,
    accent: Color,
    pink: Color,
    green: Color,
    yellow: Color,
    peach: Color,
    teal: Color,
    mauve: Color,
}

fn pal() -> Pal {
    Pal {
        bg0: hex("#0b0b14"),
        bg1: hex("#1a1630"),
        bar: hex("#1e1e2e"),
        panel: hex("#181825"),
        panel2: hex("#24243a"),
        fg: hex("#cdd6f4"),
        muted: hex("#7f849c"),
        accent: hex("#89b4fa"),
        pink: hex("#f5c2e7"),
        green: hex("#a6e3a1"),
        yellow: hex("#f9e2af"),
        peach: hex("#fab387"),
        teal: hex("#94e2d5"),
        mauve: hex("#cba6f7"),
    }
}

// ---------------------------------------------------------------------------------------------
// Geometry

#[derive(Clone, Copy, Debug, Default)]
struct R {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}
impl R {
    fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        R { x, y, w, h }
    }
    fn lerp(a: R, b: R, k: f32) -> R {
        R::new(lerp(a.x, b.x, k), lerp(a.y, b.y, k), lerp(a.w, b.w, k), lerp(a.h, b.h, k))
    }
    fn inset(&self, d: f32) -> R {
        R::new(self.x + d, self.y + d, (self.w - 2.0 * d).max(1.0), (self.h - 2.0 * d).max(1.0))
    }
    fn scale_about_center(&self, s: f32) -> R {
        let (cx, cy) = (self.x + self.w / 2.0, self.y + self.h / 2.0);
        R::new(cx - self.w * s / 2.0, cy - self.h * s / 2.0, self.w * s, self.h * s)
    }
}

fn boxed(r: R) -> El {
    el().absolute(r.x, r.y).size(r.w.max(0.0), r.h.max(0.0))
}

#[derive(Clone, Copy, PartialEq)]
enum Layout {
    Dwindle,
    Master,
    Monocle,
}

/// Same maths as lattice's layout.rs: outer gaps at the edges, inner gaps between tiles.
fn tile(kind: Layout, area: R, n: usize, inner: f32, outer: f32) -> Vec<R> {
    if n == 0 {
        return vec![];
    }
    let half = inner / 2.0;
    let a = area.inset(outer - half);
    let raw: Vec<R> = match kind {
        Layout::Monocle => vec![a; n],
        Layout::Dwindle => {
            let mut out = vec![];
            let mut rest = a;
            for i in 0..n {
                if i == n - 1 {
                    out.push(rest);
                    break;
                }
                if rest.w > rest.h {
                    let w1 = rest.w * 0.5;
                    out.push(R::new(rest.x, rest.y, w1, rest.h));
                    rest = R::new(rest.x + w1, rest.y, rest.w - w1, rest.h);
                } else {
                    let h1 = rest.h * 0.5;
                    out.push(R::new(rest.x, rest.y, rest.w, h1));
                    rest = R::new(rest.x, rest.y + h1, rest.w, rest.h - h1);
                }
            }
            out
        }
        Layout::Master => {
            if n == 1 {
                vec![a]
            } else {
                let mw = a.w * 0.58;
                let mut out = vec![R::new(a.x, a.y, mw, a.h)];
                let k = (n - 1) as f32;
                for i in 0..n - 1 {
                    let y0 = a.y + a.h * i as f32 / k;
                    out.push(R::new(a.x + mw, y0, a.w - mw, a.h / k));
                }
                out
            }
        }
    };
    raw.into_iter().map(|r| r.inset(half)).collect()
}

// ---------------------------------------------------------------------------------------------
// Apps and windows

#[derive(Clone, Copy, PartialEq)]
enum App {
    Term,
    Code,
    Web,
    Music,
    Chat,
    Games,
    Notes,
}

fn app_meta(a: App, p: &Pal) -> (&'static str, &'static str, Color) {
    match a {
        App::Term => ("terminal", ">_", p.green),
        App::Code => ("editor", "{}", p.accent),
        App::Web => ("browser", "W", p.peach),
        App::Music => ("music", "M", p.pink),
        App::Chat => ("chat", "C", p.mauve),
        App::Games => ("games", "G", p.teal),
        App::Notes => ("notes", "N", p.yellow),
    }
}

fn badge(a: App, size: f32, p: &Pal) -> El {
    let (_, glyph, c) = app_meta(a, p);
    el().size(size, size).radius(size * 0.28).fill(c).center().child(
        text(glyph).font(TextStyle::new(size * 0.52, 800.0)).color(p.bg0),
    )
}

/// Draws a mock app window at `r`. `k` scales text and details; `t` animates content.
fn window_el(a: App, r: R, k: f32, alpha: f32, t: f32, p: &Pal) -> El {
    let (name, _, c) = app_meta(a, p);
    let title_h = 30.0 * k;
    let mut w = boxed(r).radius(9.0 * k).fill(p.panel).clip_rounded(9.0 * k).opacity(alpha);
    // Title bar.
    w = w.child(
        el().absolute(0.0, 0.0)
            .size(r.w, title_h)
            .fill(p.panel2)
            .child(el().absolute(10.0 * k, 6.0 * k).child(badge(a, 18.0 * k, p)))
            .child(
                el().absolute(36.0 * k, 0.0).size(r.w, title_h).center_y().child(
                    text(name).font(TextStyle::new(13.0 * k, 600.0)).color(p.fg),
                ),
            )
            .child(dots(r.w, title_h, k, p)),
    );
    let body = R::new(0.0, title_h, r.w, r.h - title_h);
    let pad = 16.0 * k;
    match a {
        App::Term => {
            let lines: [(&str, Color); 7] = [
                ("$ cargo build --release", p.fg),
                ("   Compiling rune v0.1.0", p.muted),
                ("   Compiling lattice v0.1.0", p.muted),
                ("    Finished `release` in 28.1s", p.green),
                ("$ lattice --list", p.fg),
                ("  tiled     brave.exe   Browser", p.accent),
                ("$ _", p.fg),
            ];
            for (i, (line, col)) in lines.iter().enumerate() {
                let y = body.y + pad + i as f32 * 22.0 * k;
                if y + 22.0 * k > r.h {
                    break;
                }
                w = w.child(
                    el().absolute(pad, y)
                        .child(text(*line).font(TextStyle::new(13.0 * k, 500.0).mono()).color(*col)),
                );
            }
        }
        App::Code => {
            let cols = [p.mauve, p.accent, p.fg, p.peach, p.green, p.teal, p.muted];
            let widths = [0.42, 0.65, 0.3, 0.55, 0.48, 0.22, 0.6, 0.35, 0.5, 0.28, 0.62, 0.4, 0.33, 0.5];
            for (i, wf) in widths.iter().enumerate() {
                let y = body.y + pad + i as f32 * 20.0 * k;
                if y + 12.0 * k > r.h {
                    break;
                }
                let indent = if i % 4 == 0 { 0.0 } else if i % 3 == 0 { 2.0 } else { 1.0 };
                w = w
                    .child(boxed(R::new(pad, y + 3.0 * k, 14.0 * k, 7.0 * k)).radius(2.0).fill(p.muted).opacity(0.35))
                    .child(
                        boxed(R::new(pad + (26.0 + indent * 18.0) * k, y + 2.0 * k, (r.w - 80.0 * k) * wf, 9.0 * k))
                            .radius(4.0 * k)
                            .fill(cols[i % cols.len()])
                            .opacity(0.75),
                    );
            }
        }
        App::Web => {
            w = w.child(boxed(R::new(pad, body.y + pad, r.w - 2.0 * pad, 18.0 * k)).radius(9.0 * k).fill(p.panel2));
            let hero = R::new(pad, body.y + pad + 30.0 * k, r.w - 2.0 * pad, (body.h * 0.42).max(20.0 * k));
            w = w.child(boxed(hero).radius(10.0 * k).gradient(c, p.mauve).opacity(0.85));
            let cy = hero.y + hero.h + 14.0 * k;
            let cw = (r.w - 2.0 * pad - 2.0 * 12.0 * k) / 3.0;
            for i in 0..3 {
                let card = R::new(pad + i as f32 * (cw + 12.0 * k), cy, cw, (r.h - cy - pad).max(0.0));
                w = w.child(boxed(card).radius(8.0 * k).fill(p.panel2));
            }
        }
        App::Music => {
            let art = (body.h * 0.5).min(r.w * 0.4).max(10.0);
            w = w.child(boxed(R::new(pad, body.y + pad, art, art)).radius(10.0 * k).gradient(p.pink, p.mauve));
            w = w.child(
                el().absolute(pad * 2.0 + art, body.y + pad)
                    .child(text("Night Drive").font(TextStyle::new(16.0 * k, 700.0)).color(p.fg)),
            );
            w = w.child(
                el().absolute(pad * 2.0 + art, body.y + pad + 24.0 * k)
                    .child(text("lattice OST").font(TextStyle::new(12.0 * k, 500.0)).color(p.muted)),
            );
            // Spectrum bars dancing on the beat.
            let n = 24;
            let base_y = r.h - pad;
            let bw = (r.w - 2.0 * pad) / n as f32;
            let max_h = (r.h - (body.y + pad + art + 14.0 * k) - pad).max(4.0);
            for i in 0..n {
                let ph = i as f32 * 0.7;
                let v = 0.25 + 0.45 * pulse(t + i as f32 * 0.01) + 0.3 * ((t * 6.0 + ph).sin() * 0.5 + 0.5);
                let bh = max_h * v.min(1.0);
                w = w.child(
                    boxed(R::new(pad + i as f32 * bw + bw * 0.15, base_y - bh, bw * 0.7, bh))
                        .radius(2.0 * k)
                        .gradient(p.pink, p.accent),
                );
            }
        }
        App::Chat => {
            for i in 0..6 {
                let mine = i % 2 == 1;
                let y = body.y + pad + i as f32 * 38.0 * k;
                if y + 28.0 * k > r.h {
                    break;
                }
                let bw = (r.w - 2.0 * pad) * if i % 3 == 0 { 0.55 } else { 0.4 };
                let x = if mine { r.w - pad - bw } else { pad };
                w = w.child(boxed(R::new(x, y, bw, 28.0 * k)).radius(14.0 * k).fill(if mine { c } else { p.panel2 }).opacity(if mine { 0.8 } else { 1.0 }));
            }
        }
        App::Games => {
            let cols = 4;
            let gw = (r.w - 2.0 * pad - (cols - 1) as f32 * 10.0 * k) / cols as f32;
            let gh = gw * 1.3;
            let hues = [p.teal, p.accent, p.peach, p.mauve, p.green, p.pink, p.yellow, p.accent];
            for i in 0..8 {
                let (cx, cy) = (i % cols, i / cols);
                let rr = R::new(pad + cx as f32 * (gw + 10.0 * k), body.y + pad + cy as f32 * (gh + 10.0 * k), gw, gh);
                if rr.y + rr.h > r.h {
                    break;
                }
                w = w.child(boxed(rr).radius(8.0 * k).gradient(hues[i], p.panel2));
            }
        }
        App::Notes => {
            w = w.child(el().absolute(pad, body.y + pad).child(text("Ideas").font(TextStyle::new(18.0 * k, 800.0)).color(p.fg)));
            for i in 0..8 {
                let y = body.y + pad + 34.0 * k + i as f32 * 18.0 * k;
                if y + 8.0 * k > r.h {
                    break;
                }
                let wf = [0.8, 0.6, 0.7, 0.4, 0.75, 0.5, 0.65, 0.3][i];
                w = w.child(boxed(R::new(pad, y, (r.w - 2.0 * pad) * wf, 7.0 * k)).radius(3.5 * k).fill(p.muted).opacity(0.45));
            }
        }
    }
    w
}

fn dots(w: f32, h: f32, k: f32, p: &Pal) -> El {
    let mut row_el = el().absolute(w - 70.0 * k, 0.0).size(64.0 * k, h).center_y();
    for (i, c) in [p.muted, p.muted, p.pink].iter().enumerate() {
        row_el = row_el.child(boxed(R::new(i as f32 * 20.0 * k, h / 2.0 - 4.0 * k, 9.0 * k, 9.0 * k)).radius(4.5 * k).fill(*c).opacity(0.7));
    }
    row_el
}

/// The lattice focus border: a crisp line plus fading 1px rings (same look as border.rs).
fn glow(r: R, k: f32, color: Color, strength: f32) -> Vec<El> {
    let mut v = Vec::new();
    let n = (16.0 * k).ceil() as i32;
    for d in (1..=n).rev() {
        let t = d as f32 / (n as f32 + 1.0);
        let a = strength * (1.0 - t).powi(2);
        let df = d as f32;
        v.push(boxed(R::new(r.x - df, r.y - df, r.w + 2.0 * df, r.h + 2.0 * df)).radius(9.0 * k + df).border(1.0, color.alpha(a)));
    }
    v.push(boxed(r).radius(9.0 * k).border(2.0 * k.max(0.6), color));
    v
}

// ---------------------------------------------------------------------------------------------
// The desktop mock: bar + tiled windows + focus glow

struct WinDraw {
    app: App,
    r: R,
    alpha: f32,
}

struct Desk {
    screen: R,
    bar: bool,
    active_ws: usize,
    ws_apps: [Vec<App>; 3],
    layout: Layout,
    wins: Vec<WinDraw>,
    focus: Option<R>,
    glow_strength: f32,
}

fn layout_icon(kind: Layout, k: f32, p: &Pal) -> El {
    let (w, h, g) = (17.0 * k, 12.0 * k, 1.5 * k);
    let tiles: Vec<R> = match kind {
        Layout::Master => {
            let mw = (w - g) * 0.55;
            let sh = (h - g) / 2.0;
            vec![R::new(0.0, 0.0, mw, h), R::new(mw + g, 0.0, w - mw - g, sh), R::new(mw + g, sh + g, w - mw - g, sh)]
        }
        Layout::Monocle => vec![R::new(0.0, 0.0, w, h)],
        Layout::Dwindle => {
            let hw = (w - g) / 2.0;
            let rx = hw + g;
            let th = (h - g) / 2.0;
            let bw = (w - rx - g) / 2.0;
            vec![R::new(0.0, 0.0, hw, h), R::new(rx, 0.0, w - rx, th), R::new(rx, th + g, bw, th), R::new(rx + bw + g, th + g, bw, th)]
        }
    };
    tiles.into_iter().fold(el().size(w, h), |d, r| d.child(boxed(r).radius(1.5 * k).border(1.2 * k.max(0.7), p.fg)))
}

fn desk_el(d: &Desk, k: f32, t: f32, alpha: f32, p: &Pal) -> El {
    let s = d.screen;
    let mut root = boxed(s).radius(14.0 * k).clip_rounded(14.0 * k).gradient(hex("#2a2350"), hex("#0f1a2e")).opacity(alpha);
    // Soft wallpaper shapes.
    root = root
        .child(boxed(R::new(s.w * 0.55, s.h * 0.15, s.w * 0.6, s.w * 0.6)).radius(s.w * 0.3).fill(p.mauve).opacity(0.10))
        .child(boxed(R::new(-s.w * 0.15, s.h * 0.45, s.w * 0.5, s.w * 0.5)).radius(s.w * 0.25).fill(p.accent).opacity(0.08));
    let bar_h = if d.bar { 30.0 * k } else { 0.0 };
    for w in &d.wins {
        root = root.child(window_el(w.app, w.r, k, w.alpha, t, p));
    }
    if let Some(f) = d.focus {
        for g in glow(f, k, p.accent, d.glow_strength) {
            root = root.child(g);
        }
    }
    if d.bar {
        let mut bar = el().absolute(0.0, 0.0).size(s.w, bar_h).fill(p.bar);
        // Workspace pills.
        let mut x = 8.0 * k;
        for (i, apps) in d.ws_apps.iter().enumerate() {
            let on = i == d.active_ws;
            let pw = 30.0 * k + apps.len() as f32 * 19.0 * k;
            let mut pill = boxed(R::new(x, 4.0 * k, pw, bar_h - 8.0 * k))
                .radius((bar_h - 8.0 * k) / 2.0)
                .fill(if on { p.accent } else { p.panel2 })
                .child(
                    el().absolute(9.0 * k, 0.0).size(12.0 * k, bar_h - 8.0 * k).center_y().child(
                        text((i + 1).to_string()).font(TextStyle::new(13.0 * k, 800.0)).color(if on { p.bg0 } else { p.fg }),
                    ),
                );
            for (j, a) in apps.iter().enumerate() {
                pill = pill.child(el().absolute(24.0 * k + j as f32 * 19.0 * k, (bar_h - 8.0 * k - 15.0 * k) / 2.0).child(badge(*a, 15.0 * k, p)));
            }
            bar = bar.child(pill);
            x += pw + 6.0 * k;
        }
        // Layout button, stats, clock.
        let rx = s.w - 300.0 * k;
        bar = bar
            .child(
                boxed(R::new(rx, 4.0 * k, 31.0 * k, bar_h - 8.0 * k)).radius((bar_h - 8.0 * k) / 2.0).fill(p.panel2).center().child(layout_icon(d.layout, k, p)),
            )
            .child(el().absolute(rx + 46.0 * k, 0.0).size(240.0 * k, bar_h).center_y().child(
                text(format!("CPU {:>2}%    MEM 41%    Tue 16:20", 6 + ((t * 2.0) as i32 % 5))).font(TextStyle::new(12.0 * k, 600.0)).color(p.fg),
            ));
        root = root.child(bar);
    }
    root
}

/// Tiling area inside a screen (below the bar), in canvas coordinates.
fn area(screen: R, bar: bool, k: f32) -> R {
    let bh = if bar { 30.0 * k } else { 0.0 };
    R::new(screen.x + 0.0, screen.y + bh, screen.w, screen.h - bh)
}

/// Windows are drawn inside the screen box, so convert canvas rects to screen-local ones.
fn local(r: R, screen: R) -> R {
    R::new(r.x - screen.x, r.y - screen.y, r.w, r.h)
}

// ---------------------------------------------------------------------------------------------
// Captions, keycaps, logo

fn caption(title: &str, sub: &str, a: f32, p: &Pal) -> El {
    el().absolute(0.0, 930.0).size(W, 120.0).center().opacity(a).child(
        rune::col().center().gap(6.0)
            .child(text(title.to_string()).font(TextStyle::new(44.0, 800.0).tracking(-1.0)).color(p.fg))
            .child(text(sub.to_string()).font(TextStyle::new(22.0, 500.0)).color(p.muted)),
    )
}

fn keycaps(keys: &[&str], a: f32, y: f32, p: &Pal) -> El {
    let mut r = rune::row().gap(10.0).center_y();
    for (i, key) in keys.iter().enumerate() {
        if i > 0 {
            r = r.child(text("+").font(TextStyle::new(22.0, 700.0)).color(p.muted));
        }
        r = r.child(
            el().px(16.0).py(8.0).radius(10.0).fill(p.panel2).border(1.5, p.accent.alpha(0.8)).shadow(4.0, 10.0, p.bg0)
                .child(text(key.to_string()).font(TextStyle::new(22.0, 700.0)).color(p.fg)),
        );
    }
    el().absolute(0.0, y).size(W, 60.0).center().opacity(a).scale(lerp(0.9, 1.0, a)).child(r)
}

fn logo(t0: f32, t: f32, size: f32, cx: f32, cy: f32, p: &Pal) -> El {
    // The dwindle glyph, its four tiles popping in one per beat.
    let (w, h, g) = (size, size * 0.7, size * 0.06);
    let hw = (w - g) / 2.0;
    let rx = hw + g;
    let th = (h - g) / 2.0;
    let bw = (w - rx - g) / 2.0;
    let tiles = [
        R::new(0.0, 0.0, hw, h),
        R::new(rx, 0.0, w - rx, th),
        R::new(rx, th + g, bw, th),
        R::new(rx + bw + g, th + g, bw, th),
    ];
    let cols = [p.accent, p.mauve, p.pink, p.teal];
    let mut root = el().absolute(cx - w / 2.0, cy - h / 2.0).size(w, h);
    for (i, r) in tiles.iter().enumerate() {
        let a = ramp(t, t0 + i as f32 * BEAT, 0.25);
        let rr = r.scale_about_center(lerp(0.6, 1.0, a));
        root = root.child(boxed(rr).radius(size * 0.05).gradient(cols[i], cols[i].mix(p.bg0, 0.35)).opacity(a));
    }
    root
}

// ---------------------------------------------------------------------------------------------
// The timeline

struct State {
    t: f32,
}

fn init() -> State {
    State { t: 0.0 }
}

fn poll(s: &mut State, t: f64, tx: &mut Tx) -> bool {
    s.t = t as f32;
    tx.instant(); // every frame is computed from t: no springs
    true
}

/// ws1 on the main screen gains a window every 1.5 s, then focus hops on the beat.
const APPEAR: [f32; 4] = [4.5, 6.0, 7.5, 9.0];
const WS1: [App; 4] = [App::Term, App::Code, App::Web, App::Music];

fn single_screen_desk(t: f32, screen: R, k: f32) -> Desk {
    let inner = 10.0 * k;
    let outer = 12.0 * k;
    let ar = area(screen, true, k);
    // How many windows exist, and how far the newest one has animated in.
    let n = APPEAR.iter().filter(|a| t >= **a).count();
    let grow = if n > 0 { ramp(t, APPEAR[n - 1], 0.35) } else { 1.0 };
    // Layout phase: dwindle -> master (12) -> monocle (14) -> dwindle (16).
    let (from, to, lk) = if t < 12.0 {
        (Layout::Dwindle, Layout::Dwindle, 1.0)
    } else if t < 14.0 {
        (Layout::Dwindle, Layout::Master, ramp(t, 12.0, 0.4))
    } else if t < 16.0 {
        (Layout::Master, Layout::Monocle, ramp(t, 14.0, 0.4))
    } else {
        (Layout::Monocle, Layout::Dwindle, ramp(t, 16.0, 0.4))
    };
    let now: Vec<R> = {
        let a = tile(from, ar, n, inner, outer);
        let b = tile(to, ar, n, inner, outer);
        a.iter().zip(&b).map(|(x, y)| R::lerp(*x, *y, lk)).collect()
    };
    let before = tile(Layout::Dwindle, ar, n.saturating_sub(1), inner, outer);
    // Focus target.
    let hops = [3usize, 1, 0, 2, 3, 1];
    let focus_idx = if t < 9.0 {
        n.saturating_sub(1)
    } else if t < 12.0 {
        let b = ((t - 9.5) / BEAT).floor().max(0.0) as usize;
        if t < 9.5 { 3 } else { hops[b.min(hops.len() - 1)] }
    } else if (14.0..16.0).contains(&t) {
        // Monocle cycles through the stack on the beat.
        let b = ((t - 14.5) / BEAT).floor().max(-1.0);
        if b < 0.0 { 0 } else { (b as usize + 1) % 4 }
    } else {
        0
    };
    let mut wins = Vec::new();
    for i in 0..n {
        let mut r = now[i];
        let mut alpha = 1.0;
        if t < 12.0 {
            if i + 1 == n {
                r = r.scale_about_center(lerp(0.85, 1.0, grow));
                alpha = grow;
            } else if let Some(b) = before.get(i) {
                r = R::lerp(*b, now[i], grow);
            }
        }
        // In monocle only the focused window shows (crossfade during the change).
        if (14.0..16.4).contains(&t) {
            let mono = if t < 16.0 { ramp(t, 14.0, 0.4) } else { 1.0 - ramp(t, 16.0, 0.4) };
            if i != focus_idx {
                alpha *= 1.0 - mono;
            }
        }
        wins.push(WinDraw { app: WS1[i], r: local(r, screen), alpha });
    }
    // Draw the focused window last (on top).
    if focus_idx < wins.len() {
        let f = wins.remove(focus_idx);
        wins.push(f);
    }
    // Focus rect glides between windows.
    let focus = if n == 0 {
        None
    } else {
        let target = wins.last().map(|w| w.r).unwrap();
        let glide = if (9.0..12.0).contains(&t) { ramp(t, (t / BEAT).floor() * BEAT, 0.18) } else { 1.0 };
        let prev = if (9.5..12.0).contains(&t) {
            let b = ((t - 9.5) / BEAT).floor() as usize;
            let pi = if b == 0 { 3 } else { hops[(b - 1).min(hops.len() - 1)] };
            local(now[pi.min(n - 1)], screen)
        } else {
            target
        };
        Some(R::lerp(prev, target, glide))
    };
    let mut ws_apps: [Vec<App>; 3] = [WS1[..n].to_vec(), vec![App::Web, App::Chat], vec![App::Games]];
    if n == 0 {
        ws_apps[0].clear();
    }
    let layout = if lk > 0.5 { to } else { from };
    Desk {
        screen,
        bar: true,
        active_ws: 0,
        ws_apps,
        layout,
        wins,
        focus,
        glow_strength: 0.5 + 0.35 * pulse(t),
    }
}

/// Dual-monitor section: workspaces that span both screens, switched on the beat.
fn dual_desks(t: f32, left: R, right: R, k: f32) -> (Desk, Desk) {
    // Which workspace is active: 1 until 19.5, then 2, 3, 1, then window sent to 3, then 3.
    let ws = if t < 19.5 {
        0
    } else if t < 20.5 {
        1
    } else if t < 21.5 {
        2
    } else if t < 23.5 {
        0
    } else {
        2
    };
    let sent = t >= 22.5; // terminal sent to ws3 with Win+Shift+3
    let mut left_apps: [Vec<App>; 3] = [vec![App::Chat, App::Notes], vec![App::Web], vec![App::Music]];
    let mut right_apps: [Vec<App>; 3] = [vec![App::Term, App::Code, App::Web, App::Music], vec![App::Games, App::Chat], vec![App::Code]];
    if sent {
        right_apps[0].retain(|a| *a != App::Term);
        right_apps[2].push(App::Term);
    }
    let _ = &mut left_apps;
    let inner = 10.0 * k;
    let outer = 12.0 * k;
    let mk = |screen: R, bar: bool, apps: &Vec<App>, retile_from: Option<usize>| -> Vec<WinDraw> {
        let ar = area(screen, bar, k);
        let now = tile(Layout::Dwindle, ar, apps.len(), inner, outer);
        let prev = retile_from.map(|m| tile(Layout::Dwindle, ar, m, inner, outer));
        apps.iter()
            .enumerate()
            .map(|(i, a)| {
                let mut r = now[i];
                if let Some(p) = &prev {
                    // After Win+Shift+3 the rest of ws1 re-tiles into the freed space.
                    let src = p.get(i + 1).copied().unwrap_or(r);
                    r = R::lerp(src, now[i], ramp(t, 22.5, 0.35));
                }
                WinDraw { app: *a, r: local(r, screen), alpha: 1.0 }
            })
            .collect()
    };
    let retile = if sent && ws == 0 && t < 23.0 { Some(4) } else { None };
    let lw = mk(left, false, &left_apps[ws], None);
    let rw = mk(right, true, &right_apps[ws], retile);
    // Focus: right screen's first window (the moved terminal on ws3).
    let focus_r = if ws == 2 && sent { rw.iter().find(|w| w.app == App::Term).map(|w| w.r) } else { rw.first().map(|w| w.r) };
    let all_ws: [Vec<App>; 3] = std::array::from_fn(|i| {
        let mut v = right_apps[i].clone();
        v.extend(left_apps[i].iter().copied());
        v
    });
    let gs = 0.5 + 0.35 * pulse(t);
    (
        Desk { screen: left, bar: false, active_ws: ws, ws_apps: all_ws.clone(), layout: Layout::Dwindle, wins: lw, focus: None, glow_strength: gs },
        Desk { screen: right, bar: true, active_ws: ws, ws_apps: all_ws, layout: Layout::Dwindle, wins: rw, focus: focus_r, glow_strength: gs },
    )
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let t = s.t;
    let p = pal();
    let mut root = el().w_full().h_full().gradient(p.bg1, p.bg0);
    // Beat-synced ambient glow behind everything.
    let beat_glow = (0.16 + 0.12 * pulse(t)) * window(t, 0.0, 29.6, 0.8);
    root = root.child(boxed(R::new(0.0, 0.0, W, H)).radial_gradient(
        [0.5, 0.45],
        [1100.0, 700.0],
        [
            (0.0, p.accent.alpha(beat_glow)),
            (0.45, p.mauve.alpha(beat_glow * 0.35)),
            (1.0, p.mauve.alpha(0.0)),
        ],
    ));

    // ---- Intro (0-4s)
    if t < 4.2 {
        let a = 1.0 - ramp(t, 3.6, 0.4);
        root = root.child(logo(0.0, t, 260.0, W / 2.0, 380.0, &p).opacity(a));
        let ta = ramp(t, 2.0, 0.5) * a;
        root = root.child(
            el().absolute(0.0, 520.0 + (1.0 - ta) * 20.0).size(W, 150.0).center().opacity(ta).child(
                text("lattice").font(TextStyle::new(128.0, 800.0).tracking(-4.0)).color(p.fg),
            ),
        );
        let sa = ramp(t, 3.0, 0.4) * a;
        root = root.child(
            el().absolute(0.0, 680.0).size(W, 50.0).center().opacity(sa).child(
                text("a tiling window manager for Windows 11").font(TextStyle::new(30.0, 500.0)).color(p.muted),
            ),
        );
    }

    // ---- Single big screen (4-18.6s)
    let big = R::new(240.0, 60.0, 1440.0, 810.0);
    let right_mon = R::new(990.0, 215.0, 870.0, 489.0);
    let left_mon = R::new(60.0, 215.0, 870.0, 489.0);
    if (4.0..18.6).contains(&t) {
        let enter = ramp(t, 4.0, 0.5);
        let to_dual = ramp(t, 18.0, 0.6);
        let screen = R::lerp(R::new(big.x, big.y + 30.0 * (1.0 - enter), big.w, big.h), right_mon, to_dual);
        let k = screen.w / big.w;
        let d = single_screen_desk(t.min(17.99), screen, k);
        root = root.child(desk_el(&d, k, t, enter, &p));
        if to_dual > 0.0 {
            let lm = R::new(left_mon.x - 200.0 * (1.0 - to_dual), left_mon.y, left_mon.w, left_mon.h);
            let (ld, _) = dual_desks(18.6, lm, right_mon, left_mon.w / big.w);
            root = root.child(desk_el(&ld, left_mon.w / big.w, t, to_dual, &p));
        }
    }
    // ---- Dual monitors (18.6-25.5s)
    if (18.6..25.6).contains(&t) {
        let k = left_mon.w / big.w;
        let a = 1.0 - ramp(t, 25.0, 0.5);
        let (ld, rd) = dual_desks(t, left_mon, right_mon, k);
        root = root.child(desk_el(&ld, k, t, a, &p)).child(desk_el(&rd, k, t, a, &p));
    }

    // ---- Captions & keycaps
    let caps: [(&str, &str, f32, f32); 6] = [
        ("Auto-tiling", "New windows snap into place, Hyprland-style", 4.8, 9.0),
        ("Glowing focus", "Always know where your keys are going", 9.2, 12.0),
        ("Three layouts, one key", "Dwindle  ·  Master  ·  Monocle", 12.2, 18.0),
        ("Workspaces span every monitor", "Switch instantly — no animations, no virtual desktops", 18.8, 22.4),
        ("Send windows anywhere", "Move to a workspace without leaving yours", 22.5, 25.0),
        ("", "", 0.0, 0.0),
    ];
    for (title, sub, a, b) in caps {
        let al = window(t, a, b, 0.3);
        if al > 0.0 {
            root = root.child(caption(title, sub, al, &p));
        }
    }
    let keys: [(&[&str], f32); 9] = [
        (&["Win", "Space"], 12.0),
        (&["Win", "Space"], 14.0),
        (&["Win", "Space"], 16.0),
        (&["Win", "2"], 19.5),
        (&["Win", "3"], 20.5),
        (&["Win", "1"], 21.5),
        (&["Win", "Shift", "3"], 22.5),
        (&["Win", "3"], 23.5),
        (&["Win", "→"], 9.5),
    ];
    for (k, at) in keys {
        let a = window(t, at - 0.05, at + 0.85, 0.12);
        if a > 0.0 {
            let y = if t < 18.0 { 875.0 } else { 735.0 };
            root = root.child(keycaps(k, a, y, &p));
        }
    }

    // ---- Outro (25.4-30s)
    if t >= 25.4 {
        let a = ramp(t, 25.4, 0.5) * (1.0 - ramp(t, 29.3, 0.6));
        root = root.child(logo(25.5, t, 170.0, W / 2.0, 250.0, &p).opacity(a));
        root = root.child(
            el().absolute(0.0, 360.0).size(W, 130.0).center().opacity(a).child(
                text("lattice").font(TextStyle::new(110.0, 800.0).tracking(-3.0)).color(p.fg),
            ),
        );
        root = root.child(
            el().absolute(0.0, 500.0).size(W, 40.0).center().opacity(a).child(
                text("Tiling for Windows 11  ·  built in Rust  ·  UI by Rune").font(TextStyle::new(26.0, 500.0)).color(p.muted),
            ),
        );
        let chips = ["Auto-tiling", "Instant workspaces", "Glowing focus", "Custom keybinds", "Top bar"];
        let cols = [p.accent, p.mauve, p.pink, p.teal, p.peach];
        let mut row = rune::row().gap(16.0).center_y();
        for (i, c) in chips.iter().enumerate() {
            let ca = ramp(t, 26.0 + i as f32 * BEAT, 0.25);
            row = row.child(
                el().px(22.0).py(12.0).radius(24.0).fill(p.panel2).border(1.5, cols[i].alpha(0.9)).opacity(ca).scale(lerp(0.8, 1.0, ca))
                    .child(text(c.to_string()).font(TextStyle::new(24.0, 650.0)).color(p.fg)),
            );
        }
        root = root.child(el().absolute(0.0, 610.0).size(W, 70.0).center().opacity(a).child(row));
    }
    // Final fade to black.
    let fade = ramp(t, 29.4, 0.6);
    if fade > 0.0 {
        root = root.child(boxed(R::new(0.0, 0.0, W, H)).fill(Color::BLACK).opacity(fade));
    }
    root
}

// ---------------------------------------------------------------------------------------------
// Music: an original 120 BPM synthwave loop in A minor, synthesized sample by sample.

const SR: u32 = 44_100;

fn midi(n: f32) -> f32 {
    440.0 * 2f32.powf((n - 69.0) / 12.0)
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0
    }
}

fn saw(ph: f32) -> f32 {
    2.0 * (ph - ph.floor()) - 1.0
}

struct Lp(f32);
impl Lp {
    fn run(&mut self, x: f32, fc: f32) -> f32 {
        let a = 1.0 - (-2.0 * std::f32::consts::PI * fc / SR as f32).exp();
        self.0 += a * (x - self.0);
        self.0
    }
}

fn write_music(path: &str) -> std::io::Result<()> {
    let dur = 30.0_f32;
    let n = (dur * SR as f32) as usize;
    let mut l = vec![0f32; n];
    let mut r = vec![0f32; n];
    let bar = 2.0_f32;
    // Am  F  C  G
    let chords: [[f32; 3]; 4] = [[57.0, 60.0, 64.0], [57.0, 60.0, 65.0], [55.0, 60.0, 64.0], [55.0, 59.0, 62.0]];
    let bass: [f32; 4] = [33.0, 29.0, 36.0, 31.0];
    let chord_at = |t: f32| ((t / bar) as usize) % 4;
    let mut rng = Rng(0x9E3779B97F4A7C15);

    // Arrangement flags.
    let kick_on = |t: f32| (4.0..17.5).contains(&t) || (18.0..28.0).contains(&t);
    let clap_on = |t: f32| (6.0..17.5).contains(&t) || (18.0..28.0).contains(&t);
    let hat_on = |t: f32| (4.0..17.75).contains(&t) || (18.0..28.0).contains(&t);
    let bass_on = |t: f32| (4.0..28.0).contains(&t);
    let arp_on = |t: f32| (0.0..30.0).contains(&t);

    let mut pad_lp = [Lp(0.0), Lp(0.0)];
    let mut bass_lp = Lp(0.0);
    let mut arp_lp = Lp(0.0);
    let mut hat_hp = Lp(0.0);
    let mut clap_lp = Lp(0.0);
    let mut clap_hp = Lp(0.0);
    let mut rise_lp = Lp(0.0);
    let mut crash_hp = Lp(0.0);
    let mut phases = [0f32; 16];
    let mut kick_phase = 0f32;
    let mut arp_buf = vec![0f32; n];

    for i in 0..n {
        let t = i as f32 / SR as f32;
        let beat_pos = t / BEAT;
        let dt_beat = t - beat_pos.floor() * BEAT;
        let c = chord_at(t);
        // Sidechain: duck pad/bass after each kick.
        let duck = if kick_on(t) { 1.0 - 0.55 * (-dt_beat * 9.0).exp() } else { 1.0 };

        // Kick.
        let mut kick = 0.0;
        if kick_on(t) && dt_beat < 0.45 {
            let f = 46.0 + 110.0 * (-dt_beat * 28.0).exp();
            kick_phase += f / SR as f32;
            kick = (kick_phase * std::f32::consts::TAU).sin() * (-dt_beat * 7.5).exp() * 0.95;
        } else {
            kick_phase = 0.0;
        }

        // Clap on beats 2 and 4.
        let mut clap = 0.0;
        let beat_in_bar = (beat_pos.floor() as i64).rem_euclid(4);
        if clap_on(t) && (beat_in_bar == 1 || beat_in_bar == 3) && dt_beat < 0.3 {
            let nz = rng.next();
            let lp = clap_lp.run(nz, 3500.0);
            let hp = lp - clap_hp.run(lp, 900.0);
            let env = (-dt_beat * 22.0).exp() + 0.5 * (-(dt_beat - 0.012).abs() * 120.0).exp();
            clap = hp * env * 0.55;
        }

        // Hats on 8ths, accented off-beats.
        let mut hat = 0.0;
        let dt8 = t - (t / (BEAT / 2.0)).floor() * (BEAT / 2.0);
        if hat_on(t) {
            let nz = rng.next();
            let hp = nz - hat_hp.run(nz, 7000.0);
            let off = ((t / (BEAT / 2.0)).floor() as i64) % 2 == 1;
            hat = hp * (-dt8 * 70.0).exp() * if off { 0.22 } else { 0.12 };
        }

        // Bass: 8th-note pulses on the chord root, octave jump on the last 8th.
        let mut bs = 0.0;
        if bass_on(t) {
            let eighth = ((t / (BEAT / 2.0)).floor() as i64) % 8;
            let note = bass[c] + if eighth == 7 { 12.0 } else { 0.0 };
            phases[0] += midi(note) / SR as f32;
            phases[1] += midi(note) * 1.005 / SR as f32;
            let raw = saw(phases[0]) + 0.6 * saw(phases[1]);
            let env = 0.35 + 0.65 * (-dt8 * 9.0).exp();
            let cutoff = 180.0 + 900.0 * (-dt8 * 14.0).exp();
            bs = bass_lp.run(raw, cutoff) * env * 0.45 * duck;
        }

        // Pad: detuned saws, opening filter over the intro.
        let open = 500.0 + 1700.0 * clamp01(t / 4.0);
        let mut pad = [0.0f32; 2];
        for (v, note) in chords[c].iter().enumerate() {
            for side in 0..2 {
                let det = if side == 0 { 0.997 } else { 1.003 };
                let idx = 2 + v * 2 + side;
                phases[idx] += midi(*note) * det / SR as f32;
                pad[side] += saw(phases[idx]);
            }
        }
        let pad_env = clamp01(t / 1.5) * (1.0 - ramp(t, 28.5, 1.5)) * 0.12 * duck;
        let pl = pad_lp[0].run(pad[0], open) * pad_env;
        let pr = pad_lp[1].run(pad[1], open) * pad_env;

        // Arp: 16th notes through the chord, quiet in the intro, full from the drop.
        if arp_on(t) {
            let sixteenth = (t / (BEAT / 4.0)).floor() as i64;
            let dt16 = t - sixteenth as f32 * (BEAT / 4.0);
            let pattern = [0usize, 1, 2, 3, 2, 1, 0, 2];
            let step = pattern[(sixteenth as usize) % pattern.len()];
            let note = if step == 3 { chords[c][0] + 12.0 } else { chords[c][step] + 12.0 };
            phases[8] += midi(note) / SR as f32;
            let sq = if (phases[8] - phases[8].floor()) < 0.5 { 1.0 } else { -1.0 };
            let lvl = if t < 4.0 { 0.04 + 0.04 * t / 4.0 } else if t < 12.0 { 0.06 } else if t < 25.0 { 0.1 } else { 0.07 };
            let bright = if t < 12.0 { 1400.0 } else { 3200.0 };
            arp_buf[i] = arp_lp.run(sq, bright) * (-dt16 * 14.0).exp() * lvl;
        }

        // Risers into 4s and 18s, crash at 28s.
        let mut fx = 0.0;
        for (a, b) in [(2.5f32, 4.0f32), (16.5, 18.0)] {
            if (a..b).contains(&t) {
                let x = (t - a) / (b - a);
                let nz = rng.next();
                fx += rise_lp.run(nz, 300.0 + 8000.0 * x * x) * x * x * 0.35;
            }
        }
        for at in [4.0f32, 18.0, 28.0] {
            if t >= at && t < at + 2.0 {
                let nz = rng.next();
                let hp = nz - crash_hp.run(nz, 4000.0);
                fx += hp * (-(t - at) * 2.2).exp() * 0.18;
            }
        }

        let mono = kick + clap + bs + fx;
        l[i] = mono + hat * 0.8 + pl;
        r[i] = mono + hat * 1.2 + pr;
    }

    // Ping-pong delay (dotted 8th) on the arp for space.
    let d = (0.375 * SR as f32) as usize;
    for i in 0..n {
        let dry = arp_buf[i];
        let wl = if i >= d { arp_buf[i - d] * 0.45 } else { 0.0 };
        let wr = if i >= 2 * d { arp_buf[i - 2 * d] * 0.3 } else { 0.0 };
        l[i] += dry + wl;
        r[i] += dry + wr + if i >= d { arp_buf[i - d] * 0.15 } else { 0.0 };
    }

    // Master: gentle saturation, normalize to -1 dBFS, short fade-out.
    let mut peak = 0f32;
    for i in 0..n {
        l[i] = (l[i] * 1.1).tanh();
        r[i] = (r[i] * 1.1).tanh();
        peak = peak.max(l[i].abs()).max(r[i].abs());
    }
    let gain = 0.89 / peak.max(1e-6);
    let mut data = Vec::with_capacity(n * 4);
    for i in 0..n {
        let t = i as f32 / SR as f32;
        let fade = 1.0 - ramp(t, 29.2, 0.8);
        for v in [l[i], r[i]] {
            let s = (v * gain * fade * 32767.0).clamp(-32768.0, 32767.0) as i16;
            data.extend_from_slice(&s.to_le_bytes());
        }
    }
    let mut wav = Vec::with_capacity(44 + data.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&2u16.to_le_bytes()); // stereo
    wav.extend_from_slice(&SR.to_le_bytes());
    wav.extend_from_slice(&(SR * 4).to_le_bytes());
    wav.extend_from_slice(&4u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(&data);
    if path.ends_with(".m4a") {
        return encode_aac(path, &data).map_err(|e| std::io::Error::other(e.to_string()));
    }
    std::fs::write(path, wav)
}

/// Encodes interleaved 16-bit stereo PCM to AAC in an .m4a with Windows' built-in Media
/// Foundation encoder (no ffmpeg build or download needed for the audio).
fn encode_aac(path: &str, pcm: &[u8]) -> windows::core::Result<()> {
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
    use windows::core::HSTRING;
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
        let full = std::path::absolute(path).unwrap_or_else(|_| path.into());
        let _ = std::fs::remove_file(&full);
        let writer = MFCreateSinkWriterFromURL(&HSTRING::from(full.as_os_str()), None, None)?;

        let out = MFCreateMediaType()?;
        out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        out.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
        out.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        out.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, SR)?;
        out.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
        out.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 24_000)?; // 192 kbps
        let stream = writer.AddStream(&out)?;

        let input = MFCreateMediaType()?;
        input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        input.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
        input.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        input.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, SR)?;
        input.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
        input.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 4)?;
        input.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, SR * 4)?;
        writer.SetInputMediaType(stream, &input, None)?;
        writer.BeginWriting()?;

        // ~0.1 s chunks; times are in 100 ns units.
        let frames_per_chunk = (SR / 10) as usize;
        let mut frame = 0usize;
        for chunk in pcm.chunks(frames_per_chunk * 4) {
            let buf = MFCreateMemoryBuffer(chunk.len() as u32)?;
            let mut ptr = std::ptr::null_mut();
            buf.Lock(&mut ptr, None, None)?;
            std::ptr::copy_nonoverlapping(chunk.as_ptr(), ptr, chunk.len());
            buf.Unlock()?;
            buf.SetCurrentLength(chunk.len() as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buf)?;
            let frames = chunk.len() / 4;
            sample.SetSampleTime((frame as i64 * 10_000_000) / SR as i64)?;
            sample.SetSampleDuration((frames as i64 * 10_000_000) / SR as i64)?;
            writer.WriteSample(stream, &sample)?;
            frame += frames;
        }
        writer.Finalize()?;
        MFShutdown()?;
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--music") {
        // `.wav` for PCM, `.m4a` for AAC via Media Foundation.
        let path = args.get(i + 1).map(String::as_str).unwrap_or("music.wav");
        write_music(path).expect("write music");
        println!("wrote {path}");
        return;
    }
    rune::run(AppSpec::<State> { poll: Some(poll), ..AppSpec::new("lattice teaser", (W, H), init, view) });
}
