//! Where an installed lattice lives on macOS: nowhere yet (no app bundle or installer), so a
//! macOS build never offers to update itself.

use std::path::PathBuf;

pub fn install_dirs() -> Vec<PathBuf> {
    Vec::new()
}
