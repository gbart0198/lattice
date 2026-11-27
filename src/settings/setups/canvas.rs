//! The layout canvas: a monitor of a setup drawn the way lattice will tile it. Its size and
//! geometry (screen pixels to canvas points), the tiles and the seams between them, and dragging
//! either: a tile onto another to swap them, a seam to resize.

use rune::{Color, Cursor, Drag, El, Phase, TextStyle, Tx, col, el, row, text};

use super::model::{Editor, to_config};
use super::view::small;
use crate::config::{NewWindow, SetupWorkspace};
use crate::layout::{self, LayoutKind, Rect};
use crate::settings::{LAYOUTS, NEW_WINDOW, P, S};
use crate::setups::{self, Defaults};

/// The canvas's width in logical pixels; its height follows the screen's shape.
const CANVAS_W: f32 = 640.0;
/// The tallest it gets (a portrait monitor is fitted into this).
const CANVAS_MAX_H: f32 = 400.0;
/// With more than one monitor, the room for each in the carousel: smaller, so the ones either
/// side peek in.
pub(super) const SLIDE_W: f32 = 460.0;
const SLIDE_H: f32 = 300.0;
pub(super) const SLIDE_GAP: f32 = 18.0;
/// How wide the grab area of a seam is.
const SEAM: f32 = 14.0;
/// Pointer travel before a press on a window counts as a drag.
const DRAG_SLOP: f32 = 4.0;

/// Tile colours, by position (Catppuccin Mocha accents, like the bar).
const PALETTE: [&str; 8] = ["#89b4fa", "#cba6f7", "#a6e3a1", "#fab387", "#94e2d5", "#f5c2e7", "#f9e2af", "#89dceb"];

/// A window being dragged on the canvas.
#[derive(Clone, Copy, Debug)]
pub(super) struct TileDrag {
    pub(super) from: usize,
    /// The tile's top left on the canvas when the press landed, and where in it.
    pub(super) origin: (f32, f32),
    pub(super) grab: (f32, f32),
    /// The pointer, on the canvas.
    pub(super) pointer: (f32, f32),
    pub(super) moved: bool,
}

impl Editor {
    /// The desktop areas a setup's `count` monitors are drawn with.
    pub(super) fn areas(&self, count: usize) -> Vec<Rect> {
        let connected: Vec<Rect> = self.screens.iter().map(|s| s.area).collect();
        setups::monitor_areas(count, &connected, self.primary)
    }

    /// The tiling area monitor `m` of the open setup is drawn with.
    pub(super) fn area(&self, m: usize) -> Rect {
        let a = self.areas(self.count());
        a.get(m).copied().unwrap_or(a[0])
    }

    /// The canvas for monitor `m`: big with one monitor, a carousel slide with more.
    pub(super) fn canvas_for(&self, m: usize) -> Canvas {
        let a = self.area(m);
        let (w, h) = if self.count() > 1 { (SLIDE_W, SLIDE_H) } else { (CANVAS_W, CANVAS_MAX_H) };
        Canvas::new((a.w, a.h), w, h)
    }

    /// The canvas for the monitor shown.
    pub(super) fn canvas(&self) -> Canvas {
        self.canvas_for(self.mon)
    }
}

/// The tiling settings the canvas uses: the General tab's, as they are now.
pub(super) fn defaults(s: &S) -> Defaults {
    Defaults {
        layout: LayoutKind::parse(LAYOUTS[s.layout]).unwrap_or(LayoutKind::Dwindle),
        split_ratio: s.split_ratio,
        master_ratio: s.master_ratio,
        gaps_inner: s.gaps_inner.round() as i32,
        gaps_outer: s.gaps_outer.round() as i32,
        new_window: if NEW_WINDOW[s.new_window] == "end" { NewWindow::End } else { NewWindow::AfterFocused },
    }
}

/// The canvas: its size, and pixels on screen to points on it.
#[derive(Clone, Copy, Debug)]
pub(super) struct Canvas {
    w: f32,
    pub(super) h: f32,
    scale: f32,
    screen: Rect,
}

impl Canvas {
    /// A screen of this size fitted into `max_w` × `max_h` (a portrait monitor is narrower).
    fn new(screen: (i32, i32), max_w: f32, max_h: f32) -> Self {
        let scale = (max_w / screen.0 as f32).min(max_h / screen.1 as f32);
        Canvas {
            w: (screen.0 as f32 * scale).round(),
            h: (screen.1 as f32 * scale).round(),
            scale,
            screen: Rect::new(0, 0, screen.0, screen.1),
        }
    }
}

