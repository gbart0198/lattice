//! The focus border on macOS: not written yet (borderless, click-through NSWindows). No border.

use crate::config;
use crate::layout::Rect;

pub struct Border;

impl Border {
    pub fn new(cfg: config::Border) -> Option<Self> {
        if cfg.enabled {
            super::unsupported("the focus border");
        }
        None
    }

    pub fn set(&mut self, _target: Option<(Rect, f32)>) {}
}

pub fn preview(_cfg: config::Border) {
    super::unsupported("--border-preview");
}
