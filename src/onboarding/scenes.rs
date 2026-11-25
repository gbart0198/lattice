//! The animated illustration at the top of each slide. Each is a pure function of the slide's
//! `tick` (it advances every `TICK` seconds while the slide is shown): the view says where
//! things are at that beat, and Rune's springs carry them there.

use rune::{Color, El, Spring, TextStyle, Transition, col, el, row, text};

use super::Pal;
use super::parts::{self, APPS, App};

/// The stage every illustration fills.
pub const W: f32 = 880.0;
pub const H: f32 = 320.0;

pub fn scene(p: &Pal, slide: usize, tick: u32) -> El {
    let content = match slide {
        0 => welcome(p, tick),
        1 => tiling(p, tick),
        2 => workspaces(p, tick),
        3 => keybinds(p, tick),
        4 => launcher(p, tick),
        5 => bar_and_border(p, tick),
        6 => settings(p, tick),
        _ => finish(p, tick),
    };
    el().size(W, H).child(parts::wallpaper(p, W, H)).child(content)
}

fn stage() -> El {
    el().key("scene").absolute(0.0, 0.0).size(W, H)
}

// ---------------------------------------------------------------------------------------------
// 0. Welcome: the logo assembles, then what lattice does

fn welcome(p: &Pal, tick: u32) -> El {
    let shown = (tick as usize + 1).min(4);
    let (lw, lh) = (150.0, 108.0);
    let halo = el()
        .absolute(W / 2.0 - 220.0, 20.0)
        .size(440.0, 220.0)
        .radial_gradient([0.5, 0.5], [220.0, 110.0], [(0.0, p.accent.alpha(0.30)), (0.6, p.accent2.alpha(0.08)), (1.0, p.accent2.alpha(0.0))]);
    let features = ["Auto-tiling", "Workspaces", "Launcher", "Focus glow", "Keyboard first"];
    let mut chips = row().gap(10.0).center_y();
    for (i, f) in features.iter().enumerate() {
        if tick as usize >= 4 + i {
            chips = chips.child(
                row()
                    .key(format!("chip-{i}"))
                    .h(30.0)
                    .px(14.0)
                    .center()
                    .radius(15.0)
                    .fill(p.fg.alpha(0.06))
                    .border(1.0, p.fg.alpha(0.10))
                    .transition(Transition::offset(0.0, 10.0).combined(Transition::opacity()))
                    .child(text(f.to_string()).font(TextStyle::new(12.5, 560.0)).color(p.fg)),
            );
        }
    }
    stage()
        .child(halo)
        .child(el().absolute((W - lw) / 2.0, 56.0).child(parts::logo(p, lw, lh, shown)))
        .child(row().absolute(0.0, 222.0).w(W).h(40.0).center().child(chips))
}

// ---------------------------------------------------------------------------------------------
// 1. Auto-tiling: windows arrive and split the screen, then the layout cycles

fn tiling(p: &Pal, tick: u32) -> El {
    let step = (tick / 2) % 10;
    let (n, layout) = match step {
        0..=3 => (step as usize + 1, "dwindle"),
        4 => (4, "dwindle"),
        5 | 6 => (4, "master"),
        _ => (4, "monocle"),
    };
    let area = (24.0, 24.0, W - 48.0, H - 48.0);
    let rects = match layout {
        "master" => parts::master(n, area, 10.0),
        "monocle" => vec![area; n],
        _ => parts::dwindle(n, area, 10.0),
    };
    let focused = n - 1;
    let mut s = stage().child(parts::glow_halo(p, rects[focused]));
    // Monocle shows only the focused window; the others fade behind it.
    for (i, r) in rects.iter().enumerate() {
        let mut w = parts::window(p, format!("tile-{i}"), *r, &APPS[i], i == focused);
        if layout == "monocle" && i != focused {
            w = w.opacity(0.0);
        }
        s = s.child(w);
    }
    s = s.child(parts::glow_ring(p, rects[focused]));
    let label = row()
        .gap(8.0)
        .center_y()
        .child(parts::layout_glyph(p, layout, p.accent))
        .child(text(layout.to_string()).key(format!("layout-{layout}")).font(TextStyle::new(12.0, 640.0)).color(p.fg).transition(Transition::opacity()));
    s.child(el().absolute(W - 160.0, H - 64.0).w(124.0).child(row().w_full().justify_end().child(parts::tag(p, label))))
}

