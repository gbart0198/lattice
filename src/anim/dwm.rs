//! The real animation backend: live DWM previews (thumbnails) on a click-through overlay that
//! covers every monitor, and windows made transparent while their preview stands in for them.
//!
//! The overlay is a `WS_EX_NOREDIRECTIONBITMAP` window: it has no surface of its own, so it
//! costs no GPU memory and draws nothing but the previews on it. It exists only while
//! something animates.

use std::collections::HashSet;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION, DWM_TNP_RECTSOURCE, DWM_TNP_SOURCECLIENTAREAONLY,
    DWM_TNP_VISIBLE, DwmQueryThumbnailSourceSize, DwmRegisterThumbnail, DwmUnregisterThumbnail, DwmUpdateThumbnailProperties,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

use super::{Backend, Thumb, clip_preview};
use crate::layout::Rect;
use crate::shared;
use crate::win::{self, Faded, Hwnd};

#[derive(Default)]
pub struct Dwm {
    overlay: Option<(HWND, Rect)>,
    /// Previews that exist, so `idle` and drop can clean up.
    thumbs: HashSet<Thumb>,
}

unsafe extern "system" fn overlay_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        // Clicks go to whatever is underneath.
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// The whole desktop, all monitors.
fn virtual_screen() -> Rect {
    unsafe {
        Rect::new(
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

fn rect(r: Rect) -> RECT {
    RECT { left: r.x, top: r.y, right: r.right(), bottom: r.bottom() }
}

impl Dwm {
    fn overlay(&mut self) -> Option<(HWND, Rect)> {
        if let Some(o) = self.overlay {
            return Some(o);
        }
        let area = virtual_screen();
        unsafe {
            let inst = GetModuleHandleW(None).ok()?;
            let class = w!("lattice-anim-overlay");
            let wc = WNDCLASSW { lpfnWndProc: Some(overlay_proc), hInstance: inst.into(), lpszClassName: class, ..Default::default() };
            // Fails harmlessly once it's registered.
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(
                WS_EX_NOREDIRECTIONBITMAP | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class,
                w!(""),
                WS_POPUP,
                area.x,
                area.y,
                area.w,
                area.h,
                None,
                None,
                Some(inst.into()),
                None,
            )
            .ok()?;
            // A layered window isn't drawn at all until it has attributes.
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            self.overlay = Some((hwnd, area));
        }
        self.overlay
    }
}

impl Backend for Dwm {
    fn fade(&mut self, h: Hwnd) -> Option<Faded> {
        let f = win::fade(h)?;
        // Recorded first thing, so even a crash leaves a way to make it opaque again.
        shared::mark_faded(h, Some(f));
        Some(f)
    }

    fn unfade(&mut self, h: Hwnd, f: Faded) {
        win::unfade(h, f);
        shared::mark_faded(h, None);
    }

    fn preview(&mut self, h: Hwnd) -> Option<Thumb> {
        let (overlay, _) = self.overlay()?;
        let t = unsafe { DwmRegisterThumbnail(overlay, win::h(h)) }.ok()?;
        self.thumbs.insert(t);
        Some(t)
    }

    fn show(&mut self, t: Thumb, at: Rect, clip: Option<Rect>) {
        let Some((overlay, origin)) = self.overlay else { return };
        // Other always-on-top windows (the bar, the focus border, an app's) can be raised over
        // it mid-animation: keep it the topmost window. A cheap check; it rarely moves.
        unsafe {
            if GetTopWindow(None).ok() != Some(overlay) {
                let _ = SetWindowPos(overlay, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        }
        let Ok(size) = (unsafe { DwmQueryThumbnailSourceSize(t) }) else { return };
        // The source is the window's visible frame (no invisible resize borders), the same
        // rect lattice tiles by, so `at` maps onto it directly.
        let (dest, src, visible) = match clip_preview(at, clip, (size.cx, size.cy)) {
            Some((d, s)) => (d, s, true),
            None => (at, Rect::new(0, 0, size.cx, size.cy), false),
        };
        let props = DWM_THUMBNAIL_PROPERTIES {
            dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_RECTSOURCE | DWM_TNP_VISIBLE | DWM_TNP_OPACITY | DWM_TNP_SOURCECLIENTAREAONLY,
            rcDestination: rect(dest.offset(-origin.x, -origin.y)),
            rcSource: rect(src),
            opacity: 255,
            fVisible: visible.into(),
            fSourceClientAreaOnly: false.into(),
        };
        let _ = unsafe { DwmUpdateThumbnailProperties(t, &props) };
    }

    fn drop_preview(&mut self, t: Thumb) {
        if self.thumbs.remove(&t) {
            let _ = unsafe { DwmUnregisterThumbnail(t) };
        }
    }

    fn idle(&mut self) {
        for t in self.thumbs.drain() {
            let _ = unsafe { DwmUnregisterThumbnail(t) };
        }
        if let Some((hwnd, _)) = self.overlay.take() {
            let _ = unsafe { DestroyWindow(hwnd) };
        }
    }

    fn is_visible(&self, h: Hwnd) -> bool {
        win::is_visible(h)
    }

    fn place(&mut self, h: Hwnd, r: Rect) {
        win::place(h, r);
    }

    fn arrived(&self, h: Hwnd, r: Rect) -> bool {
        win::frame_rect(h) == Some(r)
    }
}

impl Drop for Dwm {
    fn drop(&mut self) {
        self.idle();
    }
}
