//! What `--launcher-bench` needs on macOS: not written yet, so the launcher never counts as
//! on screen and the bench reports that it never showed.

pub fn on_screen(_hwnd: isize) -> bool {
    super::unsupported("--launcher-bench");
    false
}

pub fn press_letter(_c: char) {}

pub fn press_escape() {}
