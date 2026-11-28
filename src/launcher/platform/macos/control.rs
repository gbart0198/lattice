//! Command mode's system commands on macOS: not written yet. None are offered.

use crate::launcher::commands::{Snapshot, System};

pub fn snapshot() -> Snapshot {
    super::unsupported("system commands (power, audio, Wi-Fi)");
    Snapshot::default()
}

pub fn run(_cmd: &System) {
    super::unsupported("system commands (power, audio, Wi-Fi)");
}
