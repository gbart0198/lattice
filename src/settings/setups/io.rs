//! The setups outside the editor: checking and writing them into config.toml as `[[setup]]`
//! tables, and talking to the running lattice over IPC, to capture what's open as a new setup
//! and to restore one. Replies come back on threads and are taken up by `poll`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use toml_edit::DocumentMut;

use super::model::{Editor, WsEdit, unique_name};
use crate::config;
use crate::layout::LayoutKind;
use crate::settings::S;
use crate::setups;

// ---------------------------------------------------------------------------------------------
// Saving

/// Problems that keep the setups from being saved.
pub(crate) fn validate(s: &S) -> Result<(), String> {
    let workspaces = s.workspaces.round() as usize;
    let mut names = std::collections::HashSet::new();
    let mut keys: std::collections::HashMap<(u8, u32), String> = std::collections::HashMap::new();
    for b in &s.binds {
        if let Ok(c) = config::parse_chord(b.chord.text.trim()) {
            keys.insert(c, format!("the keybind for \"{}\"", b.command.text.trim()));
        }
    }
    for setup in &s.ed.list {
        let name = setup.name.text.trim();
        if name.is_empty() {
            return Err("Every setup needs a name".into());
        }
        if !names.insert(name.to_lowercase()) {
            return Err(format!("Two setups are called \"{name}\""));
        }
        let key = setup.key.text.trim();
        if !key.is_empty() {
            let chord = config::parse_chord(key).map_err(|e| format!("\"{name}\" shortcut: {e}"))?;
            if let Some(other) = keys.insert(chord, format!("\"{name}\"")) {
                return Err(format!("\"{name}\" shortcut {key} is already {other}"));
            }
        }
        for w in &setup.workspaces {
            if w.apps.is_empty() {
                continue;
            }
            if w.number > workspaces {
                return Err(format!("\"{name}\" uses workspace {}, but there are only {workspaces}", w.number));
            }
            for a in &w.apps {
                let exe = a.exe.text.trim();
                if exe.is_empty() {
                    return Err(format!("\"{name}\": an app on workspace {} has no program", w.number));
                }
                if crate::settings::bad_exe(exe) {
                    return Err(format!("\"{name}\": use the program's file name, like Code.exe (not \"{exe}\")"));
                }
            }
        }
    }
    Ok(())
}

/// Writes the setups as `[[setup]]` tables, replacing the ones there. A one-monitor setup
/// leaves `monitor` out (it goes on the primary); others say how many monitors and which.
pub(crate) fn write(doc: &mut DocumentMut, ed: &Editor) {
    let list = &ed.list;
    doc.remove("setup");
    doc.remove("setups");
    if list.is_empty() {
        return;
    }
    let mut arr = toml_edit::ArrayOfTables::new();
    for (i, s) in list.iter().enumerate() {
        let mut t = toml_edit::Table::new();
        if i == 0 {
            t.decor_mut().set_prefix("\n# Setups: apps, the workspaces they go on, and their layout. Restore one with its key,\n# `lattice msg restore-setup NAME`, or \"workspaces\" in the launcher. The settings\n# window's Setups tab edits them (and rewrites this part of the file).\n");
        }
        t["name"] = toml_edit::value(s.name.text.trim());
        let key = s.key.text.trim();
        if !key.is_empty() {
            t["key"] = toml_edit::value(key);
        }
        if s.monitors > 1 {
            t["monitors"] = toml_edit::value(s.monitors as i64);
        }
        let mut wss = toml_edit::ArrayOfTables::new();
        for w in s.workspaces.iter().filter(|w| !w.apps.is_empty()) {
            let mut wt = toml_edit::Table::new();
            wt["number"] = toml_edit::value(w.number as i64);
            if s.monitors > 1 {
                wt["monitor"] = toml_edit::value(w.monitor as i64 + 1);
            }
            if let Some(l) = w.layout {
                wt["layout"] = toml_edit::value(l.name());
            }
            if !w.splits.is_empty() {
                let splits: toml_edit::Array = w.splits.iter().take(w.apps.len().saturating_sub(1)).map(|r| crate::settings::round2(*r)).collect();
                wt["splits"] = toml_edit::value(splits);
            }
            if let Some(m) = w.master_ratio {
                wt["master_ratio"] = toml_edit::value(crate::settings::round2(m));
            }
            let mut apps = toml_edit::Array::new();
            for a in &w.apps {
                let mut it = toml_edit::InlineTable::new();
                it.insert("exe", a.exe.text.trim().into());
                let run = a.run();
                if !run.is_empty() {
                    it.insert("run", run.into());
                }
                apps.push(it);
            }
            // One app a line.
            for v in apps.iter_mut() {
                v.decor_mut().set_prefix("\n  ");
            }
            apps.set_trailing("\n");
            apps.set_trailing_comma(true);
            wt["apps"] = toml_edit::value(apps);
            wss.push(wt);
        }
        if !wss.is_empty() {
            t["workspace"] = toml_edit::Item::ArrayOfTables(wss);
        }
        arr.push(t);
    }
    doc["setup"] = toml_edit::Item::ArrayOfTables(arr);
}