// ---------------------------------------------------------------------------------------------
// 2. Workspaces: the bar's pills, and a different set of windows on each

fn workspace_apps() -> [Vec<&'static App>; 4] {
    [vec![&APPS[1], &APPS[0]], vec![&APPS[2], &APPS[3], &APPS[4]], vec![&APPS[5]], vec![]]
}

fn workspaces(p: &Pal, tick: u32) -> El {
    let ws = ((tick / 3) % 3) as usize;
    let apps = workspace_apps();
    let area = (16.0, 46.0, W - 32.0, H - 62.0);
    let rects = parts::dwindle(apps[ws].len(), area, 10.0);
    let mut s = stage().child(parts::bar(p, W, &apps, ws, apps[ws][0].title, "dwindle"));
    for (i, (r, a)) in rects.iter().zip(&apps[ws]).enumerate() {
        s = s.child(parts::window(p, format!("ws{ws}-{i}"), *r, a, i == 0));
    }
    let n = (ws + 1).to_string();
    let press = parts::tag(p, row().gap(8.0).center_y().child(parts::chord(p, &["Win", &n])).child(
        text(format!("Workspace {n}")).font(TextStyle::new(12.0, 600.0)).color(p.fg),
    ))
    .key(format!("press-{ws}"))
    .h(40.0)
    .radius(20.0)
    .transition(Transition::offset(0.0, 12.0).combined(Transition::opacity()));
    s.child(row().absolute(0.0, H - 64.0).w(W).center().child(press))
}

// ---------------------------------------------------------------------------------------------
// 3. Keybinds: a cheat sheet whose highlight walks across it

pub const BINDS: [(&[&str], &str); 8] = [
    (&["Win", "←", "→"], "Move focus"),
    (&["Win", "Shift", "←"], "Swap windows"),
    (&["Win", "1…9"], "Switch workspace"),
    (&["Win", "\\"], "Cycle layout"),
    (&["Win", "F"], "Fullscreen"),
    (&["Win", "Shift", "Space"], "Float a window"),
    (&["Win", "Enter"], "Open a terminal"),
    (&["Win", "Q"], "Close the window"),
];

fn keybinds(p: &Pal, tick: u32) -> El {
    let active = ((tick / 2) % BINDS.len() as u32) as usize;
    let (cols, gap) = (4usize, 14.0);
    let cw = (W - 64.0 - gap * (cols as f32 - 1.0)) / cols as f32;
    let ch = 112.0;
    let top = (H - 2.0 * ch - gap) / 2.0;
    let mut s = stage();
    let pos = |i: usize| (32.0 + (i % cols) as f32 * (cw + gap), top + (i / cols) as f32 * (ch + gap));
    // The highlight is one element that glides from card to card.
    let (hx, hy) = pos(active);
    s = s.child(
        el().key("bind-hl")
            .absolute(hx - 1.0, hy - 1.0)
            .size(cw + 2.0, ch + 2.0)
            .radius(15.0)
            .border(1.5, p.accent)
            .fill(p.accent.alpha(0.10))
            .shadow(0.0, 30.0, p.accent.alpha(0.40))
            .motion(parts::GLIDE),
    );
    for (i, (keys, what)) in BINDS.iter().enumerate() {
        let (x, y) = pos(i);
        let on = i == active;
        s = s.child(
            col()
                .key(format!("bind-{i}"))
                .absolute(x, y)
                .size(cw, ch)
                .padding(16.0)
                .gap(14.0)
                .justify_between()
                .radius(14.0)
                .fill(p.win.alpha(if on { 0.6 } else { 0.92 }))
                .border(1.0, p.fg.alpha(if on { 0.0 } else { 0.07 }))
                .motion(parts::GLIDE)
                .child(parts::chord(p, keys))
                .child(text(what.to_string()).font(TextStyle::new(14.0, 600.0)).color(if on { p.fg } else { p.muted })),
        );
    }
    s
}

