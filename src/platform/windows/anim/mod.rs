//! The animation back end on Windows: live DWM previews (`dwm`), the frame clock (`DwmFlush`),
//! and `--animation-preview` (`preview`).

mod dwm;
pub mod preview;

/// The back end `crate::anim::Animator` uses.
pub type Native = dwm::Dwm;

/// Blocks until the compositor has composed the next frame; false if it can't tell (no
/// compositor), and the caller sleeps instead.
pub fn wait_frame() -> bool {
    unsafe { windows::Win32::Graphics::Dwm::DwmFlush() }.is_ok()
}
