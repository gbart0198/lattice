//! The launcher's app catalog, icons, launching and focus on macOS: not written yet.

use super::unsupported;
use crate::launcher::platform::{App, Icon};

pub fn com_init() {}

pub fn scan() -> Vec<App> {
    unsupported("app catalog");
    Vec::new()
}

pub fn target_exe(_id: &str) -> Option<String> {
    None
}

pub fn icon(_id: &str, _size: u32) -> Option<Icon> {
    None
}

pub fn file_icon(_path: &str, _size: u32) -> Option<Icon> {
    None
}

pub fn launch(_id: &str, _clean_env: &[String]) {
    unsupported("launching apps");
}

pub fn open_path(path: &str, clean_env: &[String]) {
    run_open(&[path], clean_env);
}

pub fn reveal(path: &str, clean_env: &[String]) {
    run_open(&["-R", path], clean_env);
}

fn run_open(args: &[&str], clean_env: &[String]) {
    let mut c = std::process::Command::new("open");
    c.args(args);
    for k in clean_env {
        c.env_remove(k);
    }
    if let Err(e) = c.spawn() {
        log::error!("launcher: open {args:?}: {e}");
    }
}

pub fn exec(line: &str, clean_env: &[String]) {
    let mut c = std::process::Command::new("/bin/sh");
    c.arg("-c").arg(line);
    for k in clean_env {
        c.env_remove(k);
    }
    if let Err(e) = c.spawn() {
        log::error!("launcher: exec '{line}': {e}");
    }
}

pub fn copy_text(_text: &str) {
    unsupported("copying to the clipboard");
}

pub fn bring_to_front(_hwnd: isize) {}

pub fn uncloak(_hwnd: isize) {}

pub fn window_rect(_hwnd: isize) -> Option<(i32, i32, i32, i32)> {
    None
}

pub fn foreground() -> isize {
    0
}

pub fn is_foreground(_hwnd: isize) -> bool {
    false
}

pub fn style_window(_hwnd: isize, _border: [u8; 3]) {}
