//! The Setups tab's view: the list page (a row a setup, with a preview and its shortcut), and a
//! setup's own page with its header row (back, name, shortcut, Apply, Delete), the workspace cards, the toolbar (tiling, the
//! editor's help, the add picker), the monitor bar and carousel around the canvas with a button
//! either side to add a monitor, and the inspector for the selected window.

use rune::tok::{space, type_};
use rune::{Color, Cursor, El, Icon, Spring, TextStyle, Tx, col, el, icon, row, text};
use rune_kit::{Button, ButtonVariant, Combobox, ComboboxOption, IconButton, IconButtonVariant, Placement, Popover, Select, TextInput};

use super::canvas::{SLIDE_GAP, SLIDE_W, canvas, color, defaults};
use super::catalog;
use super::io::{capture, restore};
use super::model::{Editor, SetupEdit, setup_config, to_config, unique_name};
use crate::config::{self, NewWindow, Setup, SetupWorkspace};
use crate::layout::LayoutKind;
use crate::settings::{LAYOUTS, P, S, kit, section};
use crate::setups::{self, Defaults};

/// How the carousel slides, and how the monitors either side shrink and dim.
const SLIDE: Spring = Spring::new(0.45, 0.06);

const OTHER: &str = "__other__";

pub(super) static PLUS: Icon = Icon {
    name: "setups-plus",
    view: 16.0,
    body: r#"<path d="M8 3.5V12.5M3.5 8H12.5" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>"#,
};
pub(super) static TRASH: Icon = Icon {
    name: "setups-trash",
    view: 16.0,
    body: r#"<path d="M3 4.5H13M6.5 4.5V3H9.5V4.5M4.5 4.5L5.2 13H10.8L11.5 4.5M7 7V11M9 7V11" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};
static BACK: Icon = Icon {
    name: "setups-back",
    view: 16.0,
    body: r#"<path d="M10 3.5L5.5 8L10 12.5" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};
static INFO: Icon = Icon {
    name: "setups-info",
    view: 16.0,
    body: r#"<circle cx="8" cy="8" r="6" stroke="currentColor" stroke-width="1.2" fill="none"/><path d="M8 7.2V11" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/><circle cx="8" cy="5" r="0.8" fill="currentColor"/>"#,
};

/// The tiling dropdown's names for `LAYOUTS`.
const LAYOUT_NAMES: [&str; 3] = ["Dwindle", "Master", "Monocle"];

/// A workspace of `setup` drawn small: each of its monitors.
fn ws_thumb(ed: &Editor, setup: &SetupEdit, n: usize, d: &Defaults) -> setups::Thumb {
    setups::ws_thumb(&setup_config(setup), n, &ed.areas(setup.monitors.max(1)), ed.primary, d)
}

pub(super) fn small(t: &str, c: Color) -> El {
    text(t.to_string()).font(TextStyle::new(11.5, 400.0)).color(c)
}

pub(crate) fn tab(s: &S, p: &P, overlays: &mut Vec<El>) -> Vec<El> {
    if let Some(i) = s.ed.sel.filter(|i| *i < s.ed.list.len()) {
        return editor_page(s, p, i, overlays);
    }
    vec![
        col().key("setups-help").gap(4.0).mx(space::S3).w_full().child(
            small(
                "Save a set of apps with their workspaces and layout. Restoring one starts what isn't running and moves everything into place: with its shortcut, by typing \u{201c}workspaces\u{201d} in the launcher, or `lattice msg restore-setup NAME`.",
                p.t.text_secondary,
            )
            .w_full()
            .wrap(),
        ),
        list_section(s, p),
    ]
}

fn list_section(s: &S, p: &P) -> El {
    let k = kit(p);
    let mut rows = Vec::new();
    for (i, setup) in s.ed.list.iter().enumerate() {
        let preview = SetupPreview::of(setup);
        let name = setup.name.text.trim();
        let title = text(if name.is_empty() { "Untitled".to_string() } else { name.to_string() }).font(TextStyle::new(13.5, 600.0)).color(p.t.text);
        let detail = small(&setups::summary(&preview.setup), p.t.text_secondary).truncate();
        let key = setup.key.text.trim();
        let mut right = row().gap(space::S2).center_y();
        if !key.is_empty() {
            right = right.child(key_caps(key, p));
        }
        let restore_name = name.to_string();
        right = right
            .child(
                Button::new("Apply")
                    .variant(ButtonVariant::Outline)
                    .disabled(name.is_empty())
                    .on_press(move |s: &mut S| restore(s, restore_name.clone()))
                    .build(&k),
            )
            .child(text("\u{203a}").font(TextStyle::new(20.0, 400.0)).color(p.t.text_secondary).ml(2.0));
        rows.push(
            row()
                .key(format!("setup-{}", setup.id))
                .hit()
                .cursor(Cursor::Pointer)
                .label(format!("Edit {name}"))
                .px(space::S3)
                .py(space::S2)
                .gap(space::S3)
                .center_y()
                .radius(8.0)
                .hover(p.t.text.alpha(0.05))
                .on_press(move |s: &mut S| s.ed.select(i))
                .child(mini_screens(setup, s, p))
                .child(col().gap(2.0).grow(1.0).shrink().child(title).child(detail))
                .child(right),
        );
    }
    if rows.is_empty() {
        rows.push(
            col()
                .key("setups-empty")
                .px(space::S3)
                .py(16.0)
                .gap(4.0)
                .child(text("No setups yet").font(type_::BODY).color(p.t.text))
                .child(small("Make one from scratch, or save the windows you have open now.", p.t.text_secondary)),
        );
    }
    let new = Button::new("New setup").on_press(|s: &mut S| {
        let name = unique_name(&s.ed.list, "New workspace");
        let id = s.ed.id();
        let _ = id;
        let blank = s.ed.blank(name);
        s.ed.list.push(blank);
        let i = s.ed.list.len() - 1;
        s.ed.select(i);
    });
    let capture_label = if s.ed.capturing { "Reading windows…" } else { "Save what's open as a setup" };
    let cap = Button::new(capture_label).variant(ButtonVariant::Outline).disabled(s.ed.capturing).on_press(|s: &mut S| {
        s.ed.capturing = true;
        capture();
    });
    rows.push(row().key("setups-add").px(space::S3).py(space::S2).gap(space::S2).child(new.build(&k)).child(cap.build(&k)));
    section(p, "setups-list", &format!("Setups ({})", s.ed.list.len()), rows)
}

/// One setup, on a page of its own.
fn editor_page(s: &S, p: &P, i: usize, overlays: &mut Vec<El>) -> Vec<El> {
    vec![header(s, p, i), layout_section(s, p, overlays)]
}

/// A setup in the form `setups` previews take.
struct SetupPreview {
    setup: Setup,
}

impl SetupPreview {
    fn of(e: &SetupEdit) -> Self {
        SetupPreview { setup: setup_config(e) }
    }
}

/// Up to three little screens with each workspace's tiles.
fn mini_screens(setup: &SetupEdit, s: &S, p: &P) -> El {
    let d = defaults(s);
    let mut r = row().gap(4.0).center_y().w(3.0 * 40.0 + 8.0);
    let numbers: Vec<usize> = setups::numbers(&setup.workspaces.iter().map(to_config).collect::<Vec<_>>());
    for &n in numbers.iter().take(3) {
        r = r.child(mini_thumb(&ws_thumb(&s.ed, setup, n, &d), 40.0, 24.0, p));
    }
    if numbers.is_empty() {
        r = r.child(el().size(40.0, 22.0).radius(4.0).border(p.hair, p.t.card_border));
    }
    r
}

fn key_caps(chord: &str, p: &P) -> El {
    chord.split('+').map(str::trim).filter(|k| !k.is_empty()).fold(row().gap(3.0).center_y(), |r, k| {
        let label = match k.to_ascii_lowercase().as_str() {
            "super" | "win" | "mod4" | "meta" => "Win".to_string(),
            "ctrl" | "control" => "Ctrl".to_string(),
            "alt" => "Alt".to_string(),
            "shift" => "Shift".to_string(),
            other if other.len() == 1 => other.to_uppercase(),
            other => other.to_string(),
        };
        r.child(
            row()
                .h(20.0)
                .min_w(20.0)
                .px(6.0)
                .radius(5.0)
                .center()
                .fill(p.t.bg)
                .border(p.hair, p.t.card_border)
                .child(text(label).font(TextStyle::new(11.0, 600.0)).color(p.t.text_secondary)),
        )
    })
}

/// The setup's header, one row: back to the list, its name and shortcut, and Apply and Delete.
/// A bad shortcut says why under it.
fn header(s: &S, p: &P, i: usize) -> El {
    let k = kit(p);
    let setup = &s.ed.list[i];
    let name_text = setup.name.text.trim().to_string();
    let back = IconButton::new(&BACK)
        .variant(IconButtonVariant::Ghost)
        .label("Back to setups")
        .help("Back to setups")
        .on_press(|s: &mut S| s.ed.close())
        .build(&k);
    let name = TextInput::new(&setup.name)
        .label("Name")
        .placeholder("Setup name")
        .width(None)
        .invalid(name_text.is_empty())
        .on_edit(move |s: &mut S| &mut s.ed.list[i].name)
        .build(&k);
    let key_text = setup.key.text.trim();
    let key_err = (!key_text.is_empty()).then(|| config::parse_chord(key_text).err()).flatten();
    let key = TextInput::new(&setup.key)
        .label("Shortcut")
        .placeholder("Shortcut")
        .width(Some(150.0))
        .invalid(key_err.is_some())
        .on_edit(move |s: &mut S| &mut s.ed.list[i].key)
        .build(&k);
    let apply_name = name_text.clone();
    let apply = Button::new("Apply")
        .variant(ButtonVariant::Outline)
        .disabled(name_text.is_empty())
        .on_press(move |s: &mut S| restore(s, apply_name.clone()))
        .build(&k);
    let delete = Button::new("Delete")
        .variant(ButtonVariant::Danger)
        .on_press(move |s: &mut S| {
            if i < s.ed.list.len() {
                let gone = s.ed.list.remove(i);
                s.ed.close();
                s.status = format!("Deleted \u{201c}{}\u{201d}; Save to keep the change. Its apps stay open.", gone.name.text.trim());
                s.status_ok = true;
            }
        })
        .build(&k);
    let bar = row()
        .key("setup-header-row")
        .gap(space::S2)
        .center_y()
        .child(back)
        .child(name.grow(1.0).shrink())
        .child(key)
        .child(apply)
        .child(delete);
    let mut card = col()
        .key("setup-header")
        .gap(4.0)
        .py(space::S2)
        .pl(space::S1)
        .pr(space::S2)
        .fill(p.t.card)
        .radius(rune::tok::radius::LG)
        .border(p.hair, p.t.card_border)
        .child(bar);
    if let Some(e) = key_err {
        card = card.child(small(&format!("Shortcut: {e}"), crate::bar::color("#f38ba8", Color::WHITE)).ml(40.0));
    }
    card
}

fn empty_ws(number: usize) -> SetupWorkspace {
    SetupWorkspace { number, monitor: None, layout: None, splits: Vec::new(), master_ratio: None, apps: Vec::new() }
}

fn layout_section(s: &S, p: &P, overlays: &mut Vec<El>) -> El {
    let k = kit(p);
    let d = defaults(s);
    // An empty monitor shows the layout the workspace has on its others.
    let sibling = s.ed.setup().and_then(|e| e.workspaces.iter().find(|w| w.number == s.ed.ws)).map(to_config);
    let shown = s.ed.shown().map(to_config).unwrap_or_else(|| SetupWorkspace { layout: sibling.as_ref().and_then(|w| w.layout), ..empty_ws(s.ed.ws) });
    let kind = setups::workspace_layout(&shown, d.layout);
    let current = LAYOUTS.iter().position(|l| *l == kind.name()).map(|i| LAYOUT_NAMES[i]);
    let layout_pick = Select::new("setup-tiling", &LAYOUT_NAMES)
        .label("Tiling")
        .width(130.0)
        .current(current)
        .open(s.ed.layout_open)
        .on_toggle(|s: &mut S| s.ed.layout_open = !s.ed.layout_open)
        .on_close(|s: &mut S| s.ed.layout_open = false)
        .on_choose(|s: &mut S, chosen: &str, _tx: &mut Tx| {
            s.ed.layout_open = false;
            let Some(i) = LAYOUT_NAMES.iter().position(|n| *n == chosen) else { return };
            if let Some(w) = s.ed.shown_or_add() {
                w.layout = LayoutKind::parse(LAYOUTS[i]);
            }
            s.ed.sync_siblings();
        })
        .build(&k, overlays);
    let toolbar = row()
        .key("setup-toolbar")
        .px(space::S3)
        .pt(space::S2)
        .gap(space::S2)
        .center_y()
        .child(small("Tiling", p.t.text_secondary))
        .child(layout_pick)
        .child(info(s, p, &d, kind, overlays))
        .child(el().grow(1.0))
        .child(add_picker(s, p, overlays));
    let canvas = row()
        .key("canvas-wrap")
        .px(space::S2)
        .py(space::S2)
        .gap(space::S1)
        .center_y()
        .child(add_monitor_button(p, 0, "Add a monitor on the left"))
        .child(carousel(s, p, &d, shown.layout).grow(1.0).shrink())
        .child(add_monitor_button(p, s.ed.count(), "Add a monitor on the right"));
    section(p, "setup-layout", "", vec![ws_cards(s, p, overlays), toolbar, monitor_bar(s, p), canvas, inspector(s, p)])
}

/// A "+" beside the carousel: lays the setup out for another monitor on that side.
fn add_monitor_button(p: &P, at: usize, label: &str) -> El {
    el().key(format!("add-monitor-{}", if at == 0 { "left" } else { "right" }))
        .size(30.0, 72.0)
        .radius(8.0)
        .border(p.hair, p.t.card_border)
        .hover(p.t.text.alpha(0.06))
        .center()
        .hit()
        .cursor(Cursor::Pointer)
        .label(label.to_string())
        .on_press(move |s: &mut S| s.ed.add_monitor_at(at))
        .child(icon(&PLUS, 14.0).color(p.t.text_secondary))
}

/// The info button by the tiling dropdown, and what it opens: how the editor works.
fn info(s: &S, p: &P, d: &Defaults, kind: LayoutKind, overlays: &mut Vec<El>) -> El {
    let k = kit(p);
    let trigger = IconButton::new(&INFO)
        .variant(IconButtonVariant::GhostMuted)
        .label("How the layout editor works")
        .on_press(|s: &mut S| s.ed.info_open = !s.ed.info_open)
        .build(&k);
    let order = match d.new_window {
        NewWindow::AfterFocused => "after the selected window",
        NewWindow::End => "at the end",
    };
    let mut lines = vec![
        "Drag a window onto another to swap them.".to_string(),
        "Drag the gap between two windows to resize them.".to_string(),
        format!("New apps go {order}, as lattice places new windows."),
        "Click a window to change its program, or move or remove it.".to_string(),
    ];
    if kind == LayoutKind::Monocle {
        lines[1] = "Monocle: every window fills the screen; they're fanned out here so each shows.".to_string();
    }
    let body = lines.into_iter().fold(col().gap(6.0).w(260.0), |c, l| c.child(small(&l, p.t.text_secondary).w_full().wrap()));
    Popover::new("setup-info")
        .trigger(trigger)
        .title("Layout editor")
        .open(s.ed.info_open)
        .placement(Placement::BottomStart)
        .on_close(|s: &mut S| s.ed.info_open = false)
        .child(body)
        .build(&k, overlays)
}

/// The monitor shown, and the arrows to slide to the others.
fn monitor_bar(s: &S, p: &P) -> El {
    let k = kit(p);
    let ed = &s.ed;
    let (n, m) = (ed.count(), ed.mon);
    let mut bar = row().key("setup-monitors").px(space::S3).pt(space::S2).gap(space::S2).center_y();
    if n > 1 {
        let arrow = |label: &str, to: usize, disabled: bool| {
            Button::new(label).variant(ButtonVariant::Ghost).disabled(disabled).on_press(move |s: &mut S| s.ed.show_mon(to)).build(&k)
        };
        bar = bar
            .child(arrow("\u{2039}", m.saturating_sub(1), m == 0))
            .child(text(format!("{} of {n}", m + 1)).font(type_::BODY).color(p.t.text_secondary).w(44.0).align(rune::Align::Center))
            .child(arrow("\u{203a}", (m + 1).min(n - 1), m + 1 >= n));
    }
    let name = text(ed.monitor_name(m)).font(TextStyle::new(13.0, 600.0)).color(p.t.text);
    bar.child(name)
}

/// Takes the monitor shown out of the setup (its delete button on the canvas).
pub(super) fn remove_monitor(s: &mut S) {
    let m = s.ed.mon;
    let gone = s.ed.remove_monitor();
    s.status = match gone {
        0 => format!("Removed monitor {}.", m + 1),
        1 => format!("Removed monitor {} and its app. Close without saving to undo.", m + 1),
        g => format!("Removed monitor {} and its {g} apps. Close without saving to undo.", m + 1),
    };
    s.status_ok = true;
}

/// The canvas of each monitor of the workspace, side by side: the one shown in the middle, the
/// others either side, smaller and dimmed, to click or slide to. With one monitor, its canvas.
fn carousel(s: &S, p: &P, d: &Defaults, layout: Option<LayoutKind>) -> El {
    let ed = &s.ed;
    let n = ed.count();
    let entry = |m: usize| {
        let mine = ed.setup().and_then(|e| e.entry(ed.ws, m)).map(to_config);
        // An empty monitor shows the layout the workspace has on its others.
        mine.unwrap_or_else(|| SetupWorkspace { layout, monitor: Some(m + 1), ..empty_ws(ed.ws) })
    };
    if n <= 1 {
        return canvas(s, p, &entry(0), d, 0, true);
    }
    let pitch = SLIDE_W + SLIDE_GAP;
    // As tall as the tallest monitor's canvas.
    let tall = (0..n).map(|m| ed.canvas_for(m).h).fold(0.0, f32::max);
    let mut strip = row()
        .key("monitor-strip")
        .gap(SLIDE_GAP)
        .center_y()
        .motion(SLIDE)
        // The strip is centred in the carousel; this puts the monitor shown in the middle.
        .offset(-(ed.mon as f32 - (n - 1) as f32 / 2.0) * pitch, 0.0);
    for m in 0..n {
        let live = m == ed.mon;
        let mut slide = col()
            .key(format!("slide-{m}"))
            .size(SLIDE_W, tall)
            .center()
            .motion(SLIDE)
            .scale(if live { 1.0 } else { 0.9 })
            .opacity(if live { 1.0 } else { 0.55 })
            .child(canvas(s, p, &entry(m), d, m, live));
        if !live {
            slide = slide.hit().cursor(Cursor::Pointer).label(format!("Show monitor {}", m + 1)).on_press(move |s: &mut S| s.ed.show_mon(m));
        }
        strip = strip.child(slide);
    }
    let mut dots = row().key("monitor-dots").gap(2.0).center_y();
    for m in 0..n {
        let on = m == ed.mon;
        dots = dots.child(
            el().key(format!("dot-{m}"))
                .px(4.0).py(4.0)
                .hit()
                .cursor(Cursor::Pointer)
                .label(format!("Monitor {}", m + 1))
                .on_press(move |s: &mut S| s.ed.show_mon(m))
                .child(el().size(if on { 18.0 } else { 6.0 }, 6.0).radius(3.0).motion(SLIDE).fill(if on { p.accent } else { p.t.text_secondary.alpha(0.4) })),
        );
    }
    col()
        .key("carousel")
        .w_full()
        .gap(space::S1)
        .items_center()
        .child(row().key("carousel-view").w_full().h(tall + 8.0).justify_center().center_y().clip_fade_linear(28.0, 0.0, 28.0, 0.0).child(strip))
        .child(dots)
}

/// A workspace drawn small in a `bw` × `bh` box: its monitors side by side as they're
/// arranged, with their tiles.
fn mini_thumb(th: &setups::Thumb, bw: f32, bh: f32, p: &P) -> El {
    let (w, h) = setups::fit(th.aspect, bw, bh);
    let (ox, oy) = ((bw - w) / 2.0, (bh - h) / 2.0);
    let mut out = el().size(bw, bh);
    for r in &th.screens {
        // A pixel between monitors.
        out = out.child(
            el().absolute(ox + r[0] * w, oy + r[1] * h).size((r[2] * w - 1.0).max(2.0), (r[3] * h).max(2.0)).radius(3.0).fill(p.t.bg).border(p.hair, p.t.card_border),
        );
    }
    for (i, r) in th.tiles.iter().enumerate() {
        // A pixel of gap around each tile, so neighbours read apart.
        out = out.child(
            el().absolute(ox + r[0] * w + 1.0, oy + r[1] * h + 1.0)
                .size((r[2] * w - 2.0).max(2.0), (r[3] * h - 2.0).max(2.0))
                .radius(1.5)
                .fill(color(i).alpha(0.75)),
        );
    }
    out
}

/// A card for each workspace this setup uses (click to show it), and one to add
/// another.
fn ws_cards(s: &S, p: &P, overlays: &mut Vec<El>) -> El {
    let k = kit(p);
    let d = defaults(s);
    let setup = s.ed.setup().expect("a setup is open");
    let max = (s.workspaces.round() as usize).max(1);
    let cards = s.ed.cards();
    let mut strip = row().key("ws-cards").px(space::S3).pt(space::S3).pb(space::S1).gap(space::S2).flex_wrap();
    for &n in &cards {
        let on = n == s.ed.ws;
        // What the setup puts on this workspace (every monitor), by name.
        let names: Vec<String> = setup
            .workspaces
            .iter()
            .filter(|w| w.number == n)
            .flat_map(|w| &w.apps)
            .map(|a| setups::app_label(&a.exe.text))
            .filter(|n| !n.is_empty())
            .collect();
        let contents = if names.is_empty() { "No apps yet".to_string() } else { names.join(", ") };
        strip = strip.child(
            row()
                .key(format!("ws-card-{n}"))
                .hit()
                .cursor(Cursor::Pointer)
                .label(format!("Workspace {n}: {contents}"))
                .on_press(move |s: &mut S| {
                    if s.ed.ws != n {
                        s.ed.show_ws(n);
                    }
                })
                .h(48.0)
                .px(10.0)
                .gap(10.0)
                .center_y()
                .radius(9.0)
                .fill(if on { p.accent.alpha(0.14) } else { p.t.bg })
                .hover(if on { p.accent.alpha(0.18) } else { p.t.text.alpha(0.05) })
                .border(if on { 1.5 } else { p.hair }, if on { p.accent } else { p.t.card_border })
                .child(mini_thumb(&ws_thumb(&s.ed, setup, n, &d), 52.0, 30.0, p))
                .child(
                    col()
                        .gap(1.0)
                        // Fixed, so three cards and "Add workspace" share a row; the names
                        // are all on the canvas below.
                        .w(90.0)
                        .child(text(format!("Workspace {n}")).font(TextStyle::new(12.5, 650.0)).color(if on { p.t.text } else { p.t.text.alpha(0.85) }).truncate())
                        .child(small(&contents, p.t.text_secondary).truncate()),
                ),
        );
    }
    let free: Vec<usize> = (1..=max).filter(|n| !cards.contains(n)).collect();
    if !free.is_empty() {
        let options: Vec<ComboboxOption> = free.iter().map(|n| ComboboxOption::new(n.to_string(), format!("Workspace {n}"))).collect();
        let trigger = row()
            .h(48.0)
            .px(12.0)
            .gap(6.0)
            .center_y()
            .radius(9.0)
            .border(p.hair, p.t.card_border)
            .hover(p.t.text.alpha(0.05))
            .cursor(Cursor::Pointer)
            .child(text("+").font(TextStyle::new(16.0, 500.0)).color(p.t.text_secondary))
            .child(text("Add workspace").font(TextStyle::new(12.5, 600.0)).color(p.t.text_secondary));
        strip = strip.child(
            Combobox::new("setup-add-ws", &s.ed.ws_picker, &options, |s: &mut S| &mut s.ed.ws_picker)
                .label("Add workspace")
                .search_placeholder("Workspace number…")
                .width(200.0)
                .trigger(trigger)
                .on_choose(|s: &mut S, chosen: &str, _tx: &mut Tx| {
                    s.ed.ws_picker.close();
                    if let Ok(n) = chosen.parse() {
                        s.ed.show_ws(n);
                    }
                })
                .build(&k, overlays),
        );
    }
    strip
}

/// A caption over a control.
fn field(label: &str, control: El, p: &P) -> El {
    col().gap(4.0).child(small(label, p.t.text_secondary)).child(control)
}

/// The selected window's program, start command and workspace, under the canvas. Always the
/// same height, so selecting a window doesn't move anything.
fn inspector(s: &S, p: &P) -> El {
    let strip = row()
        .key("inspector")
        .h(66.0)
        .mx(space::S1)
        .mb(space::S1)
        .px(space::S3)
        .gap(space::S3)
        .center_y()
        .radius(8.0)
        .fill(p.t.bg);
    let selected = s.ed.tile.filter(|t| s.ed.shown().is_some_and(|w| *t < w.apps.len()));
    let Some(t) = selected else {
        let msg = if s.ed.shown().is_some_and(|w| !w.apps.is_empty()) { "No window selected." } else { "Nothing here yet." };
        return strip.child(small(msg, p.t.text_secondary).grow(1.0).shrink().wrap());
    };
    let k = kit(p);
    let ws = s.ed.ws;
    let workspaces = (s.workspaces.round() as usize).max(1);
    let si = s.ed.sel.expect("open");
    let mon = s.ed.mon;
    let wi = s.ed.list[si].entry_index(ws, mon).expect("shown workspace");
    let app = &s.ed.list[si].workspaces[wi].apps[t];
    let exe_text = app.exe.text.trim();
    let initial = setups::app_label(exe_text).chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "?".into());
    let badge = el()
        .size(32.0, 32.0)
        .radius(9.0)
        .fill(color(t))
        .center()
        .child(text(initial).font(TextStyle::new(15.0, 750.0)).color(crate::bar::color("#11111b", Color::BLACK)));
    let exe = TextInput::new(&app.exe)
        .label("Program")
        .placeholder("Code.exe")
        .width(None)
        .invalid(exe_text.is_empty() || crate::settings::bad_exe(&app.exe.text))
        .on_edit(move |s: &mut S| &mut s.ed.list[si].workspaces[wi].apps[t].exe)
        .build(&k);
    let step = |label: &str, to: usize, disabled: bool| {
        Button::new(label).variant(ButtonVariant::Ghost).disabled(disabled).on_press(move |s: &mut S| s.ed.move_to(t, to)).build(&k)
    };
    let mover = row()
        .gap(0.0)
        .center_y()
        .child(step("\u{2039}", ws.saturating_sub(1).max(1), ws <= 1))
        .child(text(ws.to_string()).w(18.0).align(rune::Align::Center).font(type_::BODY).color(p.t.text))
        .child(step("\u{203a}", (ws + 1).min(workspaces), ws >= workspaces));
    let remove = Button::new("Remove").variant(ButtonVariant::GhostMuted).on_press(move |s: &mut S| s.ed.remove(t)).build(&k);
    let mut strip = strip
        .child(badge)
        .child(field("Program", exe, p).grow(1.0).shrink())
        .child(field("Workspace", mover, p));
    let count = s.ed.count();
    if count > 1 {
        let mstep = |label: &str, to: usize, disabled: bool| {
            Button::new(label).variant(ButtonVariant::Ghost).disabled(disabled).on_press(move |s: &mut S| s.ed.move_to_monitor(t, to)).build(&k)
        };
        let mon_mover = row()
            .gap(0.0)
            .center_y()
            .child(mstep("\u{2039}", mon.saturating_sub(1), mon == 0))
            .child(text((mon + 1).to_string()).w(18.0).align(rune::Align::Center).font(type_::BODY).color(p.t.text))
            .child(mstep("\u{203a}", (mon + 1).min(count - 1), mon + 1 >= count));
        strip = strip.child(field("Monitor", mon_mover, p));
    }
    strip.child(remove)
}

fn add_picker(s: &S, p: &P, overlays: &mut Vec<El>) -> El {
    let k = kit(p);
    let mut options = Vec::new();
    let programs = if s.ed.picker.is_open() { catalog::programs() } else { None };
    for (i, prog) in programs.unwrap_or_default().iter().enumerate() {
        let mut o = ComboboxOption::new(i.to_string(), prog.name.clone());
        o.detail = prog.exe.clone();
        options.push(o);
    }
    if s.ed.picker.is_open() && programs.is_none() {
        let mut o = ComboboxOption::new("__loading__", "Finding programs\u{2026}");
        o.disabled = true;
        options.push(o);
    }
    options.push(ComboboxOption::new(OTHER, "Other program\u{2026}"));
    Combobox::new("setup-add-app", &s.ed.picker, &options, |s: &mut S| &mut s.ed.picker)
        .label("Add an app")
        .placeholder("+ Add app")
        .search_placeholder("Search programs\u{2026}")
        .width(240.0)
        .max_height(320.0)
        .on_choose(|s: &mut S, chosen: &str, _tx: &mut Tx| {
            s.ed.picker.close();
            let nw = defaults(s).new_window;
            if chosen == OTHER {
                s.ed.add("", "", nw);
            } else if let Some(prog) = chosen.parse::<usize>().ok().and_then(|i| catalog::programs()?.get(i)) {
                s.ed.add(&prog.exe, &prog.run, nw);
            }
        })
        .build(&k, overlays)
}
