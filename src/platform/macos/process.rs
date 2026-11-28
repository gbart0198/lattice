//! Processes on macOS: starting programs works (through `sh` and `open`); a single-instance
//! lock is a locked file. Signals between processes aren't written yet.

use std::path::Path;
use std::process::Command;

pub fn set_dpi_aware() {}

pub fn attach_parent_console() {}

pub fn restore_hidden_on_console_close() {}

/// Holds a single-instance lock for as long as it lives (or forever, `std::mem::forget`).
pub struct InstanceLock(#[allow(dead_code)] Option<std::fs::File>);

/// The lock called `name`, or None when another process holds it.
pub fn single_instance(name: &str) -> Option<InstanceLock> {
    let path = std::env::temp_dir().join(format!("{name}.lock"));
    let file = match std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&path) {
        Ok(f) => f,
        Err(e) => {
            // No lock to be had: carry on rather than refuse to start.
            log::warn!("single-instance lock {}: {e}", path.display());
            return Some(InstanceLock(None));
        }
    };
    match file.try_lock() {
        Ok(()) => Some(InstanceLock(Some(file))),
        Err(_) => None,
    }
}

/// A named flag one process sets and another sees: not written yet, never set.
pub struct Signal;

impl Signal {
    pub fn open(_name: &str) -> Option<Signal> {
        super::unsupported("signals between lattice processes");
        None
    }

    pub fn set(&self) {}

    pub fn is_set(&self) -> bool {
        false
    }
}

pub fn bring_to_front(_title: &str) {
    super::unsupported("bringing a window to the front");
}

pub fn allow_set_foreground() {}

pub fn no_console(_: &mut Command) {}

/// `exec`: a command line run by the shell.
pub fn shell_start(base: impl FnOnce(&str) -> Command, line: &str) -> Command {
    let mut c = base("/bin/sh");
    c.arg("-c").arg(line);
    c
}

/// A `[[hook]]` command line run by the shell.
pub fn shell_run(base: impl FnOnce(&str) -> Command, line: &str) -> Command {
    shell_start(base, line)
}

/// Opens a file with its default app.
pub fn open_file(path: &Path) {
    if let Err(e) = Command::new("open").arg(path).spawn() {
        log::error!("open {}: {e}", path.display());
    }
}