/// What the canvas shows now: the entry on it as config, the tiling settings it's drawn with,
/// and the canvas of the monitor shown. None if the setup has no entry there.
pub(super) fn on_canvas(s: &S) -> Option<(SetupWorkspace, Defaults, Canvas)> {
    let w = s.ed.shown().map(to_config)?;
    Some((w, defaults(s), s.ed.canvas()))
}

/// Where each app's tile is drawn on the canvas, as (x, y, w, h). Monocle stacks every window on
/// the whole screen; here they fan out a little so each one shows.
pub(super) fn tile_rects(w: &SetupWorkspace, d: &Defaults, c: &Canvas) -> Vec<(f32, f32, f32, f32)> {
    let rects = setups::tiles(w, c.screen, d);
    let kind = setups::workspace_layout(w, d.layout);
    let n = rects.len();
    rects
        .into_iter()
        .enumerate()
        .map(|(i, r)| {
            let (x, y, rw, rh) = (r.x as f32 * c.scale, r.y as f32 * c.scale, r.w as f32 * c.scale, r.h as f32 * c.scale);
            if kind == LayoutKind::Monocle && n > 1 {
                let step = 18.0;
                let spread = step * (n - 1) as f32;
                (x + step * i as f32, y + step * i as f32, rw - spread, rh - spread)
            } else {
                (x, y, rw, rh)
            }
        })
        .collect()
}

fn tile_under(rects: &[(f32, f32, f32, f32)], p: (f32, f32), skip: usize) -> Option<usize> {
    rects
        .iter()
        .enumerate()
        .rev()
        .find(|(i, r)| *i != skip && p.0 >= r.0 && p.0 < r.0 + r.2 && p.1 >= r.1 && p.1 < r.1 + r.3)
        .map(|(i, _)| i)
}

fn seams(w: &SetupWorkspace, d: &Defaults, c: &Canvas) -> Vec<layout::Seam> {
    let kind = setups::workspace_layout(w, d.layout);
    layout::seams(kind, c.screen, w.apps.len(), &setups::params(w, d, 1.0))
}

pub(super) fn color(i: usize) -> Color {
    crate::bar::color(PALETTE[i % PALETTE.len()], Color::WHITE)
}

