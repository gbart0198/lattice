//! Draws lattice's app icon (the dwindle tiles) at several sizes, packs them into an .ico and
//! embeds it in the executable, so Explorer, the Start menu and Task Manager show it.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let ico = out.join("lattice.ico");
    std::fs::write(&ico, make_ico(&[16, 24, 32, 48, 64, 256])).unwrap();
    let rc = out.join("lattice.rc");
    let ico_path = ico.display().to_string().replace('\\', "\\\\");
    std::fs::write(&rc, format!("1 ICON \"{ico_path}\"\n")).unwrap();
    embed_resource::compile(&rc, embed_resource::NONE).manifest_optional().unwrap();
}

/// sRGB colours, straight alpha.
type Rgba = [f32; 4];

fn hex(s: &str) -> Rgba {
    let p = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).unwrap() as f32 / 255.0;
    [p(1), p(3), p(5), 1.0]
}

/// Signed distance to a rounded rectangle (negative inside).
fn rounded_rect_sd(px: f32, py: f32, x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    let qx = (px - cx).abs() - (w / 2.0 - r);
    let qy = (py - cy).abs() - (h / 2.0 - r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

fn over(dst: Rgba, src: Rgba) -> Rgba {
    let a = src[3] + dst[3] * (1.0 - src[3]);
    if a <= 0.0 {
        return [0.0; 4];
    }
    let c = |i: usize| (src[i] * src[3] + dst[i] * dst[3] * (1.0 - src[3])) / a;
    [c(0), c(1), c(2), a]
}

/// Renders the icon at `n` x `n`, 4x4 supersampled. Returns BGRA rows, top first.
fn render(n: u32) -> Vec<u8> {
    let s = n as f32;
    let bg = hex("#1e1e2e");
    let tiles_col = [hex("#89b4fa"), hex("#cba6f7"), hex("#f5c2e7"), hex("#94e2d5")];
    // Background plate, then the dwindle glyph inside it.
    let plate_r = s * 0.22;
    let m = s * 0.17;
    let (gx, gy, gw, gh) = (m, m + s * 0.04, s - 2.0 * m, s - 2.0 * m - s * 0.08);
    let g = (s * 0.055).max(1.0);
    let hw = (gw - g) / 2.0;
    let rx = gx + hw + g;
    let th = (gh - g) / 2.0;
    let bw = (gx + gw - rx - g) / 2.0;
    let tiles = [
        (gx, gy, hw, gh),
        (rx, gy, gx + gw - rx, th),
        (rx, gy + th + g, bw, th),
        (rx + bw + g, gy + th + g, bw, th),
    ];
    let tile_r = (s * 0.06).max(1.0);
    let mut out = Vec::with_capacity((n * n * 4) as usize);
    const SS: u32 = 4;
    for py in 0..n {
        for px in 0..n {
            let mut acc = [0f32; 4];
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = px as f32 + (sx as f32 + 0.5) / SS as f32;
                    let y = py as f32 + (sy as f32 + 0.5) / SS as f32;
                    let mut c: Rgba = [0.0; 4];
                    if rounded_rect_sd(x, y, 0.0, 0.0, s, s, plate_r) <= 0.0 {
                        c = bg;
                    }
                    for (i, (tx, ty, tw, tth)) in tiles.iter().enumerate() {
                        if rounded_rect_sd(x, y, *tx, *ty, *tw, *tth, tile_r) <= 0.0 {
                            // Vertical gradient: full colour at the top, darker at the bottom.
                            let k = ((y - ty) / tth).clamp(0.0, 1.0) * 0.3;
                            let col = tiles_col[i];
                            let tint = [col[0] * (1.0 - k) + bg[0] * k, col[1] * (1.0 - k) + bg[1] * k, col[2] * (1.0 - k) + bg[2] * k, 1.0];
                            c = over(c, tint);
                        }
                    }
                    // Accumulate premultiplied.
                    acc[0] += c[0] * c[3];
                    acc[1] += c[1] * c[3];
                    acc[2] += c[2] * c[3];
                    acc[3] += c[3];
                }
            }
            let k = (SS * SS) as f32;
            let a = acc[3] / k;
            let un = |v: f32| if a > 0.0 { (v / k / a * 255.0).round().clamp(0.0, 255.0) as u8 } else { 0 };
            out.extend_from_slice(&[un(acc[2]), un(acc[1]), un(acc[0]), (a * 255.0).round() as u8]);
        }
    }
    out
}

/// An .ico with one 32-bit BMP image per size.
fn make_ico(sizes: &[u32]) -> Vec<u8> {
    let images: Vec<Vec<u8>> = sizes
        .iter()
        .map(|&n| {
            let bgra = render(n);
            let mut img = Vec::new();
            // BITMAPINFOHEADER; height is doubled to include the (empty) AND mask.
            img.extend_from_slice(&40u32.to_le_bytes());
            img.extend_from_slice(&(n as i32).to_le_bytes());
            img.extend_from_slice(&((n * 2) as i32).to_le_bytes());
            img.extend_from_slice(&1u16.to_le_bytes());
            img.extend_from_slice(&32u16.to_le_bytes());
            img.extend_from_slice(&[0u8; 24]);
            // Pixels bottom-up.
            for row in (0..n).rev() {
                let start = (row * n * 4) as usize;
                img.extend_from_slice(&bgra[start..start + (n * 4) as usize]);
            }
            // AND mask: 1 bit per pixel, rows padded to 32 bits, all zero (alpha decides).
            let mask_row = n.div_ceil(32) * 4;
            img.extend(std::iter::repeat_n(0u8, (mask_row * n) as usize));
            img
        })
        .collect();
    let mut ico = Vec::new();
    ico.extend_from_slice(&0u16.to_le_bytes());
    ico.extend_from_slice(&1u16.to_le_bytes());
    ico.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (n, img) in sizes.iter().zip(&images) {
        let dim = if *n >= 256 { 0u8 } else { *n as u8 };
        ico.extend_from_slice(&[dim, dim, 0, 0]);
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&32u16.to_le_bytes());
        ico.extend_from_slice(&(img.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += img.len() as u32;
    }
    for img in images {
        ico.extend_from_slice(&img);
    }
    ico
}
