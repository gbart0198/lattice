//! `lattice --launcher-bench`: opens and dismisses the launcher on its own (no window manager,
//! no bar) and prints how long each open takes to reach the screen, and how long a typed key
//! takes to show up. Steals the focus while it runs.
//!
//! Each cycle: toggle → wait until the window is visible and uncloaked (Rune reveals it once
//! its frame is drawn) → type a few letters, timing each until the view rebuilt for it and the
//! window painted, starting `settle` after it showed → Escape (the real dismiss path, `Tx::close_window`) → wait `gap`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY, VK_ESCAPE,
};
use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;

use super::{HWND as LAUNCHER_HWND, Options};

/// View builds and their total time, counted by `ui::view` (always on; two relaxed adds).
pub(super) static VIEWS: AtomicU64 = AtomicU64::new(0);
pub(super) static VIEW_NANOS: AtomicU64 = AtomicU64::new(0);
/// App catalog scans that have landed.
pub(super) static SCANNED: AtomicU64 = AtomicU64::new(0);

pub fn run(opts: Options, cycles: usize, gap: Duration, settle: Duration) {
    let window = super::window(opts);
    std::thread::spawn(move || {
        drive(cycles, gap, settle);
        std::process::exit(0);
    });
    rune::run_windows(move |_| vec![window]);
}

struct Cycle {
    open: Duration,
    open_views: u64,
    open_view_time: Duration,
    keys: Vec<Duration>,
    close: Duration,
}

fn drive(cycles: usize, gap: Duration, settle: Duration) {
    // Let the catalog scan land first, as it does long before anyone presses the hotkey.
    let start = Instant::now();
    while SCANNED.load(Ordering::SeqCst) == 0 && start.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(5));
    }
    println!("catalog ready after {:?}", start.elapsed());
    std::thread::sleep(Duration::from_millis(500));

    let mut results = Vec::new();
    for i in 0..cycles {
        let views0 = VIEWS.load(Ordering::SeqCst);
        let nanos0 = VIEW_NANOS.load(Ordering::SeqCst);
        let t0 = Instant::now();
        super::toggle(None);
        if !wait(Duration::from_secs(5), on_screen) {
            println!("cycle {i}: never showed");
            return;
        }
        let open = t0.elapsed();
        let open_views = VIEWS.load(Ordering::SeqCst) - views0;
        let open_view_time = Duration::from_nanos(VIEW_NANOS.load(Ordering::SeqCst) - nanos0);
        // A person reads the list before typing; `show`'s focus nudge runs meanwhile.
        std::thread::sleep(settle);
        let mut keys = Vec::new();
        for c in ['c', 'h', 'r'] {
            let before = VIEWS.load(Ordering::SeqCst);
            let t = Instant::now();
            key(VIRTUAL_KEY(c.to_ascii_uppercase() as u16));
            wait(Duration::from_secs(2), || VIEWS.load(Ordering::SeqCst) > before);
            keys.push(t.elapsed());
            std::thread::sleep(Duration::from_millis(60));
        }
        std::thread::sleep(Duration::from_millis(200));
        let t = Instant::now();
        // Twice: the first clears the query, the second closes, as a person does.
        key(VK_ESCAPE);
        std::thread::sleep(Duration::from_millis(30));
        key(VK_ESCAPE);
        if !wait(Duration::from_secs(2), || !super::is_visible()) {
            super::hide();
        }
        let close = t.elapsed();
        results.push(Cycle { open, open_views, open_view_time, keys, close });
        std::thread::sleep(gap);
    }

    println!("\n cycle   open(ms)  views  view(ms)   key1   key2   key3  close(ms)");
    for (i, c) in results.iter().enumerate() {
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        println!(
            " {i:>5}  {:>9.1}  {:>5}  {:>8.2}  {:>5.1}  {:>5.1}  {:>5.1}  {:>9.1}",
            ms(c.open),
            c.open_views,
            ms(c.open_view_time),
            ms(c.keys[0]),
            ms(c.keys[1]),
            ms(c.keys[2]),
            ms(c.close)
        );
    }
    let later: Vec<f64> = results.iter().skip(1).map(|c| c.open.as_secs_f64() * 1000.0).collect();
    if !later.is_empty() {
        let mut sorted = later.clone();
        sorted.sort_by(f64::total_cmp);
        println!(
            "\nfirst open {:.1} ms; later opens: median {:.1} ms, min {:.1}, max {:.1}",
            results[0].open.as_secs_f64() * 1000.0,
            sorted[sorted.len() / 2],
            sorted[0],
            sorted[sorted.len() - 1]
        );
    }
}

fn wait(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let t = Instant::now();
    while t.elapsed() < limit {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_micros(250));
    }
    false
}

/// Shown and not cloaked by DWM: on screen.
fn on_screen() -> bool {
    let hwnd = LAUNCHER_HWND.load(Ordering::SeqCst);
    if hwnd == 0 {
        return false;
    }
    let hwnd = HWND(hwnd as *mut _);
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            return false;
        }
        let mut cloaked = 0u32;
        let ok = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, 4).is_ok();
        ok && cloaked == 0
    }
}

fn key(vk: VIRTUAL_KEY) {
    let input = |flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, dwFlags: flags, ..Default::default() } },
    };
    unsafe {
        SendInput(&[input(Default::default()), input(KEYEVENTF_KEYUP)], std::mem::size_of::<INPUT>() as i32);
    }
}