// ---------------------------------------------------------------------------------------------
// 4. Launcher: someone types "vsc", then "f readme"

struct Hit {
    name: &'static str,
    detail: &'static str,
    /// Byte offsets of the matched letters.
    marks: &'static [usize],
    tint: &'static str,
}

const fn hit(name: &'static str, detail: &'static str, marks: &'static [usize], tint: &'static str) -> Hit {
    Hit { name, detail, marks, tint }
}

/// What the launcher shows at a beat: file mode, the query, and the results.
fn launcher_beat(tick: u32) -> (bool, &'static str, Vec<Hit>) {
    const RECENT: [Hit; 4] = [
        hit("Windows Terminal", "Opened 12 times", &[], "#a6e3a1"),
        hit("Visual Studio Code", "Opened 9 times", &[], "#89b4fa"),
        hit("Browser", "Opened 7 times", &[], "#fab387"),
        hit("Notes", "Opened 3 times", &[], "#f9e2af"),
    ];
    const README: [Hit; 3] = [
        hit("README.md", "~\\dev\\lattice", &[0, 1, 2, 3, 4, 5], "#cdd6f4"),
        hit("readme.txt", "~\\Downloads\\fonts", &[0, 1, 2, 3, 4, 5], "#cdd6f4"),
        hit("Readme", "~\\Documents\\notes", &[0, 1, 2, 3, 4, 5], "#f9e2af"),
    ];
    match tick % 26 {
        0..=2 => (false, "", RECENT.into()),
        3 => (false, "v", vec![
            hit("Visual Studio Code", "Application", &[0], "#89b4fa"),
            hit("VLC media player", "Application", &[0], "#fab387"),
            hit("Voice Recorder", "Application", &[0], "#f5c2e7"),
        ]),
        4 => (false, "vs", vec![
            hit("Visual Studio Code", "Application", &[0, 7], "#89b4fa"),
            hit("Visual Studio Installer", "Application", &[0, 7], "#cba6f7"),
        ]),
        5..=9 => (false, "vsc", vec![hit("Visual Studio Code", "Application", &[0, 7, 14], "#89b4fa")]),
        10..=11 => (false, "", RECENT.into()),
        12 => (false, "f", vec![hit("Files", "Application", &[0], "#94e2d5"), hit("Firefox", "Application", &[0], "#fab387")]),
        13 => (true, "", vec![]),
        14 => (true, "r", vec![hit("Recordings", "~\\Videos", &[0], "#cdd6f4"), hit("README.md", "~\\dev\\lattice", &[0], "#cdd6f4")]),
        15 => (true, "re", vec![hit("README.md", "~\\dev\\lattice", &[0, 1], "#cdd6f4"), hit("Recordings", "~\\Videos", &[0, 1], "#cdd6f4")]),
        16 => (true, "rea", README.into()),
        17 => (true, "read", README.into()),
        _ => (true, "readme", README.into()),
    }
}

fn marked(p: &Pal, name: &str, marks: &[usize]) -> El {
    let mut r = row();
    let mut run = String::new();
    let mut run_on = false;
    let flush = |r: El, run: &mut String, on: bool| {
        if run.is_empty() {
            return r;
        }
        let t = text(std::mem::take(run)).font(TextStyle::new(13.5, if on { 700.0 } else { 540.0 })).color(if on { p.accent } else { p.fg });
        r.child(t)
    };
    for (i, ch) in name.char_indices() {
        let on = marks.contains(&i);
        if on != run_on {
            r = flush(r, &mut run, run_on);
            run_on = on;
        }
        run.push(ch);
    }
    flush(r, &mut run, run_on)
}

