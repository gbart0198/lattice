//! The focus border: a rounded, glowing frame around the focused window. The OS back end draws
//! it (`platform::border`; on Windows four click-through layered windows, one per side); its
//! shape is worked out here, the same everywhere.

use crate::config;
use crate::layout::Rect;

pub use crate::platform::border::{Border, preview};

/// The frame colour, sRGB. `bar::color` returns linear values; the strips blend in sRGB.
pub(crate) fn frame_rgb(cfg: &config::Border) -> [u8; 3] {
    let c = crate::bar::color(&cfg.color, rune::Color { r: 0.25, g: 0.45, b: 0.95, a: 1.0 });
    let srgb = |v: f32| {
        let s = if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
        (s.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    [srgb(c.r), srgb(c.g), srgb(c.b)]
}

/// The frame's shape at one scale, in physical pixels.
pub(crate) struct Geometry {
    /// How far the overlay extends past the window on each side.
    pub margin: i32,
    /// Frame line width, inside the window's edge.
    pub width: f32,
    pub radius: f32,
    /// Glow reach outside the frame, and its faint falloff inside it (0: no glow).
    pub glow: f32,
    pub inner: f32,
    pub opacity: f32,
    /// Strip thickness: every pixel further than this from the overlay's edge is transparent.
    pub thick: i32,
}

impl Geometry {
    pub fn new(cfg: &config::Border, scale: f32) -> Self {
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
    pub fn alpha(&self, d: f32) -> f32 {
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
pub(crate) fn rounded_rect_sdf(p: (f32, f32), c: (f32, f32), half: (f32, f32), r: f32) -> f32 {
    let qx = (p.0 - c.0).abs() - (half.0 - r);
    let qy = (p.1 - c.1).abs() - (half.1 - r);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

/// The four strips of a `w` x `h` overlay, in overlay coordinates: top, bottom, left, right.
/// Small overlays split between top and bottom (and left and right) with no overlap or gap.
pub(crate) fn strip_rects(w: i32, h: i32, thick: i32) -> [Rect; 4] {
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

/// Premultiplied BGRA pixels of strip `r` of an overlay of `size`, in sRGB colour `rgb`.
pub(crate) fn fill(px: &mut [u8], rgb: [u8; 3], g: &Geometry, size: (i32, i32), r: Rect) {
    let c = (size.0 as f32 / 2.0, size.1 as f32 / 2.0);
    let half = ((c.0 - g.margin as f32).max(0.0), (c.1 - g.margin as f32).max(0.0));
    let radius = g.radius.min(half.0).min(half.1);
    let [red, green, blue] = rgb.map(|v| v as f32);
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
