//! Windows and monitors on macOS: not written yet (Accessibility API and CoreGraphics). Every
//! query answers "nothing there" and every action does nothing, so the WM sees an empty desktop.

use super::unsupported;
use crate::layout::Rect;
pub use crate::platform::{Candidate, MonitorInfo, WindowId};

fn windows() {
    unsupported("window management");
}

pub fn own_pid() -> u32 {
    std::process::id()
}

pub fn is_window(_: WindowId) -> bool {
    false
}

pub fn is_visible(_: WindowId) -> bool {
    false
}

pub fn is_minimized(_: WindowId) -> bool {
    false
}

pub fn is_maximized(_: WindowId) -> bool {
    false
}

pub fn is_cloaked(_: WindowId) -> bool {
    false
}

pub fn title(_: WindowId) -> String {
    String::new()
}

pub fn class(_: WindowId) -> String {
    String::new()
}

pub fn pid(_: WindowId) -> u32 {
    0
}

pub fn exe_name(_: WindowId) -> String {
    String::new()
}

pub fn exe_path(_: WindowId) -> String {
    String::new()
}

pub fn owner(_: WindowId) -> Option<WindowId> {
    None
}

pub fn is_top_level(_: WindowId) -> bool {
    false
}

pub fn candidate(_: WindowId) -> Option<Candidate> {
    None
}

pub fn enum_windows() -> Vec<WindowId> {
    windows();
    Vec::new()
}

pub fn window_rect(_: WindowId) -> Option<Rect> {
    None
}

pub fn frame_rect(_: WindowId) -> Option<Rect> {
    None
}

pub fn place(_: WindowId, _: Rect) {
    windows();
}

pub fn place_quiet(_: WindowId, _: Rect) {
    windows();
}

/// A window made invisible for an animation. Nothing is ever faded on macOS yet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Faded {}

impl Faded {
    pub fn encode(&self) -> String {
        "-".into()
    }

    pub fn decode<'a>(_: impl Iterator<Item = &'a str>) -> Option<Faded> {
        None
    }
}

pub fn fade(_: WindowId) -> Option<Faded> {
    None
}

pub fn unfade(_: WindowId, _: Faded) {}

pub fn raise(_: WindowId) {
    windows();
}

pub fn repaint(_: WindowId) {}

pub fn find_own_window(_: &str) -> WindowId {
    0
}

pub fn place_overlay(_: WindowId, _: Rect) {}

pub fn hide(_: WindowId) {
    windows();
}

pub fn show(_: WindowId) {
    windows();
}

pub fn close(_: WindowId) {
    windows();
}

pub fn foreground() -> WindowId {
    0
}

pub fn is_fullscreen_on(_: WindowId, _: Rect) -> bool {
    false
}

pub fn focus(_: WindowId) {
    windows();
}

pub fn focus_desktop() {}

pub fn cursor_pos() -> (i32, i32) {
    (0, 0)
}

pub fn set_cursor(_: i32, _: i32) {}

pub fn monitors() -> Vec<MonitorInfo> {
    unsupported("monitors");
    Vec::new()
}

pub fn monitor_of(_: WindowId) -> isize {
    0
}

pub fn monitor_at(_: (i32, i32)) -> isize {
    0
}
