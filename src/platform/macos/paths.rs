//! Where lattice keeps its files on macOS: `~/.config/lattice`, like other tiling WMs there
//! (and `$XDG_CONFIG_HOME` when it's set).

use std::path::PathBuf;

pub fn config_base() -> PathBuf {
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME").filter(|x| !x.is_empty()) {
        return PathBuf::from(x);
    }
    home().map(|h| h.join(".config")).unwrap_or_else(|| PathBuf::from("."))
}

pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from)
}