fn launcher(p: &Pal, tick: u32) -> El {
    let (files, query, hits) = launcher_beat(tick);
    // A dimmed desktop behind it.
    let area = (16.0, 16.0, W - 32.0, H - 32.0);
    let mut s = stage();
    for (i, r) in parts::dwindle(3, area, 10.0).into_iter().enumerate() {
        s = s.child(parts::window(p, format!("bg-{i}"), r, &APPS[i + 2], false).opacity(0.35));
    }
    let pw = 460.0;
    let lens = el().size(12.0, 12.0).radius(6.0).border(1.8, p.muted);
    let mut field = row().h(50.0).px(16.0).gap(10.0).center_y().child(lens);
    if files {
        field = field.child(
            row()
                .key("files-chip")
                .h(22.0)
                .px(8.0)
                .center()
                .radius(6.0)
                .fill(p.accent.alpha(0.18))
                .transition(Transition::scale(0.8))
                .child(text("Files").font(TextStyle::new(11.5, 680.0)).color(p.accent)),
        );
    }
    let typed = if query.is_empty() {
        text(if files { "Search files and folders" } else { "Search apps" }).font(TextStyle::new(15.0, 480.0)).color(p.muted.alpha(0.7))
    } else {
        text(query.to_string()).font(TextStyle::new(15.0, 520.0)).color(p.fg)
    };
    const BLINK: &[rune::Beat] = &[
        rune::Beat::new(0.0).opacity(1.0),
        rune::Beat::new(0.5).opacity(1.0),
        rune::Beat::new(0.55).opacity(0.0),
        rune::Beat::new(1.0).opacity(0.0),
        rune::Beat::new(1.05).opacity(1.0),
    ];
    let caret = el().key("caret").w(2.0).h(18.0).radius(1.0).fill(p.accent).sequence(rune::Sequence::new(BLINK).repeating());
    field = if query.is_empty() { field.child(caret).child(typed) } else { field.child(typed).child(caret) };

    let section = if files { "Files" } else if query.is_empty() { "Recent" } else { "Apps" };
    // A fixed height (three rows), so the panel stays still while results come and go.
    let hits: Vec<Hit> = hits.into_iter().take(3).collect();
    let mut list = col().h(26.0 + 3.0 * 46.0 + 8.0).px(8.0).pb(8.0).gap(2.0).child(
        text(section.to_string()).px(10.0).h(26.0).font(TextStyle::new(11.0, 640.0)).color(p.muted),
    );
    if hits.is_empty() {
        list = list.child(text("Keep typing…").key("empty").px(10.0).h(42.0).font(TextStyle::new(13.0, 500.0)).color(p.muted).transition(Transition::opacity()));
    }
    for (i, h) in hits.iter().enumerate() {
        let c = crate::bar::color(h.tint, p.accent);
        let badge = if files {
            // A page with a folded corner for files.
            el().size(26.0, 26.0).radius(6.0).fill(c.alpha(0.16)).border(1.0, c.alpha(0.4)).child(
                el().absolute(7.0, 7.0).size(12.0, 2.0).radius(1.0).fill(c.alpha(0.7)),
            ).child(el().absolute(7.0, 12.0).size(9.0, 2.0).radius(1.0).fill(c.alpha(0.5))).child(el().absolute(7.0, 17.0).size(11.0, 2.0).radius(1.0).fill(c.alpha(0.5)))
        } else {
            row().size(26.0, 26.0).center().radius(7.0).fill(c).child(
                text(h.name[..1].to_string()).font(TextStyle::new(13.0, 760.0)).color(Color::BLACK.alpha(0.7)),
            )
        };
        let sel = i == 0;
        list = list.child(
            row()
                .key(format!("hit-{}", h.name))
                .h(44.0)
                .px(10.0)
                .gap(12.0)
                .center_y()
                .radius(9.0)
                .fill(if sel { p.accent.alpha(0.16) } else { Color::TRANSPARENT })
                .transition(Transition::offset(0.0, 6.0).combined(Transition::opacity()))
                .motion(Spring::SNAPPY)
                .child(badge)
                .child(col().gap(1.0).grow(1.0).shrink().child(marked(p, h.name, h.marks)).child(
                    text(h.detail.to_string()).font(TextStyle::new(11.0, 480.0)).color(p.muted),
                ))
                .child(if sel { parts::key(p, "Enter") } else { el() }),
        );
    }
    let foot = row()
        .h(34.0)
        .px(16.0)
        .gap(16.0)
        .center_y()
        .fill(p.fg.alpha(0.03))
        .child(text("↑↓ choose").font(TextStyle::new(11.0, 520.0)).color(p.muted))
        .child(text("Enter open").font(TextStyle::new(11.0, 520.0)).color(p.muted))
        .child(text(if files { "Ctrl+Enter show in folder" } else { "f + space: files" }).font(TextStyle::new(11.0, 520.0)).color(p.muted))
        .child(el().grow(1.0))
        .child(text("Esc close").font(TextStyle::new(11.0, 520.0)).color(p.muted));
    let panel = col()
        .key("launcher")
        .absolute((W - pw) / 2.0, 26.0)
        .w(pw)
        .radius(14.0)
        .fill(p.bg)
        .border(1.0, p.fg.alpha(0.10))
        .shadow(18.0, 50.0, Color::BLACK.alpha(0.55))
        .clip_rounded(14.0)
        .motion(Spring::SNAPPY)
        .child(field)
        .child(el().h(1.0).w_full().fill(p.fg.alpha(0.07)))
        .child(list)
        .child(foot);
    s.child(panel)
}

