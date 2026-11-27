//! lattice: a tiling window manager for Windows 11.
//!
//! Threads: the main thread runs Rune (the top bar and the app launcher); the WM thread owns all
//! window-management state and the Win32 hooks (see `hooks.rs`).

// Release builds have no console window; logs go to %APPDATA%\lattice\lattice.log.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod anim;
mod bar;
mod border;
mod config;
mod events;
mod hooks;
mod icon;
mod ipc;
mod launcher;
mod layout;
mod logger;
mod shared;
mod settings;
mod setups;
mod sysinfo;
mod tour;
mod update;
mod update_toast;
mod win;
mod wm;

use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
use windows::Win32::System::Console::SetConsoleCtrlHandler;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::core::{BOOL, w};

unsafe extern "system" fn on_console_ctrl(_: u32) -> BOOL {
    // Ctrl+C / closing the console: bring hidden windows back before Windows kills us.
    shared::restore_hidden();
    BOOL(0)
}

const HELP: &str = "lattice - tiling window manager for Windows 11

USAGE: lattice [--list | --bar-preview | --settings | --tour | --animation-preview SCENE | --help]
       lattice msg COMMAND | query WHAT | subscribe [EVENT...]

  (no args)       start the window manager and bar
  --list          print monitors and the windows lattice would manage, then exit (changes nothing)
  --bar-preview   show only the bar with sample data (no window management)
  --settings      open the settings window (Win+Alt+Comma)
  --tour          show the guided tour over your desktop (Win+Shift+/); again to end it.
                  lattice shows it by itself once, on its first start. Add --tour-preview to
                  see it over a sample desktop in a window, without lattice running
  --animation-preview SCENE [fast]  play an animation (move, swap, workspace, fullscreen,
                  minimize) over demo windows in the middle of the screen, then exit. Uses the
                  fancy style unless `fast` is given; touches no other window
  --launcher-preview  show the launcher's setup picker over sample data (debug
                  builds: add --snap out.png --scene setups)
  --update-preview  show the update notice with a made-up release (changes nothing)
  --launcher-bench  open and dismiss the launcher on its own and print how long each open and
                  keystroke takes (takes the focus while it runs). --cycles N (8), --gap MS
                  between opens (1000), --settle MS before typing (250)
  --restore       show any windows a killed lattice left hidden, then exit (used by the uninstaller)

Scripting a running lattice:
  msg COMMAND     run a command as a keybind would (`lattice msg workspace 3`,
                  `lattice msg restore-setup Dev` for a setup)
  query WHAT      print JSON: workspaces, windows, monitors, focused, or state (all of them)
  subscribe [EVENT...]  print one JSON line per event (all, or the ones named) until Ctrl+C
                  Events: startup, config-reloaded, workspace-changed, layout-changed,
                  focus-changed, window-opened, window-closed, window-moved, monitor-changed,
                  display-changed. Run commands on them with [[hook]] in the config.
  Exit codes: 0 ok, 1 lattice not running or the request failed, 2 bad arguments

Config: %APPDATA%\\lattice\\config.toml   Log: %APPDATA%\\lattice\\lattice.log";

/// `--list`: what lattice sees, without touching anything.
fn list() {
    let cfg = config::load();
    println!("Monitors:");
    for (i, m) in win::monitors().iter().enumerate() {
        println!(
            "  {i}: {}x{} at ({}, {}), work area {}x{} at ({}, {}), scale {}{}",
            m.rect.w, m.rect.h, m.rect.x, m.rect.y, m.work.w, m.work.h, m.work.x, m.work.y, m.scale,
            if m.primary { ", primary" } else { "" }
        );
    }
    println!("\nWindows lattice would manage:");
    for hwnd in win::enum_windows() {
        let Some(c) = win::candidate(hwnd) else { continue };
        let rule = cfg.rules.iter().filter(|r| r.matches(&c.exe, &c.class, &c.title));
        let (mut ignore, mut float) = (false, cfg.general.float_unresizable && !c.resizable);
        for r in rule {
            ignore |= r.ignore;
            float |= r.float;
        }
        let mode = if ignore { "ignored" } else if float { "floating" } else { "tiled" };
        let ws = match config::rule_workspace(&cfg.rules, &c.exe, &c.class, &c.title) {
            Some(n) if !ignore => format!("ws {n}"),
            _ => String::new(),
        };
        println!("  {hwnd:#010x}  {mode:<8}  {ws:<5} {:<28} {:<32} {}", c.exe, c.class, c.title);
    }
}

