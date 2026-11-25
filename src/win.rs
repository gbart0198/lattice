//! Thin, safe-ish wrappers over the Win32 calls lattice needs. Window handles are stored as
//! `isize` (`Hwnd`) so they can live in maps and cross threads; `h()` turns one back into HWND.

use std::ffi::c_void;

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST,
    MONITORINFO, MonitorFromPoint, MonitorFromWindow,
};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEINPUT, SendInput, keybd_event, KEYEVENTF_KEYUP, KEYBD_EVENT_FLAGS,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, PWSTR};

use crate::layout::Rect;

pub type Hwnd = isize;

pub fn h(hwnd: Hwnd) -> HWND {
    HWND(hwnd as *mut c_void)
}

pub fn own_pid() -> u32 {
    unsafe { GetCurrentProcessId() }
}

pub fn is_window(hwnd: Hwnd) -> bool {
    unsafe { IsWindow(Some(h(hwnd))).as_bool() }
}

pub fn is_visible(hwnd: Hwnd) -> bool {
    unsafe { IsWindowVisible(h(hwnd)).as_bool() }
}

pub fn is_minimized(hwnd: Hwnd) -> bool {
    unsafe { IsIconic(h(hwnd)).as_bool() }
}

pub fn is_maximized(hwnd: Hwnd) -> bool {
    unsafe { IsZoomed(h(hwnd)).as_bool() }
}

pub fn is_cloaked(hwnd: Hwnd) -> bool {
    let mut cloaked: u32 = 0;
    unsafe {
        DwmGetWindowAttribute(h(hwnd), DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut c_void, 4).is_ok() && cloaked != 0
    }
}

pub fn style(hwnd: Hwnd) -> WINDOW_STYLE {
    unsafe { WINDOW_STYLE(GetWindowLongPtrW(h(hwnd), GWL_STYLE) as u32) }
}

pub fn ex_style(hwnd: Hwnd) -> WINDOW_EX_STYLE {
    unsafe { WINDOW_EX_STYLE(GetWindowLongPtrW(h(hwnd), GWL_EXSTYLE) as u32) }
}

pub fn title(hwnd: Hwnd) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { GetWindowTextW(h(hwnd), &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

pub fn class(hwnd: Hwnd) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(h(hwnd), &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

pub fn pid(hwnd: Hwnd) -> u32 {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(h(hwnd), Some(&mut pid)) };
    pid
}

/// The process image file name, e.g. "firefox.exe". Empty when access is denied.
pub fn exe_name(hwnd: Hwnd) -> String {
    exe_path(hwnd).rsplit(['\\', '/']).next().unwrap_or_default().to_string()
}

/// The full path of the window's process image. Empty when access is denied.
pub fn exe_path(hwnd: Hwnd) -> String {
    let pid = pid(hwnd);
    unsafe {
        let Ok(proc) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return String::new();
        };
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(proc, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(proc);
        if !ok {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..len as usize])
    }
}

pub fn owner(hwnd: Hwnd) -> Option<Hwnd> {
    unsafe { GetWindow(h(hwnd), GW_OWNER).ok().filter(|o| !o.is_invalid()).map(|o| o.0 as isize) }
}

pub fn is_top_level(hwnd: Hwnd) -> bool {
    unsafe { GetAncestor(h(hwnd), GA_ROOT).0 as isize == hwnd }
}

const IGNORED_CLASSES: &[&str] = &[
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "Progman",
    "WorkerW",
    "Windows.UI.Core.CoreWindow",
    "ForegroundStaging",
    "MultitaskingViewFrame",
    "XamlExplorerHostIslandWindow",
    "TopLevelWindowForOverflowXamlIsland",
    "NotifyIconOverflowWindow",
    "tooltips_class32",
    "#32768", // menus
    "IME",
    "MSCTFIME UI",
    "Windows.Internal.Shell.TabProxyWindow",
];

const IGNORED_EXES: &[&str] = &["ShellExperienceHost.exe", "StartMenuExperienceHost.exe", "SearchHost.exe", "LockApp.exe"];

/// Facts about a window that decide whether (and how) lattice manages it.
pub struct Candidate {
    pub exe: String,
    pub class: String,
    pub title: String,
    pub resizable: bool,
}

/// Returns `Some` for real application windows: visible, top-level, uncloaked, unowned (or
/// app-windowed), not a tool window, not shell UI, not ours.
pub fn candidate(hwnd: Hwnd) -> Option<Candidate> {
    // Cheapest checks first: Show and NameChange events bring every window on the desktop here.
    if hwnd == 0 || !is_window(hwnd) || !is_visible(hwnd) || !is_top_level(hwnd) {
        return None;
    }
    let st = style(hwnd);
    let ex = ex_style(hwnd);
    if st.contains(WS_CHILD) || ex.contains(WS_EX_TOOLWINDOW) || ex.contains(WS_EX_NOACTIVATE) {
        return None;
    }
    if pid(hwnd) == own_pid() || is_cloaked(hwnd) {
        return None;
    }
    if owner(hwnd).is_some() && !ex.contains(WS_EX_APPWINDOW) {
        return None;
    }
    // Real app windows have a caption; borderless popups (splash screens, overlays) don't.
    if !st.contains(WS_CAPTION) && !st.contains(WS_THICKFRAME) {
        return None;
    }
    let class = class(hwnd);
    if IGNORED_CLASSES.contains(&class.as_str()) {
        return None;
    }
    let title = title(hwnd);
    if title.is_empty() {
        return None;
    }
    let exe = exe_name(hwnd);
    if IGNORED_EXES.iter().any(|e| e.eq_ignore_ascii_case(&exe)) {
        return None;
    }
    // lattice's own settings window runs as a second lattice.exe: it floats on top, untiled,
    // and stays put across workspace switches.
    if exe.eq_ignore_ascii_case("lattice.exe") {
        return None;
    }
    Some(Candidate { exe, class, title, resizable: st.contains(WS_THICKFRAME) })
}

pub fn enum_windows() -> Vec<Hwnd> {
    unsafe extern "system" fn cb(hwnd: HWND, lp: LPARAM) -> BOOL {
        let v = unsafe { &mut *(lp.0 as *mut Vec<Hwnd>) };
        v.push(hwnd.0 as isize);
        BOOL(1)
    }
    let mut v: Vec<Hwnd> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut v as *mut _ as isize));
    }
    v
}