// ---------------------------------------------------------------------------------------------
// 5. The bar and the focus glow following focus

fn bar_and_border(p: &Pal, tick: u32) -> El {
    let order = [0usize, 1, 2, 1];
    let focus = order[((tick / 3) % 4) as usize];
    let apps = [&APPS[1], &APPS[0], &APPS[2]];
    let all = [vec![apps[0], apps[1], apps[2]], vec![&APPS[3]], vec![&APPS[4]], vec![]];
    let area = (16.0, 46.0, W - 32.0, H - 62.0);
    let rects = parts::dwindle(3, area, 10.0);
    let mut s = stage().child(parts::glow_halo(p, rects[focus])).child(parts::bar(p, W, &all, 0, apps[focus].title, "dwindle"));
    for (i, r) in rects.iter().enumerate() {
        s = s.child(parts::window(p, format!("w-{i}"), *r, apps[i], i == focus));
    }
    s.child(parts::glow_ring(p, rects[focus]))
}

// ---------------------------------------------------------------------------------------------
// 6. Settings: tabs slide, each showing a few of its controls

const TABS: [&str; 5] = ["General", "Apps", "Bar", "Border", "Keybinds"];

fn switch(p: &Pal, on: bool) -> El {
    el().size(34.0, 20.0).radius(10.0).fill(if on { p.accent } else { p.occupied }).child(
        el().absolute(if on { 16.0 } else { 2.0 }, 2.0).size(16.0, 16.0).radius(8.0).fill(Color::WHITE),
    )
}

fn slider(p: &Pal, frac: f32) -> El {
    el().w(150.0).h(4.0).radius(2.0).fill(p.occupied).child(el().h(4.0).w(150.0 * frac).radius(2.0).fill(p.accent)).child(
        el().absolute(150.0 * frac - 8.0, -6.0).size(16.0, 16.0).radius(8.0).fill(Color::WHITE).shadow(1.0, 4.0, Color::BLACK.alpha(0.4)),
    )
}