/// The screen, its tiles and the seams between them.
/// Monitor `m`'s canvas with workspace entry `w`. Only the `live` one (the monitor shown) takes
/// drags and shows the selection; the others are a picture of it.
pub(super) fn canvas(s: &S, p: &P, w: &SetupWorkspace, d: &Defaults, m: usize, live: bool) -> El {
    let c = s.ed.canvas_for(m);
    let rects = tile_rects(w, d, &c);
    let drag = s.ed.drag.filter(|dr| live && dr.moved && dr.from < rects.len());
    let target = drag.and_then(|dr| tile_under(&rects, dr.pointer, dr.from));
    let ids: Vec<u64> = s.ed.setup().and_then(|e| e.entry(w.number, m)).map_or_else(Vec::new, |x| x.apps.iter().map(|a| a.id).collect());
    let mut screen = el()
        .key(format!("canvas-{}-{m}", s.ed.ws))
        .size(c.w, c.h)
        .radius(12.0)
        .fill(crate::bar::color("#11111b", Color::BLACK))
        .border(p.hair * 2.0, p.t.card_border)
        .clip_rounded(12.0);
    if w.apps.is_empty() {
        screen = screen.child(
            col()
                .absolute(0.0, 0.0)
                .size(c.w, c.h)
                .center()
                .gap(6.0)
                .child(
                    text(if s.ed.count() > 1 { format!("Nothing on monitor {} of workspace {}", m + 1, w.number) } else { format!("Workspace {} is empty", w.number) })
                        .font(TextStyle::new(15.0, 600.0))
                        .color(p.t.text_secondary),
                )
                .child(small("Use \u{201c}+ Add app\u{201d} to place one here", p.t.text_secondary.alpha(0.7))),
        );
    }
    let names: Vec<String> = w.apps.iter().map(|a| a.exe.clone()).collect();
    for (i, r) in rects.iter().enumerate() {
        let dragged = drag.is_some_and(|dr| dr.from == i);
        let state = if dragged {
            TileLook::Placeholder
        } else if target == Some(i) {
            TileLook::Target
        } else if live && s.ed.tile == Some(i) {
            TileLook::Selected
        } else {
            TileLook::Normal
        };
        let origin = (r.0, r.1);
        let mut tile = tile_el(&names[i], i, r.2, r.3, state, p)
            .key(format!("tile-{}", ids.get(i).map_or(format!("{m}-{i}"), |id| id.to_string())))
            .absolute(r.0, r.1);
        if live {
            tile = tile
                .hit()
                .cursor(if dragged { Cursor::Grabbing } else { Cursor::Grab })
                .label(setups::app_label(&names[i]))
                .on_drag(move |s: &mut S, d: &mut Drag, _tx: &mut Tx| tile_drag(s, i, origin, d));
        }
        screen = screen.child(tile);
    }
    // A monitor that isn't connected: where its apps go instead.
    if !s.ed.connected(m) {
        screen = screen.child(
            row()
                .absolute(10.0, 10.0)
                .h(22.0)
                .px(8.0)
                .radius(6.0)
                .center()
                .fill(crate::bar::color("#f9e2af", Color::WHITE).alpha(0.9))
                .child(text("Not connected: restores onto your primary monitor").font(TextStyle::new(11.0, 650.0)).color(Color::BLACK)),
        );
    }
    // Seams: drawn over the tiles, grabbed anywhere along their length.
    if live && drag.is_none() {
        for (k, seam) in seams(w, d, &c).into_iter().enumerate() {
            let pos = seam.pos() * c.scale;
            let reg = seam.region;
            let (x, y, sw, sh) = if seam.vertical {
                (pos - SEAM / 2.0, reg.y as f32 * c.scale, SEAM, reg.h as f32 * c.scale)
            } else {
                (reg.x as f32 * c.scale, pos - SEAM / 2.0, reg.w as f32 * c.scale, SEAM)
            };
            let active = s.ed.seam == Some(k);
            let grip_len = 34.0_f32.min(if seam.vertical { sh } else { sw } * 0.5);
            let grip = if seam.vertical { el().size(4.0, grip_len) } else { el().size(grip_len, 4.0) }
                .radius(2.0)
                .fill(if active { p.accent } else { Color::WHITE.alpha(0.55) });
            let mut handle = el()
                .key(format!("seam-{k}"))
                .absolute(x, y)
                .size(sw, sh)
                .hit()
                .center()
                .cursor(if seam.vertical { Cursor::ResizeEw } else { Cursor::ResizeNs })
                .label("Resize")
                .on_drag(move |s: &mut S, d: &mut Drag, _tx: &mut Tx| seam_drag(s, k, (x, y), d))
                .child(grip);
            if active {
                handle = handle.fill(p.accent.alpha(0.18)).radius(4.0);
            }
            screen = screen.child(handle);
            if active {
                let pct = format!("{:.0}%", seam.ratio * 100.0);
                let (lx, ly) = if seam.vertical { (pos + 8.0, y + sh / 2.0 - 10.0) } else { (x + sw / 2.0 - 18.0, pos + 8.0) };
                screen = screen.child(
                    row()
                        .absolute(lx, ly)
                        .h(20.0)
                        .px(6.0)
                        .radius(5.0)
                        .center()
                        .fill(p.accent)
                        .child(text(pct).font(TextStyle::new(11.0, 700.0)).color(Color::BLACK)),
                );
            }
        }
    }
    // The window being dragged follows the pointer, over everything.
    if let Some(dr) = drag {
        let r = rects[dr.from];
        let (gx, gy) = (dr.pointer.0 - dr.grab.0, dr.pointer.1 - dr.grab.1);
        screen = screen.child(tile_el(&names[dr.from], dr.from, r.2, r.3, TileLook::Ghost, p).absolute(gx, gy).shadow(10.0, 28.0, Color::BLACK.alpha(0.5)));
    }
    screen
}

#[derive(Clone, Copy, PartialEq)]
enum TileLook {
    Normal,
    Selected,
    /// Where the dragged window will land.
    Target,
    /// The dragged window's empty spot.
    Placeholder,
    /// The dragged window itself.
    Ghost,
}