fn bar_preview(bar_cfg: config::Bar) {
    // Real icons from system executables, so the preview shows what the bar will.
    let apps = |a: &[&str]| {
        a.iter()
            .map(|s| shared::AppInfo {
                name: (*s).into(),
                icon: icon::for_exe(&format!(r"C:\Windows\{s}.exe"))
                    .or_else(|| icon::for_exe(&format!(r"C:\Windows\System32\{s}.exe"))),
            })
            .collect::<Vec<_>>()
    };
    shared::publish(shared::BarModel {
        workspaces: vec![
            shared::WsInfo { number: 1, visible: true, focused: true, apps: apps(&["explorer", "notepad"]) },
            shared::WsInfo { number: 2, visible: false, focused: false, apps: apps(&["cmd"]) },
            shared::WsInfo { number: 3, visible: false, focused: false, apps: vec![] },
            shared::WsInfo { number: 4, visible: true, focused: false, apps: apps(&["regedit", "mspaint", "taskmgr"]) },
        ],
        title: "lattice - README.md - Visual Studio Code".into(),
        // `--layout-preview master` shows another layout's icon.
        layout: std::env::args()
            .skip_while(|a| a != "--layout-preview")
            .nth(1)
            .unwrap_or_else(|| "dwindle".into()),
    });
    bar::run(bar_cfg);
}

/// The launcher's options from lattice's config: it wears the bar's colours and font.
fn launcher_options(cfg: &config::Config) -> launcher::Options {
    let (bar, fallback) = (&cfg.bar, config::Bar::default());
    let pick = |set: &str, default: &str| bar::color(set, bar::color(default, rune::Color::default()));
    launcher::Options {
        style: launcher::Style {
            background: pick(&bar.background, &fallback.background),
            surface: pick(&bar.occupied, &fallback.occupied),
            foreground: pick(&bar.foreground, &fallback.foreground),
            muted: pick(&bar.muted, &fallback.muted),
            accent: pick(&bar.accent, &fallback.accent),
        },
        families: bar::font_families(&bar.font),
        width: cfg.launcher.width,
        max_results: cfg.launcher.max_results,
        data_dir: config::config_dir(),
        clean_env: vec![CPU_FIRST_ENV.to_string()],
        files: file_options(&cfg.launcher),
        windows: launcher::WindowSearch {
            prefix: cfg.launcher.window_prefix.trim().to_string(),
            list: std::sync::Arc::new(|| shared::send_command(config::Command::ListWindows)),
            focus: std::sync::Arc::new(|hwnd| shared::send_command(config::Command::FocusWindow(hwnd))),
        },
        commands: launcher::CommandOptions {
            prefix: cfg.launcher.command_prefix.trim().to_string(),
            host: launcher_commands(cfg.general.workspaces),
        },
        setups: launcher::SetupOptions {
            keyword: cfg.launcher.setups_keyword.trim().to_string(),
            restore: std::sync::Arc::new(|name| shared::send_command(config::Command::RestoreSetup(name))),
        },
    }
}