fn seg(p: &Pal, items: &[&str], on: usize) -> El {
    items.iter().enumerate().fold(row().p_box(p), |r, (i, it)| {
        r.child(
            row()
                .h(24.0)
                .px(10.0)
                .center()
                .radius(6.0)
                .fill(if i == on { p.fg.alpha(0.14) } else { Color::TRANSPARENT })
                .child(text(it.to_string()).font(TextStyle::new(11.5, 580.0)).color(if i == on { p.fg } else { p.muted })),
        )
    })
}

trait PBox {
    fn p_box(self, p: &Pal) -> El;
}
impl PBox for El {
    fn p_box(self, p: &Pal) -> El {
        self.padding(2.0).radius(8.0).fill(p.bg.alpha(0.8)).border(1.0, p.fg.alpha(0.06))
    }
}

fn mono(p: &Pal, s: &str) -> El {
    row().h(26.0).px(10.0).center_y().radius(6.0).fill(p.bg.alpha(0.8)).border(1.0, p.fg.alpha(0.06)).child(
        text(s.to_string()).font(TextStyle::new(11.5, 560.0).mono()).color(p.fg),
    )
}

fn swatch(p: &Pal, c: Color, hex: &str) -> El {
    row().gap(8.0).center_y().child(el().size(18.0, 18.0).radius(5.0).fill(c)).child(mono(p, hex))
}

fn settings(p: &Pal, tick: u32) -> El {
    let tab = ((tick / 3) % TABS.len() as u32) as usize;
    let rows: Vec<(&str, El)> = match tab {
        0 => vec![
            ("Default layout", seg(p, &["dwindle", "master", "monocle"], 0)),
            ("Gap between windows", slider(p, 0.3)),
            ("Mouse follows focus", switch(p, true)),
        ],
        1 => vec![
            ("Discord.exe", row().gap(8.0).center_y().child(text("workspace").font(TextStyle::new(11.5, 500.0)).color(p.muted)).child(mono(p, "3"))),
            ("Spotify.exe", row().gap(8.0).center_y().child(text("workspace").font(TextStyle::new(11.5, 500.0)).color(p.muted)).child(mono(p, "4"))),
            ("Outlook.exe", row().gap(8.0).center_y().child(text("workspace").font(TextStyle::new(11.5, 500.0)).color(p.muted)).child(mono(p, "2"))),
        ],
        2 => vec![
            ("Show the bar", switch(p, true)),
            ("Font", mono(p, "Inter")),
            ("Accent", swatch(p, p.accent, &p.accent_hex)),
        ],
        3 => vec![
            ("Glow around the focused window", switch(p, true)),
            ("Glow size", slider(p, 0.45)),
            ("Colour", swatch(p, p.accent, &p.accent_hex)),
        ],
        _ => vec![
            ("super+space", mono(p, "launcher")),
            ("super+1", mono(p, "workspace 1")),
            ("super+f", mono(p, "toggle-fullscreen")),
        ],
    };
    let (ww, wh) = (600.0, H - 36.0);
    let tw = 92.0;
    let mut tabs = el().w(tw * TABS.len() as f32 + 4.0).h(30.0).p_box(p).child(
        el().key("tab-pill").absolute(2.0 + tw * tab as f32, 2.0).size(tw, 26.0).radius(6.0).fill(p.fg.alpha(0.14)).motion(parts::GLIDE),
    );
    for (i, t) in TABS.iter().enumerate() {
        tabs = tabs.child(
            row().absolute(2.0 + tw * i as f32, 2.0).size(tw, 26.0).center().child(
                text(t.to_string()).font(TextStyle::new(12.0, 600.0)).color(if i == tab { p.fg } else { p.muted }),
            ),
        );
    }
    let mut card = col().radius(12.0).fill(p.win).border(1.0, p.fg.alpha(0.06));
    let n = rows.len();
    for (i, (label, control)) in rows.into_iter().enumerate() {
        card = card.child(
            row().h(48.0).px(16.0).center_y().justify_between().child(text(label.to_string()).font(TextStyle::new(13.0, 540.0)).color(p.fg)).child(control),
        );
        if i + 1 < n {
            card = card.child(el().h(1.0).ml(16.0).mr(16.0).fill(p.fg.alpha(0.06)));
        }
    }
    let body = col().key(format!("tab-body-{tab}")).gap(8.0).transition(Transition::offset(0.0, 8.0).combined(Transition::opacity())).child(
        text(TABS[tab].to_uppercase()).ml(4.0).font(TextStyle::new(10.5, 700.0).tracking(0.8)).color(p.muted),
    ).child(card);
    let header = row()
        .gap(10.0)
        .center_y()
        .child(el().size(24.0, 24.0).radius(6.0).linear_gradient(2.2, [(0.0, p.accent), (1.0, p.accent2)]))
        .child(text("lattice settings").font(TextStyle::new(15.0, 700.0)).color(p.fg))
        .child(el().grow(1.0))
        .child(parts::chord(p, &["Win", "Alt", ","]));
    let window = col()
        .absolute((W - ww) / 2.0, (H - wh) / 2.0)
        .size(ww, wh)
        .padding(18.0)
        .gap(14.0)
        .radius(14.0)
        .fill(p.bg)
        .border(1.0, p.fg.alpha(0.09))
        .shadow(16.0, 44.0, Color::BLACK.alpha(0.5))
        .child(header)
        .child(tabs)
        .child(body);
    stage().child(window)
}