// ---------------------------------------------------------------------------------------------
// Talking to lattice: restoring, and capturing what's on screen

static CAPTURED: Mutex<Option<Result<Vec<CapturedWs>, String>>> = Mutex::new(None);
static RESTORED: Mutex<Option<Result<String, String>>> = Mutex::new(None);
static GEN: AtomicU64 = AtomicU64::new(0);
static WAKER: OnceLock<rune::Waker> = OnceLock::new();

pub(crate) fn set_waker(w: rune::Waker) {
    let _ = WAKER.set(w);
}

pub(super) fn wake() {
    GEN.fetch_add(1, Ordering::SeqCst);
    if let Some(w) = WAKER.get() {
        w();
    }
}

struct CapturedWs {
    number: usize,
    /// None: lattice didn't say (the primary).
    monitor: Option<usize>,
    layout: Option<LayoutKind>,
    apps: Vec<(String, String)>,
}

/// Asks the running lattice what's open where, off the UI thread (see `poll`).
pub(super) fn capture() {
    std::thread::spawn(|| {
        let result = crate::ipc::request("query", "state").and_then(|v| parse_capture(&v));
        *CAPTURED.lock().unwrap() = Some(result);
        wake();
    });
}

fn parse_capture(v: &serde_json::Value) -> Result<Vec<CapturedWs>, String> {
    let mut out: Vec<CapturedWs> = Vec::new();
    for w in v["windows"].as_array().ok_or("lattice sent no windows")? {
        if w["floating"].as_bool() == Some(true) || w["minimized"].as_bool() == Some(true) {
            continue;
        }
        let (Some(number), Some(exe)) = (w["workspace"].as_u64(), w["exe"].as_str()) else { continue };
        if exe.is_empty() || exe.eq_ignore_ascii_case("ApplicationFrameHost.exe") {
            continue;
        }
        let path = w["hwnd"].as_i64().map(|h| crate::win::exe_path(h as isize)).unwrap_or_default();
        let run = if path.is_empty() { String::new() } else { setups::run_for_path(&path) };
        let number = number as usize;
        let monitor = w["monitor"].as_u64().map(|m| m as usize);
        // Windows come by workspace in tiling order; each monitor of a workspace is an entry.
        let i = match out.iter().position(|c| c.number == number && c.monitor == monitor) {
            Some(i) => i,
            None => {
                let layout = v["workspaces"]
                    .as_array()
                    .and_then(|l| l.iter().find(|x| x["index"].as_u64() == Some(number as u64)))
                    .and_then(|x| x["layout"].as_str())
                    .and_then(LayoutKind::parse);
                out.push(CapturedWs { number, monitor, layout, apps: Vec::new() });
                out.len() - 1
            }
        };
        out[i].apps.push((exe.to_string(), run));
    }
    if out.is_empty() {
        return Err("No tiled windows to save".into());
    }
    Ok(out)
}

