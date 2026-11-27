//! Where windows were, kept across restarts: quitting lattice, an update or a crash would
//! otherwise put every window on the active workspace when it starts again.
//!
//! The WM writes `%APPDATA%\lattice\state.json` whenever its layout changes (workspaces,
//! monitors, tiling order, floating, layouts and split sizes) and on a clean exit. On start it
//! puts the windows that are still open back where they were: matched by handle first (the
//! same window, if lattice only restarted), then by exe and title (an app that reopened its
//! windows). Windows it doesn't recognize go to the active workspace, as before.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::layout::Rect;
use crate::win::Hwnd;

/// Bumped if the format changes incompatibly; a file of another version is ignored.
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Saved {
    pub v: u32,
    /// The workspace on screen (0-based, as everywhere in here).
    pub active: usize,
    /// Monitor rectangles by index, to find each monitor again if they changed.
    pub monitors: Vec<SavedRect>,
    pub workspaces: Vec<SavedWorkspace>,
    /// By workspace, then tiling order.
    pub windows: Vec<SavedWindow>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct SavedRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl From<Rect> for SavedRect {
    fn from(r: Rect) -> Self {
        SavedRect { x: r.x, y: r.y, w: r.w, h: r.h }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SavedWorkspace {
    pub layout: String,
    pub split_ratio: f32,
    pub master_ratio: f32,
    /// Dwindle split ratios by monitor index.
    #[serde(default)]
    pub splits: Vec<(usize, Vec<f32>)>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SavedWindow {
    pub hwnd: Hwnd,
    pub exe: String,
    pub title: String,
    pub ws: usize,
    pub mon: usize,
    pub floating: bool,
    /// The window last focused on its workspace.
    #[serde(default)]
    pub focused: bool,
}

impl Saved {
    pub fn new(active: usize, monitors: Vec<SavedRect>, workspaces: Vec<SavedWorkspace>, windows: Vec<SavedWindow>) -> Self {
        Saved { v: VERSION, active, monitors, workspaces, windows }
    }
}

fn path() -> PathBuf {
    crate::config::config_dir().join("state.json")
}

/// The saved state, if there is a readable one of this version.
pub fn load() -> Option<Saved> {
    let text = std::fs::read_to_string(path()).ok()?;
    match serde_json::from_str::<Saved>(&text) {
        Ok(s) if s.v == VERSION => Some(s),
        Ok(s) => {
            log::info!("state.json is version {}; not restoring from it", s.v);
            None
        }
        Err(e) => {
            log::warn!("state.json: {e}; not restoring from it");
            None
        }
    }
}

/// Writes the state through a temporary file, so a crash mid-write never leaves half a file.
pub fn save(json: &str) -> bool {
    let p = path();
    let tmp = p.with_extension("json.tmp");
    let result = std::fs::create_dir_all(crate::config::config_dir())
        .and_then(|_| std::fs::write(&tmp, json))
        .and_then(|_| std::fs::rename(&tmp, &p));
    if let Err(e) = &result {
        log::warn!("saving {}: {e}", p.display());
    }
    result.is_ok()
}

/// Which saved window each live one was: `live` is (hwnd, exe, title) of the windows about to
/// be managed. Handles match first, across every window, so a window matched by title can't
/// take the entry of another that is the very same window; a handle only counts with the same
/// exe, since Windows reuses handles. Then exe and title, each saved entry used once, in order.
pub fn assign(saved: &[SavedWindow], live: &[(Hwnd, String, String)]) -> HashMap<Hwnd, usize> {
    let mut out = HashMap::new();
    let mut used = vec![false; saved.len()];
    for (h, exe, _) in live {
        if let Some(i) = saved.iter().position(|s| s.hwnd == *h && s.exe.eq_ignore_ascii_case(exe)) {
            used[i] = true;
            out.insert(*h, i);
        }
    }
    for (h, exe, title) in live {
        if out.contains_key(h) {
            continue;
        }
        if let Some(i) = (0..saved.len()).find(|&i| !used[i] && saved[i].exe.eq_ignore_ascii_case(exe) && saved[i].title == *title) {
            used[i] = true;
            out.insert(*h, i);
        }
    }
    out
}

/// Each saved monitor's index now: the monitor with the same rectangle, else (when there are
/// as many monitors as before, but one was resized or moved) the same index, else none.
pub fn remap_monitors(saved: &[SavedRect], now: &[Rect]) -> Vec<Option<usize>> {
    saved
        .iter()
        .enumerate()
        .map(|(i, s)| {
            now.iter()
                .position(|r| SavedRect::from(*r) == *s)
                .or_else(|| (saved.len() == now.len()).then_some(i))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(hwnd: Hwnd, exe: &str, title: &str, ws: usize) -> SavedWindow {
        SavedWindow { hwnd, exe: exe.into(), title: title.into(), ws, mon: 0, floating: false, focused: false }
    }

    fn live(v: &[(Hwnd, &str, &str)]) -> Vec<(Hwnd, String, String)> {
        v.iter().map(|(h, e, t)| (*h, e.to_string(), t.to_string())).collect()
    }

    #[test]
    fn matches_by_handle_then_exe_and_title() {
        let saved = vec![win(10, "code.exe", "a.rs", 2), win(20, "wt.exe", "pwsh", 6), win(30, "wt.exe", "pwsh", 3)];
        // 10 kept its handle (title changed); 21 is a reopened terminal; 99 is new.
        let m = assign(&saved, &live(&[(10, "Code.exe", "b.rs"), (21, "wt.exe", "pwsh"), (99, "x.exe", "y")]));
        assert_eq!(m.get(&10), Some(&0));
        assert_eq!(m.get(&21), Some(&1));
        assert_eq!(m.get(&99), None);
    }

    #[test]
    fn handle_matches_win_over_titles() {
        // 30 comes later in the list but is the same window as saved entry 1: the title match
        // for 21 must not take it.
        let saved = vec![win(20, "wt.exe", "pwsh", 6), win(30, "wt.exe", "pwsh", 3)];
        let m = assign(&saved, &live(&[(21, "wt.exe", "pwsh"), (30, "wt.exe", "pwsh")]));
        assert_eq!(m.get(&30), Some(&1));
        assert_eq!(m.get(&21), Some(&0));
    }

    #[test]
    fn reused_handle_of_another_app_is_not_a_match() {
        let saved = vec![win(10, "code.exe", "a.rs", 2)];
        assert!(assign(&saved, &live(&[(10, "mspaint.exe", "a.rs")])).is_empty());
    }

    #[test]
    fn each_entry_used_once() {
        let saved = vec![win(1, "wt.exe", "pwsh", 4)];
        let m = assign(&saved, &live(&[(5, "wt.exe", "pwsh"), (6, "wt.exe", "pwsh")]));
        assert_eq!(m.len(), 1);
        assert_eq!(m.get(&5), Some(&0));
    }

    #[test]
    fn monitors_remap() {
        let a = Rect::new(0, 0, 1920, 1080);
        let b = Rect::new(-1920, 0, 1920, 1080);
        let saved: Vec<SavedRect> = vec![a.into(), b.into()];
        // Same monitors, enumerated the other way round.
        assert_eq!(remap_monitors(&saved, &[b, a]), vec![Some(1), Some(0)]);
        // One changed resolution: same count, so it keeps its index.
        assert_eq!(remap_monitors(&saved, &[a, Rect::new(-2560, 0, 2560, 1440)]), vec![Some(0), Some(1)]);
        // One unplugged.
        assert_eq!(remap_monitors(&saved, &[a]), vec![Some(0), None]);
    }

    #[test]
    fn round_trips_and_tolerates_missing_fields() {
        let s = Saved::new(
            2,
            vec![Rect::new(0, 0, 10, 10).into()],
            vec![SavedWorkspace { layout: "master".into(), split_ratio: 0.5, master_ratio: 0.6, splits: vec![(0, vec![0.4])] }],
            vec![win(1, "a.exe", "t", 0)],
        );
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Saved>(&json).unwrap(), s);
        let old = r#"{"v":1,"active":0,"monitors":[],"workspaces":[{"layout":"dwindle","split_ratio":0.5,"master_ratio":0.55}],
            "windows":[{"hwnd":1,"exe":"a.exe","title":"t","ws":0,"mon":0,"floating":false}]}"#;
        let parsed = serde_json::from_str::<Saved>(old).unwrap();
        assert!(parsed.workspaces[0].splits.is_empty());
        assert!(!parsed.windows[0].focused);
    }
}
