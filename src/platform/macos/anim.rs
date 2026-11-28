//! The animation back end on macOS: not written yet (it needs live window previews, which on
//! macOS mean ScreenCaptureKit). Nothing can be previewed, so every window moves instantly:
//! `Animator::glide` declines and the WM places it directly.

use crate::anim::{Backend, Thumb};
use crate::layout::Rect;
use crate::platform::{WindowId, win::{self, Faded}};

#[derive(Default)]
pub struct Native {}

impl Backend for Native {
    fn fade(&mut self, _: WindowId) -> Option<Faded> {
        None
    }

    fn unfade(&mut self, _: WindowId, _: Faded) {}

    fn preview(&mut self, _: WindowId) -> Option<Thumb> {
        None
    }

    fn show(&mut self, _: Thumb, _: Rect, _: Option<Rect>) {}

    fn drop_preview(&mut self, _: Thumb) {}

    fn idle(&mut self) {}

    fn is_visible(&self, h: WindowId) -> bool {
        win::is_visible(h)
    }

    fn place(&mut self, h: WindowId, r: Rect) {
        win::place(h, r);
    }

    fn arrived(&self, h: WindowId, r: Rect) -> bool {
        win::frame_rect(h) == Some(r)
    }
}

/// No compositor clock to wait on: the caller sleeps a frame instead.
pub fn wait_frame() -> bool {
    false
}

pub mod preview {
    pub fn run(_args: &[String]) {
        super::super::unsupported("--animation-preview");
    }
}