/// Saves, then asks the running lattice to restore `name` (the Apply button).
pub(super) fn restore(s: &mut S, name: String) {
    match crate::settings::save(s) {
        Ok(_) => {
            s.status = format!("Applying \u{201c}{name}\u{201d}…");
            s.status_ok = true;
            std::thread::spawn(move || {
                let line = format!("restore-setup {name}");
                let r = crate::ipc::request("msg", &line).map(|_| name);
                *RESTORED.lock().unwrap() = Some(r);
                wake();
            });
        }
        Err(e) => {
            s.status = e;
            s.status_ok = false;
        }
    }
}

/// Takes what came back from lattice; true when something changed.
pub(crate) fn poll(s: &mut S) -> bool {
    let mut changed = super::catalog::arrived();
    if let Some(r) = CAPTURED.lock().unwrap().take() {
        s.ed.capturing = false;
        changed = true;
        match r {
            Ok(wss) => {
                let name = unique_name(&s.ed.list, "Current layout");
                let id = s.ed.id();
                let _ = id;
                let mut setup = s.ed.blank(name.clone());
                for c in wss {
                    let apps = c.apps.iter().map(|(exe, run)| s.ed.app(exe, run)).collect();
                    let monitor = setups::monitor_slot(setup.monitors, c.monitor.map(|m| m + 1), s.ed.primary);
                    setup.workspaces.push(WsEdit { number: c.number, monitor, layout: c.layout, splits: Vec::new(), master_ratio: None, apps });
                }
                s.ed.list.push(setup);
                let i = s.ed.list.len() - 1;
                s.ed.select(i);
                s.status = format!("Saved what's open as \u{201c}{name}\u{201d}; Save to keep it.");
                s.status_ok = true;
            }
            Err(e) => {
                s.status = format!("Couldn't read the open windows: {e}");
                s.status_ok = false;
            }
        }
    }
    if let Some(r) = RESTORED.lock().unwrap().take() {
        changed = true;
        match r {
            Ok(name) => {
                s.status = format!("Applied \u{201c}{name}\u{201d}.");
                s.status_ok = true;
            }
            Err(e) => {
                s.status = format!("Couldn't apply it: {e}");
                s.status_ok = false;
            }
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_groups_windows_by_workspace() {
        let v = serde_json::json!({
            "workspaces": [{"index": 1, "layout": "master"}, {"index": 2, "layout": "dwindle"}],
            "windows": [
                {"hwnd": 0, "exe": "Code.exe", "workspace": 1, "floating": false, "minimized": false},
                {"hwnd": 0, "exe": "WindowsTerminal.exe", "workspace": 1, "floating": false, "minimized": false},
                {"hwnd": 0, "exe": "Calculator.exe", "workspace": 1, "floating": true, "minimized": false},
                {"hwnd": 0, "exe": "msedge.exe", "workspace": 2, "floating": false, "minimized": false},
            ]
        });
        let got = parse_capture(&v).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].layout, Some(LayoutKind::Master));
        assert_eq!(got[0].apps.iter().map(|a| a.0.as_str()).collect::<Vec<_>>(), ["Code.exe", "WindowsTerminal.exe"]);
        assert_eq!(got[1].number, 2);
        assert!(parse_capture(&serde_json::json!({"windows": []})).is_err());
    }

    #[test]
    fn capture_keeps_monitors_apart() {
        let v = serde_json::json!({
            "windows": [
                {"workspace": 1, "monitor": 1, "exe": "Code.exe"},
                {"workspace": 1, "monitor": 0, "exe": "msedge.exe"},
                {"workspace": 1, "monitor": 1, "exe": "WindowsTerminal.exe"},
            ],
            "workspaces": [],
        });
        let got = parse_capture(&v).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!((got[0].monitor, got[0].apps.len()), (Some(1), 2));
        assert_eq!((got[1].monitor, got[1].apps.len()), (Some(0), 1));
    }

}
