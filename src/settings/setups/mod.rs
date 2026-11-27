//! The Setups tab: setups (`[[setup]]`) and their layout editor.
//!
//! The tab lists them; clicking one opens it on a page of its own (back with "‹ Setups"). That page has its name and shortcut, a card for each workspace it uses, the
//! layout canvas, and under the canvas a strip for the selected window. The strip is always
//! there, so clicking a window changes what's in it rather than growing the page.
//!
//! The editor draws a setup the way lattice will tile it, from the same `layout`
//! code: drag a window onto another to swap them, drag the seam between windows to resize, and
//! an app you add drops in where a new window would (after the selected one, or at the end,
//! per `general.new_window`). Click a window to edit its program, or move or remove it.
//!
//! A setup is laid out for a number of monitors (at least one): the "+" either side of the
//! carousel adds one, the bin on the monitor shown removes it, and the carousel slides between
//! them. A workspace spans every monitor and each tiles its own
//! windows, so each monitor of a workspace is a `[[setup.workspace]]` entry.
//!
//! `model` holds the editor's state and operations, `canvas` the canvas's geometry, tiles,
//! seams and drags, `view` the pages around it, `catalog` the programs the add picker offers
//! (the launcher's app catalog), `io` saving, capturing and restoring, and `samples` the data
//! behind the headless `--scene setups*` previews.

mod canvas;
mod catalog;
mod io;
mod model;
mod samples;
mod view;

pub(super) use io::{poll, set_waker, validate, write};
pub(super) use model::Editor;
pub(super) use samples::{
    sample, sample_add, sample_add_ws, sample_drag, sample_info, sample_list, sample_master, sample_missing, sample_monitor, sample_none, sample_one,
    sample_resize, sample_tiling,
};
pub(super) use view::tab;
