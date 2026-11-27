//! Sample data for the headless previews of the Setups tab (`--settings --snap out.png --scene
//! setups`, and the other `setups-*` scenes in `settings/mod.rs`): made-up setups on two
//! monitors, and the states each scene shows them in.

use std::sync::atomic::Ordering;

use super::canvas::{TileDrag, on_canvas, tile_rects};
use super::model::{Editor, Screen};
use crate::config::{self, NewWindow};
use crate::layout::Rect;
use crate::settings::S;

/// Previews list made-up programs in the add picker, never what's really installed or running.
pub(super) static SAMPLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn sample(s: &mut S) {
    SAMPLE.store(true, Ordering::Relaxed);
    let cfg: config::Config = toml::from_str(
        r#"
        [[setup]]
        name = "Dev"
        key = "super+alt+1"
        [[setup.workspace]]
        number = 1
        splits = [0.62]
        apps = [{ exe = "Code.exe", run = "code" }, { exe = "WindowsTerminal.exe", run = "wt" }, { exe = "Postman.exe" }]
        [[setup.workspace]]
        number = 1
        monitor = 2
        apps = [{ exe = "msedge.exe" }, { exe = "Notion.exe" }]
        [[setup.workspace]]
        number = 2
        apps = [{ exe = "slack.exe" }]
        [[setup.workspace]]
        number = 3
        apps = [{ exe = "Spotify.exe" }]

        [[setup]]
        name = "Deep work"
        key = "super+alt+2"
        [[setup.workspace]]
        number = 1
        layout = "master"
        master_ratio = 0.6
        apps = [{ exe = "Obsidian.exe" }, { exe = "Spotify.exe" }, { exe = "Todoist.exe" }]

        [[setup]]
        name = "Writing"
        [[setup.workspace]]
        number = 4
        layout = "monocle"
        apps = [{ exe = "WINWORD.EXE" }, { exe = "msedge.exe" }]
        "#,
    )
    .expect("sample setups parse");
    // Two monitors side by side, the left one primary (with the bar).
    let screens = vec![Screen { area: Rect::new(0, 30, 1920, 1010) }, Screen { area: Rect::new(1920, 0, 1920, 1040) }];
    let mut ed = Editor::load_on(&cfg, screens, 0);
    ed.select(0);
    ed.tile = Some(0);
    s.ed = ed;
}

/// Monitor 2 of workspace 1.
pub(crate) fn sample_monitor(s: &mut S) {
    s.ed.show_mon(1);
    s.ed.tile = Some(0);
}

/// A third monitor, which isn't connected, with an app on it.
pub(crate) fn sample_missing(s: &mut S) {
    s.ed.add_monitor();
    s.ed.add("Spotify.exe", "", NewWindow::End);
    s.ed.tile = None;
}

/// A setup on one monitor.
pub(crate) fn sample_one(s: &mut S) {
    s.ed.select(1);
    s.ed.tile = Some(0);
}

/// The list, before one is opened.
pub(crate) fn sample_list(s: &mut S) {
    s.ed.close();
}

/// Nothing selected on the canvas.
pub(crate) fn sample_none(s: &mut S) {
    s.ed.tile = None;
}

/// The "Add workspace" menu open.
pub(crate) fn sample_add_ws(s: &mut S) {
    s.ed.tile = None;
    s.ed.ws_picker.step(rune_kit::ComboboxEvent::Toggle { current: None });
}

/// Terminal being dragged onto Code.
pub(crate) fn sample_drag(s: &mut S) {
    let (w, d, c) = on_canvas(s).expect("sample workspace");
    let rects = tile_rects(&w, &d, &c);
    let (from, to) = (rects[1], rects[0]);
    let grab = (from.2 * 0.5, from.3 * 0.4);
    s.ed.tile = Some(1);
    s.ed.drag = Some(TileDrag {
        from: 1,
        origin: (from.0, from.1),
        grab,
        pointer: (to.0 + to.2 * 0.55, to.1 + to.3 * 0.45),
        moved: true,
    });
}

/// The seam between Code and the rest, mid-drag.
pub(crate) fn sample_resize(s: &mut S) {
    if let Some(w) = s.ed.shown_mut() {
        w.splits = vec![0.55, 0.6];
    }
    s.ed.seam = Some(0);
    s.ed.tile = None;
}

pub(crate) fn sample_master(s: &mut S) {
    s.ed.select(1);
    s.ed.tile = Some(0);
}

/// The add picker open.
pub(crate) fn sample_add(s: &mut S) {
    s.ed.tile = Some(1);
    s.ed.picker.step(rune_kit::ComboboxEvent::Toggle { current: None });
}

/// The layout editor's help open.
pub(crate) fn sample_info(s: &mut S) {
    s.ed.tile = None;
    s.ed.info_open = true;
}

/// The tiling dropdown open.
pub(crate) fn sample_tiling(s: &mut S) {
    s.ed.layout_open = true;
}