/// The window-manager commands command mode offers, each handed to the WM thread as if its
/// keybind were pressed. Commands with an argument get one entry per value, so `> move 3`
/// finds "Move window to workspace 3".
fn launcher_commands(workspaces: usize) -> Vec<launcher::HostCommand> {
    use config::{Command, WsTarget};
    let cmd = |title: String, detail: &str, aliases: &[&str], targets_window: bool, c: Command| launcher::HostCommand {
        title,
        detail: detail.to_string(),
        aliases: aliases.iter().map(|a| a.to_string()).collect(),
        targets_window,
        run: std::sync::Arc::new(move || shared::send_command(c.clone())),
    };
    let mut out = Vec::new();
    for kind in [layout::LayoutKind::Dwindle, layout::LayoutKind::Master, layout::LayoutKind::Monocle] {
        let name = kind.name();
        let title = format!("Layout: {}{}", name[..1].to_uppercase(), &name[1..]);
        out.push(cmd(title, "Tile this workspace with it", &[], false, Command::Layout(Some(kind))));
    }
    out.push(cmd("Toggle floating".into(), "Float the window, or tile it again", &["float"], true, Command::ToggleFloat));
    out.push(cmd("Toggle fullscreen".into(), "Fill the tiling area with the window", &[], true, Command::ToggleFullscreen));
    out.push(cmd("Close window".into(), "Close the focused window", &["kill"], true, Command::Close));
    for n in 1..=workspaces {
        out.push(cmd(format!("Go to workspace {n}"), "Switch workspace", &[], false, Command::Workspace(WsTarget::Index(n - 1))));
    }
    for n in 1..=workspaces {
        let target = WsTarget::Index(n - 1);
        out.push(cmd(format!("Move window to workspace {n}"), "And follow it there", &["send"], true, Command::MoveToWorkspace { target, follow: true }));
    }
    out.push(cmd("Retile".into(), "Re-adopt windows and lay them out again", &[], false, Command::Retile));
    out.push(cmd("Reload config".into(), "Read config.toml again", &[], false, Command::Reload));
    out.push(cmd("Open settings".into(), "lattice's settings window", &["preferences"], false, Command::Settings));
    out.push(cmd("Open config file".into(), "Edit config.toml in your editor", &["edit config"], false, Command::OpenConfig));
    out.push(cmd("Quit lattice".into(), "Show every window and exit", &["exit"], false, Command::Quit));
    out
}

/// File search options; a bad action is reported and falls back to its default.
fn file_options(l: &config::Launcher) -> launcher::FileOptions {
    let defaults = launcher::FileActions::default();
    let action = |key: &str, set: &str, default: &launcher::FileAction| {
        launcher::FileAction::parse(set).unwrap_or_else(|e| {
            log::error!("config [launcher] {key} = \"{set}\": {e}; using the default");
            default.clone()
        })
    };
    let roots: Vec<std::path::PathBuf> = if l.index_roots.is_empty() {
        std::env::var_os("USERPROFILE").map(std::path::PathBuf::from).into_iter().collect()
    } else {
        l.index_roots.iter().map(|r| std::path::PathBuf::from(expand_env(r))).collect()
    };
    launcher::FileOptions {
        prefix: l.file_prefix.trim().to_string(),
        actions: launcher::FileActions {
            file: action("file_enter", &l.file_enter, &defaults.file),
            file_ctrl: action("file_ctrl_enter", &l.file_ctrl_enter, &defaults.file_ctrl),
            dir: action("folder_enter", &l.folder_enter, &defaults.dir),
            dir_ctrl: action("folder_ctrl_enter", &l.folder_ctrl_enter, &defaults.dir_ctrl),
        },
        index_roots: roots,
        index_exclude: l.index_exclude.clone(),
    }
}

/// `%NAME%` environment variables in `s` expanded ("%USERPROFILE%\\Documents"); unknown ones kept.
fn expand_env(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) => match std::env::var(&after[..end]) {
                Ok(v) if end > 0 => {
                    out.push_str(&v);
                    rest = &after[end + 1..];
                }
                _ => {
                    out.push('%');
                    rest = after;
                }
            },
            None => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Rune's switch for drawing on the CPU (see `main`).
