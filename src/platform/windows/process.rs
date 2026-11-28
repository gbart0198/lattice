//! Processes on Windows: starting programs for the user, one instance of each lattice process,
//! the console a terminal gave us, and DPI awareness.

use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;

use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole, SetConsoleCtrlHandler};
use windows::Win32::System::Threading::{CreateEventW, CreateMutexW, SetEvent, WaitForSingleObject};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow, FindWindowW, SW_RESTORE, SW_SHOWNORMAL, SetForegroundWindow, ShowWindow};
use windows::core::{BOOL, HSTRING, w};

/// Physical pixels everywhere; must happen before any window or monitor call.
pub fn set_dpi_aware() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

/// Release builds have no console of their own; print into the terminal that ran us.
pub fn attach_parent_console() {
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

unsafe extern "system" fn on_console_ctrl(_: u32) -> BOOL {
    // Ctrl+C / closing the console: bring hidden windows back before Windows kills us.
    crate::shared::restore_hidden();
    BOOL(0)
}

/// Shows every hidden window again if the console we run in is closed or gets Ctrl+C.
pub fn restore_hidden_on_console_close() {
    unsafe {
        let _ = SetConsoleCtrlHandler(Some(on_console_ctrl), true);
    }
}

/// Holds a single-instance lock for as long as it lives (or forever, `std::mem::forget`).
pub struct InstanceLock(#[allow(dead_code)] windows::core::Result<HANDLE>);

/// The lock called `name` (one per session), or None when another process holds it.
pub fn single_instance(name: &str) -> Option<InstanceLock> {
    unsafe {
        let guard = CreateMutexW(None, true, &HSTRING::from(format!("Local\\{name}")));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return None;
        }
        Some(InstanceLock(guard))
    }
}

/// A named flag one process sets and another sees (a manual-reset event).
pub struct Signal(HANDLE);

// SAFETY: an event handle may be set and waited on from any thread.
unsafe impl Send for Signal {}
unsafe impl Sync for Signal {}

impl Signal {
    /// The signal called `name` (one per session), made if it doesn't exist yet.
    pub fn open(name: &str) -> Option<Signal> {
        unsafe { CreateEventW(None, true, false, &HSTRING::from(format!("Local\\{name}"))) }.ok().map(Signal)
    }

    pub fn set(&self) {
        unsafe {
            let _ = SetEvent(self.0);
        }
    }

    pub fn is_set(&self) -> bool {
        unsafe { WaitForSingleObject(self.0, 0) }.0 == 0
    }
}

/// Restores and focuses the top-level window titled `title` (another instance of ours).
pub fn bring_to_front(title: &str) {
    unsafe {
        if let Ok(h) = FindWindowW(None, &HSTRING::from(title))
            && !h.is_invalid()
        {
            let _ = ShowWindow(h, SW_RESTORE);
            let _ = SetForegroundWindow(h);
        }
    }
}

/// We just handled the user's key press, so we may hand the foreground on: lets a window we
/// start (or one it activates) come to the front instead of flashing.
pub fn allow_set_foreground() {
    unsafe {
        let _ = AllowSetForegroundWindow(ASFW_ANY);
    }
}

/// Keeps a console program from opening a console window of its own.
pub fn no_console(c: &mut Command) {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    c.creation_flags(CREATE_NO_WINDOW);
}

/// `exec`: a command line run the way the Run dialog would (`start` resolves App Paths such
/// as wt.exe or code, and URIs), from `base` (`wm::background_command` of the shell).
pub fn shell_start(base: impl FnOnce(&str) -> Command, line: &str) -> Command {
    let mut c = base("cmd");
    c.raw_arg(format!("/C start \"\" {line}"));
    c
}

/// A `[[hook]]` command line run through the shell. Console programs share its hidden
/// console, so nothing flashes. `/S` with the line in quotes: cmd takes everything inside them
/// literally.
pub fn shell_run(base: impl FnOnce(&str) -> Command, line: &str) -> Command {
    let mut c = base("cmd");
    c.raw_arg(format!("/D /S /C \"{line}\""));
    c
}

/// Opens a file with whatever is registered for its type, else Notepad.
pub fn open_file(path: &Path) {
    let ok = unsafe { ShellExecuteW(None, w!("open"), &HSTRING::from(path.as_os_str()), None, None, SW_SHOWNORMAL) }.0 as isize > 32;
    if !ok && let Err(e) = Command::new("notepad.exe").arg(path).spawn() {
        log::error!("open {}: {e}", path.display());
    }
}
