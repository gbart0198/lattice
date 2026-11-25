//! The focus border: a rounded, glowing frame around the focused window, drawn as four
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

/// The frame's shape at one scale, in physical pixels.
struct Geometry {
    /// How far the overlay extends past the window on each side.
    margin: i32,
    /// Frame line width, inside the window's edge.
    width: f32,
    radius: f32,
    /// Glow reach outside the frame, and its faint falloff inside it (0: no glow).
    glow: f32,
    inner: f32,
    opacity: f32,
    /// Strip thickness: every pixel further than this from the overlay's edge is transparent.
    thick: i32,
}

impl Geometry {
    fn new(cfg: &config::Border, scale: f32) -> Self {
        let glowing = cfg.glow > 0.0 && cfg.glow_opacity > 0.0;
        let width = (cfg.width * scale).max(0.0);
        let radius = (cfg.radius * scale).max(0.0);
        let glow = if glowing { (cfg.glow * scale).ceil() } else { 0.0 };
        let inner = if glowing { (cfg.glow * 0.3 * scale).ceil() } else { 0.0 };
        let margin = (cfg.margin() * scale).round() as i32;
        // Past `margin + radius` from a corner both ways, a pixel's distance to the frame is
        // its distance to the nearest straight edge; past `width + inner` inside, it's clear.
        let thick = margin + radius.max(width + inner).ceil() as i32 + 2;
        Geometry { margin, width, radius, glow, inner, opacity: cfg.glow_opacity.clamp(0.0, 1.0), thick }
    }

    /// Straight alpha (0-1) at signed distance `d` from the window's edge (positive outside).
    /// The same shape the concentric rings of the old Rune overlay drew, without the steps.
    fn alpha(&self, d: f32) -> f32 {
        let frame = if self.width > 0.0 { (0.5 - d).clamp(0.0, 1.0) * (d + self.width + 0.5).clamp(0.0, 1.0) } else { 0.0 };
        let glow = if d > 0.0 && self.glow > 0.0 {
            let t = (d / (self.glow + 1.0)).min(1.0);
            self.opacity * (1.0 - t).powi(2)
        } else if d < -self.width && self.inner > 0.0 {
            let t = ((-d - self.width) / (self.inner + 1.0)).min(1.0);
            self.opacity * 0.4 * (1.0 - t).powi(2)
        } else {
            0.0
        };
        // The frame line over its glow.
        frame + (1.0 - frame) * glow
    }
}

/// Signed distance from `p` to a rounded rectangle centred on `c` with half-size `half`
/// (negative inside).
fn rounded_rect_sdf(p: (f32, f32), c: (f32, f32), half: (f32, f32), r: f32) -> f32 {
    let qx = (p.0 - c.0).abs() - (half.0 - r);
    let qy = (p.1 - c.1).abs() - (half.1 - r);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

/// The four strips of a `w` x `h` overlay, in overlay coordinates: top, bottom, left, right.
/// Small overlays split between top and bottom (and left and right) with no overlap or gap.
fn strip_rects(w: i32, h: i32, thick: i32) -> [Rect; 4] {
    let top = thick.min(h / 2);
    let bottom = thick.min(h - top);
    let left = thick.min(w / 2);
    let right = thick.min(w - left);
    let side_h = h - top - bottom;
    [
        Rect::new(0, 0, w, top),
        Rect::new(0, h - bottom, w, bottom),
        Rect::new(0, top, left, side_h),
        Rect::new(w - right, top, right, side_h),
    ]
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
        let c = crate::bar::color(&cfg.color, rune::Color { r: 0.25, g: 0.45, b: 0.95, a: 1.0 });
        // `bar::color` returns linear values; layered windows blend in sRGB.
        let srgb = |v: f32| {
            let s = if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
            (s.clamp(0.0, 1.0) * 255.0).round() as u8
        };
        let rgb = [srgb(c.r), srgb(c.g), srgb(c.b)];
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
                    self.fill(px, g, size, r);
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

    /// Premultiplied BGRA pixels of strip `r` of an overlay of `size`.
    fn fill(&self, px: &mut [u8], g: &Geometry, size: (i32, i32), r: Rect) {
        let c = (size.0 as f32 / 2.0, size.1 as f32 / 2.0);
        let half = ((c.0 - g.margin as f32).max(0.0), (c.1 - g.margin as f32).max(0.0));
        let radius = g.radius.min(half.0).min(half.1);
        let [red, green, blue] = self.rgb.map(|v| v as f32);
        for y in 0..r.h {
            for x in 0..r.w {
                let p = ((r.x + x) as f32 + 0.5, (r.y + y) as f32 + 0.5);
                let a = g.alpha(rounded_rect_sdf(p, c, half, radius));
                let i = ((y * r.w + x) * 4) as usize;
                px[i] = (blue * a).round() as u8;
                px[i + 1] = (green * a).round() as u8;
                px[i + 2] = (red * a).round() as u8;
                px[i + 3] = (255.0 * a).round() as u8;
            }
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
    let monitors = crate::win::monitors();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_tile_the_overlay_without_overlap() {
        for (w, h, t) in [(800, 600, 30), (50, 40, 30), (61, 7, 30), (1, 1, 30), (0, 0, 30)] {
            let mut seen = vec![0u8; (w * h) as usize];
            for r in strip_rects(w, h, t) {
                for y in r.y..r.y + r.h.max(0) {
                    for x in r.x..r.x + r.w.max(0) {
                        seen[(y * w + x) as usize] += 1;
                    }
                }
            }
            // Everything a strip skips is the clear middle; nothing is covered twice.
            for y in 0..h {
                for x in 0..w {
                    let n = seen[(y * w + x) as usize];
                    let middle = x >= t && x < w - t && y >= t && y < h - t;
                    assert_eq!(n, if middle { 0 } else { 1 }, "{w}x{h} at ({x}, {y})");
                }
            }
        }
    }

    #[test]
    fn nothing_drawn_where_strips_skip() {
        let cfg = config::Border { glow: 14.0, width: 3.0, radius: 24.0, ..Default::default() };
        for scale in [1.0, 1.5, 2.0] {
            let g = Geometry::new(&cfg, scale);
            let (w, h) = (900, 700);
            let c = (w as f32 / 2.0, h as f32 / 2.0);
            let half = (c.0 - g.margin as f32, c.1 - g.margin as f32);
            for y in g.thick..h - g.thick {
                for x in g.thick..w - g.thick {
                    let d = rounded_rect_sdf((x as f32 + 0.5, y as f32 + 0.5), c, half, g.radius);
                    assert!(g.alpha(d) * 255.0 < 0.5, "scale {scale}: visible pixel at ({x}, {y})");
                }
            }
        }
    }

    #[test]
    fn frame_is_solid_on_the_edge_and_glow_fades() {
        let g = Geometry::new(&config::Border::default(), 1.0);
        assert!(g.alpha(-1.0) > 0.99, "inside the frame line");
        assert!(g.alpha(2.0) > g.alpha(8.0));
        assert_eq!(g.alpha(g.glow + 1.0), 0.0);
    }
}
