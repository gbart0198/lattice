//! The focus border on Windows: the frame (its shape is `crate::border`'s) drawn as four
//! click-through, always-on-top layered windows (the top, bottom, left and right strips of the
//! frame). The WM thread owns them and its message loop serves them.
//!
//! Plain Win32 + GDI on purpose. A transparent GPU-drawn overlay needs a GPU device, and on
//! Windows the device alone (driver, shader compiler, heaps) is ~50 MB of private memory. These
//! strips are painted on the CPU only when the frame's size changes, and in between hold nothing
//! in this process (DWM keeps the pixels). Strips rather than one window around everything: the
//! inside of the frame is empty, so only the band along the edges is ever allocated, and memory
//! scales with the window's perimeter instead of its area.

use std::ffi::c_void;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, HGDIOBJ, SelectObject,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

use crate::border::{Geometry, strip_rects};
use crate::config;
use crate::layout::Rect;

pub struct Border {
    cfg: config::Border,
    /// The frame colour, sRGB.
    rgb: [u8; 3],
    /// Top, bottom, left, right.
    strips: [HWND; 4],
    /// The overlay rectangle on screen (the window's frame plus the margin), if shown.
    at: Option<Rect>,
    /// The overlay size and scale (as bits) the strips' pixels were last painted for.
    painted: Option<(i32, i32, u32)>,
}

unsafe extern "system" fn strip_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

impl Border {
    /// Creates the (hidden) strips on the calling thread, which must pump messages.
    /// None when the border is turned off or the windows can't be made.
    pub fn new(cfg: config::Border) -> Option<Self> {
        if !cfg.enabled {
            return None;
        }
        let rgb = crate::border::frame_rgb(&cfg);
        let mut strips = [HWND::default(); 4];
        unsafe {
            let instance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap_or_default();
            let class = WNDCLASSW {
                lpfnWndProc: Some(strip_proc),
                hInstance: instance.into(),
                lpszClassName: w!("lattice-border"),
                ..Default::default()
            };
            // Fails harmlessly when a reload makes the border again: the class is still there.
            RegisterClassW(&class);
            for i in 0..strips.len() {
                let made = CreateWindowExW(
                    WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                    w!("lattice-border"),
                    w!("lattice-border"),
                    WS_POPUP,
                    0,
                    0,
                    0,
                    0,
                    None,
                    None,
                    Some(instance.into()),
                    None,
                );
                match made {
                    Ok(h) => strips[i] = h,
                    Err(e) => {
                        log::warn!("focus border: {e}; no border");
                        for h in &strips[..i] {
                            let _ = DestroyWindow(*h);
                        }
                        return None;
                    }
                }
            }
        }
        Some(Border { cfg, rgb, strips, at: None, painted: None })
    }

    /// Frames `frame` (a window's visible frame, physical pixels, on a monitor at `scale`), or
    /// hides the border for `None`. Repaints only when the frame's size or scale changed; a
    /// move just moves the strips.
    pub fn set(&mut self, target: Option<(Rect, f32)>) {
        let Some((frame, scale)) = target else {
            if self.at.take().is_some() {
                for h in self.strips {
                    unsafe {
                        let _ = ShowWindow(h, SW_HIDE);
                    }
                }
            }
            return;
        };
        let g = Geometry::new(&self.cfg, scale);
        let m = g.margin;
        let outer = Rect::new(frame.x - m, frame.y - m, frame.w + 2 * m, frame.h + 2 * m);
        let size = (outer.w, outer.h, scale.to_bits());
        if self.at == Some(outer) && self.painted == Some(size) {
            return;
        }
        let rects = strip_rects(outer.w, outer.h, g.thick);
        let above = tour_overlay().unwrap_or(HWND_TOPMOST);
        let repaint = self.painted != Some(size);
        for (h, r) in self.strips.into_iter().zip(rects) {
            let pos = POINT { x: outer.x + r.x, y: outer.y + r.y };
            unsafe {
                if r.w <= 0 || r.h <= 0 {
                    let _ = ShowWindow(h, SW_HIDE);
                    continue;
                }
                if repaint && !self.paint(h, &g, (outer.w, outer.h), r, pos) {
                    let _ = ShowWindow(h, SW_HIDE);
                    continue;
                }
                // Size comes from the painted bitmap; this moves, raises and shows.
                // Raised to the top, but kept under the product tour's overlay while it runs.
                let flags = SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_SHOWWINDOW;
                let _ = SetWindowPos(h, Some(above), pos.x, pos.y, 0, 0, flags);
            }
        }
        if repaint {
            self.painted = Some(size);
        }
        self.at = Some(outer);
    }

    /// Paints strip `r` of an overlay of `size` into layered window `h` at `pos`. The bitmap
    /// only lives for this call: DWM keeps its own copy.
    unsafe fn paint(&self, h: HWND, g: &Geometry, size: (i32, i32), r: Rect, pos: POINT) -> bool {
        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() {
                return false;
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: r.w,
                    biHeight: -r.h, // top-down rows
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let ok = match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(bmp) if !bits.is_null() => {
                    let old = SelectObject(dc, HGDIOBJ(bmp.0));
                    let px = std::slice::from_raw_parts_mut(bits as *mut u8, (r.w * r.h * 4) as usize);
                    crate::border::fill(px, self.rgb, g, size, r);
                    let blend = BLENDFUNCTION {
                        BlendOp: AC_SRC_OVER as u8,
                        BlendFlags: 0,
                        SourceConstantAlpha: 255,
                        AlphaFormat: AC_SRC_ALPHA as u8,
                    };
                    let ok = UpdateLayeredWindow(
                        h,
                        None,
                        Some(&pos),
                        Some(&SIZE { cx: r.w, cy: r.h }),
                        Some(dc),
                        Some(&POINT::default()),
                        COLORREF(0),
                        Some(&blend),
                        ULW_ALPHA,
                    )
                    .is_ok();
                    SelectObject(dc, old);
                    let _ = DeleteObject(HGDIOBJ(bmp.0));
                    ok
                }
                _ => false,
            };
            let _ = DeleteDC(dc);
            ok
        }
    }

}

/// The product tour's overlay window (`lattice --tour`), while a tour is running. The strips
/// raise themselves whenever they move; during a tour they go just under its overlay instead,
/// so the glow never shows through its backdrop or card. (The overlay opens above them, so
/// nothing else needs restacking.)
fn tour_overlay() -> Option<HWND> {
    if !crate::shared::tour_active() {
        return None;
    }
    unsafe { FindWindowW(None, w!("lattice tour")).ok().filter(|h| !h.is_invalid()) }
}

impl Drop for Border {
    fn drop(&mut self) {
        for h in self.strips {
            unsafe {
                let _ = DestroyWindow(h);
            }
        }
    }
}

/// `--border-preview`: the border around a sample rectangle in the middle of the primary
/// monitor, until the process is ended.
pub fn preview(cfg: config::Border) {
    let monitors = crate::platform::win::monitors();
    let Some(m) = monitors.iter().find(|m| m.primary).or(monitors.first()) else { return };
    let (w, h) = (m.work.w * 3 / 5, m.work.h * 3 / 5);
    let frame = Rect::new(m.work.x + (m.work.w - w) / 2, m.work.y + (m.work.h - h) / 2, w, h);
    let Some(mut border) = Border::new(config::Border { enabled: true, ..cfg }) else { return };
    border.set(Some((frame, m.scale)));
    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            DispatchMessageW(&msg);
        }
    }
}