fn from_rect(r: RECT) -> Rect {
    Rect::new(r.left, r.top, r.right - r.left, r.bottom - r.top)
}

pub fn window_rect(hwnd: Hwnd) -> Option<Rect> {
    let mut r = RECT::default();
    unsafe { GetWindowRect(h(hwnd), &mut r).ok()? };
    Some(from_rect(r))
}

/// The visible frame (what the user sees), excluding Windows 10/11's invisible resize borders.
pub fn frame_rect(hwnd: Hwnd) -> Option<Rect> {
    let mut r = RECT::default();
    unsafe {
        DwmGetWindowAttribute(
            h(hwnd),
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut r as *mut RECT as *mut c_void,
            std::mem::size_of::<RECT>() as u32,
        )
        .ok()?;
    }
    Some(from_rect(r))
}

/// Moves a window so its *visible frame* lands exactly on `target`.
pub fn place(hwnd: Hwnd, target: Rect) {
    unsafe {
        if is_maximized(hwnd) || is_minimized(hwnd) {
            let _ = ShowWindowAsync(h(hwnd), SW_RESTORE);
        }
        let (mut l, mut t, mut r, mut b) = (0, 0, 0, 0);
        if let (Some(w), Some(f)) = (window_rect(hwnd), frame_rect(hwnd)) {
            l = f.x - w.x;
            t = f.y - w.y;
            r = w.right() - f.right();
            b = w.bottom() - f.bottom();
            // Guard against garbage when a window is mid-transition.
            if [l, t, r, b].iter().any(|v| !(0..=32).contains(v)) {
                (l, t, r, b) = (0, 0, 0, 0);
            }
        }
        let flags = SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_ASYNCWINDOWPOS;
        let x = target.x - l;
        let y = target.y - t;
        let w = target.w + l + r;
        let hh = target.h + t + b;
        let crossing = monitor_of(hwnd) != monitor_at(target.center());
        let _ = SetWindowPos(h(hwnd), None, x, y, w, hh, flags);
        // Crossing to a monitor with a different DPI makes the app rescale itself after the
        // first move; a second move pins the final size.
        if crossing {
            let _ = SetWindowPos(h(hwnd), None, x, y, w, hh, flags);
        }
    }
}

