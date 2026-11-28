//! Where the installer put lattice on Windows, so `update::installed` knows this copy is the one
//! to update.

use std::path::PathBuf;

use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
use windows::core::{HSTRING, w};

/// Inno Setup's uninstall key for the installer's AppId (installer/lattice.iss).
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{6F3A9C1E-2B7D-4E8A-9C5F-1D2E3B4A5C6D}_is1";

/// The folders an installed lattice lives in: the installer's (from its uninstall key), and
/// install.ps1's (the installer upgrades that one in place).
pub fn install_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = install_location().into_iter().collect();
    if let Some(l) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(l).join(r"Programs\lattice"));
    }
    dirs
}

fn install_location() -> Option<PathBuf> {
    let key = HSTRING::from(UNINSTALL_KEY);
    let mut buf = [0u16; 1024];
    let mut len = std::mem::size_of_val(&buf) as u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &key,
            w!("InstallLocation"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut len),
        )
    };
    if rc.is_err() {
        return None;
    }
    let chars = (len as usize / 2).min(buf.len());
    let s = String::from_utf16_lossy(&buf[..chars]);
    let s = s.trim_end_matches('\0');
    (!s.is_empty()).then(|| PathBuf::from(s))
}
