//! App icons on macOS: not written yet (NSWorkspace). The bar shows app names instead.

use crate::icon::AppIcon;
use crate::platform::WindowId;

pub fn for_window(_: WindowId) -> Option<AppIcon> {
    None
}

pub fn for_exe(_: &str) -> Option<AppIcon> {
    None
}