pub fn raise(hwnd: Hwnd) {
    unsafe {
        let _ = SetWindowPos(h(hwnd), Some(HWND_TOP), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
    }
}

/// Makes a window render a fresh frame without focusing it: invalidate everything, then a
/// 1px resize and back (Chromium/CEF apps such as Steam only repaint reliably on resize).
pub fn repaint(hwnd: Hwnd) {
    use windows::Win32::Graphics::Gdi::{RDW_ALLCHILDREN, RDW_ERASE, RDW_FRAME, RDW_INVALIDATE, RedrawWindow};
    unsafe {
        let _ = RedrawWindow(Some(h(hwnd)), None, None, RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN);
        if let Some(r) = window_rect(hwnd) {
            let flags = SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_ASYNCWINDOWPOS;
            let _ = SetWindowPos(h(hwnd), None, 0, 0, r.w, r.h - 1, flags);
            let _ = SetWindowPos(h(hwnd), None, 0, 0, r.w, r.h, flags);
        }
    }
}

/// One of lattice's own top-level windows, by title (the Rune overlay windows).
pub fn find_own_window(title_wanted: &str) -> Hwnd {
    let me = own_pid();
    enum_windows().into_iter().find(|w| pid(*w) == me && title(*w) == title_wanted).unwrap_or(0)
}

/// Shows an overlay window at `r`, above everything, without activating it.
pub fn place_overlay(hwnd: Hwnd, r: Rect) {
    unsafe {
        let flags = SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_NOOWNERZORDER | SWP_ASYNCWINDOWPOS;
        let _ = SetWindowPos(h(hwnd), Some(HWND_TOPMOST), r.x, r.y, r.w, r.h, flags);
    }
}

pub fn hide(hwnd: Hwnd) {
    unsafe {
        let _ = ShowWindowAsync(h(hwnd), SW_HIDE);
    }
}

pub fn show(hwnd: Hwnd) {
    unsafe {
        let _ = ShowWindowAsync(h(hwnd), SW_SHOWNA);
    }
}

pub fn close(hwnd: Hwnd) {
    unsafe {
        let _ = PostMessageW(Some(h(hwnd)), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
}

pub fn foreground() -> Hwnd {
    unsafe { GetForegroundWindow().0 as isize }
}

/// Windows only lets the process that received the last input take the foreground. Lattice
/// swallowed that input in its keyboard hook, so a no-op mouse event re-establishes it.
pub fn focus(hwnd: Hwnd) {
    unsafe {
        if is_minimized(hwnd) {
            let _ = ShowWindowAsync(h(hwnd), SW_RESTORE);
        }
        if SetForegroundWindow(h(hwnd)).as_bool() && foreground() == hwnd {
            return;
        }
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 { mi: MOUSEINPUT { dwFlags: MOUSEEVENTF_MOVE, ..Default::default() } },
        };
        SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
        if SetForegroundWindow(h(hwnd)).as_bool() && foreground() == hwnd {
            return;
        }
        // Last resort: a synthetic Alt tap unlocks SetForegroundWindow.
        keybd_event(0x12, 0, KEYBD_EVENT_FLAGS(0), 0);
        keybd_event(0x12, 0, KEYEVENTF_KEYUP, 0);
        let _ = SetForegroundWindow(h(hwnd));
    }
}

/// Gives focus to the desktop so keys don't go to a window that was just hidden.
pub fn focus_desktop() {
    unsafe {
        let shell = GetShellWindow();
        if !shell.is_invalid() {
            focus(shell.0 as isize);
        }
    }
}

pub fn cursor_pos() -> (i32, i32) {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    (p.x, p.y)
}

pub fn set_cursor(x: i32, y: i32) {
    unsafe {
        let _ = SetCursorPos(x, y);
    }
}

// ---------------------------------------------------------------------------------------------
// Monitors

#[derive(Clone, Debug)]
pub struct MonitorInfo {
    pub handle: isize,
    pub rect: Rect,
    /// Work area: the monitor minus the Windows taskbar.
    pub work: Rect,
    pub primary: bool,
    pub scale: f32,
}

pub fn monitors() -> Vec<MonitorInfo> {
    unsafe extern "system" fn cb(hm: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> BOOL {
        let v = unsafe { &mut *(lp.0 as *mut Vec<MonitorInfo>) };
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if unsafe { GetMonitorInfoW(hm, &mut mi) }.as_bool() {
            let (mut dx, mut dy) = (96u32, 96u32);
            let _ = unsafe { GetDpiForMonitor(hm, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) };
            v.push(MonitorInfo {
                handle: hm.0 as isize,
                rect: from_rect(mi.rcMonitor),
                work: from_rect(mi.rcWork),
                primary: mi.dwFlags & 1 != 0,
                scale: dx as f32 / 96.0,
            });
        }
        BOOL(1)
    }
    let mut v: Vec<MonitorInfo> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut v as *mut _ as isize));
    }
    // Left-to-right, top-to-bottom, so "next monitor" is predictable.
    v.sort_by_key(|m| (m.rect.x, m.rect.y));
    v
}

pub fn monitor_of(hwnd: Hwnd) -> isize {
    unsafe { MonitorFromWindow(h(hwnd), MONITOR_DEFAULTTONEAREST).0 as isize }
}

pub fn monitor_at(p: (i32, i32)) -> isize {
    unsafe { MonitorFromPoint(POINT { x: p.0, y: p.1 }, MONITOR_DEFAULTTONEAREST).0 as isize }
}
