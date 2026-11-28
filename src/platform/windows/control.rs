//! Other lattice processes (the tour, the settings window) talking to the running lattice,
//! through its hidden "lattice-control" window (made in `hooks.rs`): plain window messages,
//! answered on the WM thread. `lattice msg/query` go through `ipc` instead.

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_APP, WM_CLOSE};
use windows::core::w;

/// Settings -> lattice: "config.toml was saved, reload it".
pub const WM_RELOAD: u32 = WM_APP + 2;
/// Tour -> lattice: "what's going on?" `wParam` 1 while the tour runs, 0 as it ends. Returns
/// `shared::TourStatus::pack`.
pub const WM_TOUR_STATUS: u32 = WM_APP + 4;
/// Tour -> lattice: "where is this?" `wParam` a `shared::Anchor`. Returns `shared::pack_rect`,
/// 0 for nowhere.
pub const WM_TOUR_RECT: u32 = WM_APP + 5;

/// The running lattice, as another process reaches it.
pub struct Lattice(HWND);

/// The running lattice, if there is one.
pub fn find() -> Option<Lattice> {
    unsafe { FindWindowW(w!("lattice-display-listener"), w!("lattice-control")) }
        .ok()
        .filter(|h| !h.is_invalid())
        .map(Lattice)
}

impl Lattice {
    /// One question; None when it doesn't answer in time.
    fn ask(&self, msg: u32, wp: usize) -> Option<u64> {
        let mut result = 0usize;
        let ok = unsafe { SendMessageTimeoutW(self.0, msg, WPARAM(wp), LPARAM(0), SMTO_ABORTIFHUNG, 250, Some(&mut result)) };
        (ok.0 != 0).then_some(result as u64)
    }

    /// The tour checks in (`running`) or says it ended; `shared::TourStatus::pack`ed.
    pub fn tour_status(&self, running: bool) -> Option<u64> {
        self.ask(WM_TOUR_STATUS, running as usize)
    }

    /// Where `shared::Anchor` number `anchor` is, `shared::pack_rect`ed (0: nowhere).
    pub fn tour_rect(&self, anchor: usize) -> Option<u64> {
        self.ask(WM_TOUR_RECT, anchor)
    }

    /// Asks it to read config.toml again; false if the message couldn't be sent.
    pub fn reload(&self) -> bool {
        unsafe { PostMessageW(Some(self.0), WM_RELOAD, WPARAM(0), LPARAM(0)).is_ok() }
    }

    /// Asks it to quit the clean way (every hidden window shown again).
    pub fn quit(&self) {
        unsafe {
            let _ = PostMessageW(Some(self.0), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
}