fn tile_el(exe: &str, i: usize, w: f32, h: f32, look: TileLook, p: &P) -> El {
    let c = color(i);
    let name = setups::app_label(exe);
    let pad = 4.0;
    let (fill, border, bw) = match look {
        TileLook::Normal => (c.alpha(0.16), c.alpha(0.5), 1.0),
        TileLook::Selected => (c.alpha(0.28), c, 2.0),
        TileLook::Target => (p.accent.alpha(0.3), p.accent, 2.5),
        TileLook::Placeholder => (Color::WHITE.alpha(0.03), c.alpha(0.35), 1.0),
        // Opaque, so the window under it doesn't show through.
        TileLook::Ghost => (crate::bar::color("#1e1e2e", Color::BLACK).mix(c, 0.4), c, 2.0),
    };
    let mut inner = col().w_full().h_full().center().gap(4.0).px(6.0);
    if look != TileLook::Placeholder {
        let initial = name.chars().next().map(|ch| ch.to_uppercase().to_string()).unwrap_or_default();
        if h > 64.0 && w > 50.0 {
            inner = inner.child(
                el().size(30.0, 30.0).radius(9.0).fill(c).center().child(text(initial).font(TextStyle::new(15.0, 750.0)).color(crate::bar::color("#11111b", Color::BLACK))),
            );
        }
        let title = if name.is_empty() || exe.trim().is_empty() { "New app".to_string() } else { name };
        inner = inner.child(text(title).font(TextStyle::new(if w > 120.0 { 14.0 } else { 12.0 }, 650.0)).color(Color::WHITE.alpha(0.95)).truncate());
        if look == TileLook::Target {
            inner = inner.child(text("Swap").font(TextStyle::new(11.0, 700.0)).color(p.accent));
        } else if h > 90.0 && !exe.trim().is_empty() {
            inner = inner.child(text(exe.trim().to_string()).font(TextStyle::new(11.0, 450.0)).color(Color::WHITE.alpha(0.55)).truncate());
        }
    }
    el().size((w - 2.0 * pad).max(4.0), (h - 2.0 * pad).max(4.0))
        .ml(pad)
        .mt(pad)
        .radius(9.0)
        .fill(fill)
        .border(bw, border)
        .child(inner)
}

fn tile_drag(s: &mut S, i: usize, origin: (f32, f32), d: &mut Drag) {
    match d.phase {
        Phase::Down => {
            s.ed.tile = Some(i);
            s.ed.drag = Some(TileDrag {
                from: i,
                origin,
                grab: (d.start_x, d.start_y),
                pointer: (origin.0 + d.start_x, origin.1 + d.start_y),
                moved: false,
            });
        }
        Phase::Move => {
            if let Some(dr) = s.ed.drag.as_mut() {
                dr.pointer = (dr.origin.0 + dr.grab.0 + d.dx, dr.origin.1 + dr.grab.1 + d.dy);
                dr.moved |= d.dx.abs() + d.dy.abs() > DRAG_SLOP;
            }
        }
        Phase::Up => {
            let Some(dr) = s.ed.drag.take() else { return };
            if d.cancelled() || !dr.moved {
                return;
            }
            let Some((w, dflt, c)) = on_canvas(s) else { return };
            let rects = tile_rects(&w, &dflt, &c);
            if let Some(t) = tile_under(&rects, dr.pointer, dr.from)
                && let Some(w) = s.ed.shown_mut()
                && t < w.apps.len()
                && dr.from < w.apps.len()
            {
                w.apps.swap(dr.from, t);
                s.ed.tile = Some(t);
            }
        }
    }
}

fn seam_drag(s: &mut S, k: usize, handle: (f32, f32), d: &mut Drag) {
    let Some((w, dflt, c)) = on_canvas(s) else { return };
    let Some(seam) = seams(&w, &dflt, &c).into_iter().nth(k) else { return };
    match d.phase {
        Phase::Down => {
            s.ed.seam = Some(k);
            s.ed.seam_undo = (w.splits.clone(), w.master_ratio);
            // The handle moves with the seam: keep where it was when the press landed.
            d.data = if seam.vertical { handle.0 } else { handle.1 };
            return;
        }
        Phase::Up => {
            s.ed.seam = None;
            if d.cancelled() {
                let (splits, master) = std::mem::take(&mut s.ed.seam_undo);
                if let Some(w) = s.ed.shown_mut() {
                    (w.splits, w.master_ratio) = (splits, master);
                }
                s.ed.sync_siblings();
                return;
            }
        }
        Phase::Move => {}
    }
    let along = if seam.vertical { d.data + d.start_x + d.dx } else { d.data + d.start_y + d.dy };
    let ratio = seam.ratio_at(along / c.scale);
    if let Some(w) = s.ed.shown_mut() {
        setups::set_ratio(&mut w.splits, &mut w.master_ratio, seam.target, ratio, dflt.split_ratio);
    }
    // The master column's share is the workspace's, on every monitor.
    if seam.target == layout::ResizeTarget::Master {
        s.ed.sync_siblings();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_under_the_pointer() {
        let rects = [(0.0, 0.0, 100.0, 100.0), (100.0, 0.0, 100.0, 100.0)];
        assert_eq!(tile_under(&rects, (150.0, 50.0), 0), Some(1));
        assert_eq!(tile_under(&rects, (50.0, 50.0), 0), None, "not onto itself");
        assert_eq!(tile_under(&rects, (250.0, 50.0), 0), None);
    }

}
