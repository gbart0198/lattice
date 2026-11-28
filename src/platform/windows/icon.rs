//! App icons on Windows: the window's own icon, else the icon embedded in its .exe, turned into
//! straight-alpha RGBA that Rune can display.

use std::ffi::c_void;
use std::sync::Arc;

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject,
    HGDIOBJ, SelectObject,
};
use windows::Win32::UI::Shell::ExtractIconExW;
use windows::Win32::UI::WindowsAndMessaging::{
    DI_NORMAL, DestroyIcon, DrawIconEx, GCLP_HICON, GCLP_HICONSM, GetClassLongPtrW, HICON, ICON_BIG, ICON_SMALL2,
    SMTO_ABORTIFHUNG, SMTO_BLOCK, SendMessageTimeoutW, WM_GETICON,
};
use windows::core::HSTRING;

use super::win::{self, Hwnd};
use crate::icon::{AppIcon, SIZE};

/// The icon for a window, or None if neither the window nor its exe has one.
pub fn for_window(hwnd: Hwnd) -> Option<AppIcon> {
    // The window's icon matches what the taskbar shows (PWAs, per-document icons).
    if let Some(rgba) = window_icon(hwnd).and_then(render) {
        return Some(AppIcon::new(rgba));
    }
    for_exe(&win::exe_path(hwnd))
}

/// The first icon embedded in an executable.
pub fn for_exe(path: &str) -> Option<AppIcon> {
    if path.is_empty() {
        return None;
    }
    unsafe {
        let mut large = HICON::default();
        if ExtractIconExW(&HSTRING::from(path), 0, Some(&mut large), None, 1) == 0 || large.is_invalid() {
            return None;
        }
        let rgba = render(large);
        let _ = DestroyIcon(large);
        rgba.map(AppIcon::new)
    }
}

/// Icons a window hands out belong to it: never destroy them.
fn window_icon(hwnd: Hwnd) -> Option<HICON> {
    unsafe {
        for kind in [ICON_BIG, ICON_SMALL2] {
            let mut result = 0usize;
            let ok = SendMessageTimeoutW(
                win::h(hwnd),
                WM_GETICON,
                WPARAM(kind as usize),
                LPARAM(0),
                SMTO_ABORTIFHUNG | SMTO_BLOCK,
                100,
                Some(&mut result),
            );
            if ok.0 != 0 && result != 0 {
                return Some(HICON(result as *mut c_void));
            }
        }
        for index in [GCLP_HICON, GCLP_HICONSM] {
            let h = GetClassLongPtrW(win::h(hwnd), index);
            if h != 0 {
                return Some(HICON(h as *mut c_void));
            }
        }
    }
    None
}

/// Draws the icon on black and on white and recovers straight-alpha RGBA from the difference,
/// which works for 32-bit alpha icons and old masked icons alike.
fn render(icon: HICON) -> Option<Arc<[u8]>> {
    let on_black = draw(icon, 0x00)?;
    let on_white = draw(icon, 0xFF)?;
    let n = (SIZE * SIZE) as usize;
    let mut out = vec![0u8; n * 4];
    let mut any = false;
    for i in 0..n {
        let (b, w) = (&on_black[i * 4..i * 4 + 4], &on_white[i * 4..i * 4 + 4]);
        // DIB pixels are BGRA. alpha = 255 - (white - black), per channel; take the green one.
        let alpha = 255 - (w[1] as i32 - b[1] as i32).clamp(0, 255);
        if alpha > 0 {
            any = true;
            for (c, src) in [(0usize, 2usize), (1, 1), (2, 0)] {
                out[i * 4 + c] = ((b[src] as i32 * 255) / alpha).clamp(0, 255) as u8;
            }
            out[i * 4 + 3] = alpha as u8;
        }
    }
    any.then(|| Arc::from(out))
}

fn draw(icon: HICON, background: u8) -> Option<Vec<u8>> {
    unsafe {
        let dc = CreateCompatibleDC(None);
        if dc.is_invalid() {
            return None;
        }
        let mut info = BITMAPINFO::default();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: SIZE as i32,
            biHeight: -(SIZE as i32), // top-down rows
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let result = match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(bmp) if !bits.is_null() => {
                let old = SelectObject(dc, HGDIOBJ(bmp.0));
                let len = (SIZE * SIZE * 4) as usize;
                std::ptr::write_bytes(bits as *mut u8, background, len);
                let ok = DrawIconEx(dc, 0, 0, icon, SIZE as i32, SIZE as i32, 0, None, DI_NORMAL).is_ok();
                let pixels = ok.then(|| std::slice::from_raw_parts(bits as *const u8, len).to_vec());
                SelectObject(dc, old);
                let _ = DeleteObject(HGDIOBJ(bmp.0));
                pixels
            }
            _ => None,
        };
        let _ = DeleteDC(dc);
        result
    }
}