// ---------------------------------------------------------------------------------------------
// 7. Finish: the shortcuts worth remembering, ticked off one by one

pub const ESSENTIALS: [(&[&str], &str); 4] = [
    (&["Win", "Space"], "Open the launcher"),
    (&["Win", "1…9"], "Switch workspace"),
    (&["Win", "←", "→", "↑", "↓"], "Move focus"),
    (&["Win", "Alt", ","], "Open settings"),
];

fn finish(p: &Pal, tick: u32) -> El {
    let (cw, rh) = (520.0, 52.0);
    let mut card = col().key("essentials").w(cw).padding(10.0).gap(4.0).radius(16.0).fill(p.win.alpha(0.92)).border(1.0, p.fg.alpha(0.08)).shadow(16.0, 44.0, Color::BLACK.alpha(0.45));
    for (i, (keys, what)) in ESSENTIALS.iter().enumerate() {
        let done = tick as usize > i;
        let check = row()
            .size(22.0, 22.0)
            .center()
            .radius(11.0)
            .fill(if done { p.accent } else { Color::TRANSPARENT })
            .border(1.5, if done { p.accent } else { p.fg.alpha(0.18) })
            .motion(Spring::BOUNCY)
            .child(text(if done { "✓" } else { "" }).font(TextStyle::new(12.0, 800.0)).color(p.bg));
        card = card.child(
            row()
                .h(rh)
                .px(12.0)
                .gap(14.0)
                .center_y()
                .radius(10.0)
                .fill(if done { p.accent.alpha(0.06) } else { Color::TRANSPARENT })
                .motion(parts::GLIDE)
                .child(check)
                .child(text(what.to_string()).grow(1.0).font(TextStyle::new(14.0, 600.0)).color(if done { p.fg } else { p.muted }))
                .child(parts::chord(p, keys)),
        );
    }
    let halo = el()
        .absolute(W / 2.0 - 360.0, -40.0)
        .size(720.0, 400.0)
        .radial_gradient([0.5, 0.5], [360.0, 200.0], [(0.0, p.accent2.alpha(0.18)), (1.0, p.accent2.alpha(0.0))]);
    stage().child(halo).child(row().absolute(0.0, 0.0).size(W, H).center().child(card))
}
