//! Where lattice keeps its files on Windows.

use std::path::PathBuf;

/// The folder lattice's own folder goes in: %APPDATA% (config, state, logs).
pub fn config_base() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

/// The user's home folder (%USERPROFILE%).
pub fn home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").filter(|h| !h.is_empty()).map(PathBuf::from)
}