pub const CPU_FIRST_ENV: &str = "RUNE_CPU_FIRST";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);
    if !args.is_empty() {
        // Release builds have no console of their own; print into the terminal that ran us.
        unsafe {
            let _ = windows::Win32::System::Console::AttachConsole(windows::Win32::System::Console::ATTACH_PARENT_PROCESS);
        }
    }
    if matches!(args.first().map(String::as_str), Some("msg" | "query" | "subscribe")) {
        // Talks to the running lattice (`ipc.rs`) and exits; touches no log or lock.
        std::process::exit(ipc::cli(&args));
    }
    if has("--settings") {
        // The settings window runs as its own process; it must not touch the WM log or lock.
        settings::run();
        return;
    }
    if has("--tour") {
        // Like settings: its own process, which asks the running lattice what's where.
        tour::run(&args);
        return;
    }
    if has("--help") || has("-h") {
        println!("{HELP}");
        return;
    }
    if has("--restore") {
        shared::recover_hidden_from_last_run();
        return;
    }
    // The bar paints with Rune's CPU renderer and never opens a GPU device unless a frame needs
    // one (Rune then starts it on demand). The device alone (driver, shader compiler, heaps) is
    // ~50 MB of private memory, for a bar that redraws about once a second. Set the variable
    // yourself (e.g. `RUNE_CPU_FIRST=1`) to draw the bar on the GPU again.
    if std::env::var_os(CPU_FIRST_ENV).is_none() {
        // SAFETY: no other thread exists yet. `exec` binds and the settings window remove it
        // again for the programs they start (wm.rs).
        unsafe { std::env::set_var(CPU_FIRST_ENV, "only") };
    }
    logger::init();
    unsafe {
        // Physical pixels everywhere; must happen before any window or monitor call.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    if has("--list") {
        list();
        return;
    }
    if has("--bar-preview") {
        bar_preview(config::load().bar);
        return;
    }
    if has("--update-preview") {
        // The update toast with a made-up release (end the process to close it).
        let opts = launcher_options(&config::load());
        update_toast::preview(opts.style, opts.families);
        return;
    }
    if has("--launcher-bench") {
        // The launcher alone, opened and dismissed `cycles` times (`gap` ms apart), typing `settle` ms
        // after each open; prints timings.
        let num = |flag: &str, default: u64| {
            args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(default)
        };
        let opts = launcher_options(&config::load());
        let ms = std::time::Duration::from_millis;
        launcher::bench(opts, num("--cycles", 8) as usize, ms(num("--gap", 1000)), ms(num("--settle", 250)));
        return;
    }
    if has("--launcher-preview") {
        // The launcher alone over sample setups (`--scene setups`), for screenshots.
        launcher::preview(launcher_options(&config::load()));
        return;
    }
    if has("--animation-preview") {
        anim::preview::run(&args);
        return;
    }
    if has("--border-preview") {
        // Only the focus border, around a sample rectangle (end the process to close it).
        border::preview(config::load().border);
        return;
    }
    unsafe {
        let _guard = CreateMutexW(None, true, w!("Local\\lattice-window-manager"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            log::error!("lattice is already running");
            return;
        }
        let _ = SetConsoleCtrlHandler(Some(on_console_ctrl), true);
        // Keep the mutex for the life of the process.
        std::mem::forget(_guard);
    }

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        shared::restore_hidden();
        log::error!("panic: {info}");
        default_hook(info);
        std::process::exit(1);
    }));

    // Bring back anything a killed or crashed previous run left hidden.
    shared::recover_hidden_from_last_run();
    let cfg = config::load();
    log::info!("config: {}", config::config_path().display());
    let bar_cfg = cfg.bar.clone();
    let launcher_opts = cfg.launcher.enabled.then(|| launcher_options(&cfg));
    let toast_opts = launcher_options(&cfg);

    let wm_thread = std::thread::Builder::new()
        .name("lattice-wm".into())
        .spawn(move || {
            hooks::run(cfg);
            shared::restore_hidden();
            log::info!("bye");
            std::process::exit(0);
        })
        .expect("spawn WM thread");

    // The focus border belongs to the WM thread; Rune runs the bar and the launcher, in one
    // event loop on this thread.
    let mut windows: Vec<rune::AnyWindowSpec> = bar::window(bar_cfg).into_iter().collect();
    if let Some(opts) = launcher_opts {
        windows.push(launcher::window(opts));
    }
    windows.push(update_toast::window(toast_opts.style, toast_opts.families));
    // The first time lattice runs (or the first time since the tour arrived), show it around.
    tour::start_if_new();
    update::start_background();
    if !windows.is_empty() {
        rune::run_windows(move |_displays| windows);
        // The UI only returns if the bar was closed: shut the WM down too.
        shared::send_command(config::Command::Quit);
    }
    let _ = wm_thread.join();
}
