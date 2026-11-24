//! lattice: a tiling window manager for Windows 11.
//!
//! Threads: the main thread runs the Rune top bar; the WM thread owns all window-management
//! state and the Win32 hooks (see `hooks.rs`).

// Release builds have no console window; logs go to %APPDATA%\lattice\lattice.log.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bar;
mod border;
mod config;
mod hooks;
mod icon;
mod layout;
mod logger;
mod shared;
mod settings;
mod sysinfo;
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

USAGE: lattice [--list | --bar-preview | --help]

  (no args)       start the window manager and bar
  --list          print monitors and the windows lattice would manage, then exit (changes nothing)
  --bar-preview   show only the bar with sample data (no window management)
  --settings      open the settings window (Win+Shift+Comma)

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
        println!("  {hwnd:#010x}  {mode:<8}  {:<28} {:<32} {}", c.exe, c.class, c.title);
    }
}

fn bar_preview(bar_cfg: config::Bar) {
    // Real icons from system executables, so the preview shows what the bar will.
    let apps = |a: &[&str]| {
        a.iter()
            .map(|s| shared::AppInfo {
                name: s.to_string(),
                icon: icon::for_exe(&format!(r"C:\Windows\{s}.exe"), s.to_string())
                    .or_else(|| icon::for_exe(&format!(r"C:\Windows\System32\{s}.exe"), s.to_string())),
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
    bar::run(bar_cfg, config::Border { enabled: false, ..Default::default() });
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);
    if !args.is_empty() {
        // Release builds have no console of their own; print into the terminal that ran us.
        unsafe {
            let _ = windows::Win32::System::Console::AttachConsole(windows::Win32::System::Console::ATTACH_PARENT_PROCESS);
        }
    }
    if has("--settings") {
        // The settings window runs as its own process; it must not touch the WM log or lock.
        settings::run();
        return;
    }
    if has("--help") || has("-h") {
        println!("{HELP}");
        return;
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
    if has("--border-preview") {
        // Only the focus-border window (for headless snapshots: --window border --snap).
        let cfg = config::load();
        bar::run(config::Bar { enabled: false, ..cfg.bar }, cfg.border);
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
    let border_cfg = cfg.border.clone();

    let wm_thread = std::thread::Builder::new()
        .name("lattice-wm".into())
        .spawn(move || {
            hooks::run(cfg);
            shared::restore_hidden();
            log::info!("bye");
            std::process::exit(0);
        })
        .expect("spawn WM thread");

    if bar_cfg.enabled || border_cfg.enabled {
        bar::run(bar_cfg, border_cfg);
        // The UI only returns if the bar was closed: shut the WM down too.
        shared::send_command(config::Command::Quit);
    }
    let _ = wm_thread.join();
}
