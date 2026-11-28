//! What `--launcher-bench` needs from Windows: whether the launcher is on screen, and typing.

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY, VK_ESCAPE,
};
use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;

/// Shown and not cloaked by DWM: on screen.
pub fn on_screen(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }
    let hwnd = HWND(hwnd as *mut _);
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            return false;
        }
        let mut cloaked = 0u32;
        let ok = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, 4).is_ok();
        ok && cloaked == 0
    }
}

/// Types an ASCII letter (down and up).
pub fn press_letter(c: char) {
    key(VIRTUAL_KEY(c.to_ascii_uppercase() as u16));
}

pub fn press_escape() {
    key(VK_ESCAPE);
}

fn key(vk: VIRTUAL_KEY) {
    let input = |flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, dwFlags: flags, ..Default::default() } },
    };
    unsafe {
        SendInput(&[input(Default::default()), input(KEYEVENTF_KEYUP)], std::mem::size_of::<INPUT>() as i32);
    }
}
