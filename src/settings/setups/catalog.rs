//! The programs the add picker offers: the launcher's app catalog (the Start menu's "All apps")
//! with the program each one runs, and the programs with a window open now that the catalog
//! doesn't have. Read once, off the UI thread, the first time the picker opens.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use super::samples::SAMPLE;
use crate::setups;

/// A program to add: what it's called, the file name lattice matches its windows by, and how
/// restoring starts it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Program {
    pub(super) name: String,
    pub(super) exe: String,
    pub(super) run: String,
}

static PROGRAMS: OnceLock<Vec<Program>> = OnceLock::new();
static STARTED: Mutex<bool> = Mutex::new(false);
/// Set when they've been read, until `arrived` takes it.
static FRESH: AtomicBool = AtomicBool::new(false);

/// Whether the programs came in since the last call (the picker redraws with them).
pub(super) fn arrived() -> bool {
    FRESH.swap(false, Ordering::SeqCst)
}

/// The programs, if they've been read; the first call starts reading them (`poll` picks them up).
pub(super) fn programs() -> Option<&'static [Program]> {
    if SAMPLE.load(Ordering::Relaxed) {
        return Some(PROGRAMS.get_or_init(sample));
    }
    if let Some(p) = PROGRAMS.get() {
        return Some(p);
    }
    let mut started = STARTED.lock().unwrap();
    if !*started {
        *started = true;
        std::thread::spawn(|| {
            crate::launcher::com_init();
            let apps = crate::launcher::scan_apps();
            let _ = PROGRAMS.set(merge(catalog(&apps), running()));
            FRESH.store(true, Ordering::SeqCst);
            super::io::wake();
        });
    }
    None
}

/// Catalog apps that run a program, started the way the Start menu starts them.
fn catalog(apps: &[crate::launcher::CatalogApp]) -> Vec<Program> {
    apps.iter()
        .filter_map(|a| {
            let path = crate::launcher::target_exe(&a.id)?;
            let exe = file_name(&path);
            // Packaged apps only start through the shell; others by their path, as a capture
            // writes them.
            let run = if a.id.contains('!') { format!("\"shell:AppsFolder\\{}\"", a.id) } else { setups::run_for_path(&path) };
            Some(Program { name: a.name.to_string(), exe, run })
        })
        .collect()
}

/// Programs with a window lattice would tile, by full path.
fn running() -> Vec<Program> {
    let mut out: Vec<Program> = Vec::new();
    for h in crate::win::enum_windows() {
        let Some(c) = crate::win::candidate(h) else { continue };
        if c.exe.eq_ignore_ascii_case("lattice.exe") || c.exe.eq_ignore_ascii_case("ApplicationFrameHost.exe") {
            continue;
        }
        let path = crate::win::exe_path(h);
        if path.is_empty() || out.iter().any(|p| p.exe.eq_ignore_ascii_case(&c.exe)) {
            continue;
        }
        out.push(Program { name: setups::app_label(&c.exe), exe: c.exe, run: setups::run_for_path(&path) });
    }
    out
}

/// The catalog, with the running programs it doesn't have, by name. One entry a program.
fn merge(catalog: Vec<Program>, running: Vec<Program>) -> Vec<Program> {
    let mut out: Vec<Program> = Vec::new();
    for p in catalog.into_iter().chain(running) {
        if !out.iter().any(|o| o.exe.eq_ignore_ascii_case(&p.exe)) {
            out.push(p);
        }
    }
    out.sort_by_cached_key(|p| p.name.to_lowercase());
    out
}

fn file_name(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_string()
}

/// What previews list: made up, never what's installed.
fn sample() -> Vec<Program> {
    [
        ("Discord", "Discord.exe"),
        ("Figma", "Figma.exe"),
        ("Microsoft Edge", "msedge.exe"),
        ("Notion", "Notion.exe"),
        ("Obsidian", "Obsidian.exe"),
        ("Slack", "slack.exe"),
        ("Spotify", "Spotify.exe"),
        ("Terminal", "WindowsTerminal.exe"),
        ("Visual Studio Code", "Code.exe"),
    ]
    .iter()
    .map(|(name, exe)| Program { name: name.to_string(), exe: exe.to_string(), run: String::new() })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(name: &str, exe: &str) -> Program {
        Program { name: name.into(), exe: exe.into(), run: String::new() }
    }

    #[test]
    fn running_programs_fill_in_what_the_catalog_lacks() {
        let got = merge(vec![p("Visual Studio Code", "Code.exe"), p("Terminal", "WindowsTerminal.exe")], vec![p("code", "code.exe"), p("Postman", "Postman.exe")]);
        let names: Vec<&str> = got.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Postman", "Terminal", "Visual Studio Code"]);
        assert_eq!(file_name(r"C:\Program Files\WindowsApps\Paint\PaintApp\mspaint.exe"), "mspaint.exe");
    }
}
