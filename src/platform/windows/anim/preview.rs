//! `lattice --animation-preview <scene> [fancy|fast]`: plays one animation over demo windows, in
//! a canvas window in the middle of the primary monitor, then exits. Nothing else on the desktop
//! is touched, so it's safe to run next to a live lattice (which ignores lattice's own windows).
//!
//! The demo uses the real animator (live DWM previews, each window moved once), curves, timings
//! and tiling code. The demo windows are popups owned by the canvas, since previews only exist
//! for top-level windows; slides are cut off at the canvas's edge, like a monitor's. The script
//! stands in for the keypresses. Scenes: move, swap, workspace, fullscreen, minimize.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use crate::anim::{After, Animator, Glide, Kind, WARMUP, timing};
use crate::config::AnimStyle;
use crate::layout::{self, LayoutKind, Params, Rect};
use crate::platform::win::Hwnd;

pub const TITLE: &str = "lattice animation preview";
const SCENES: [&str; 5] = ["move", "swap", "workspace", "fullscreen", "minimize"];

/// sRGB hex -> COLORREF (0x00BBGGRR).
const fn rgb(hex: u32) -> COLORREF {
    COLORREF(((hex & 0xff) << 16) | (hex & 0xff00) | ((hex >> 16) & 0xff))
}

const WALL: u32 = 0x11111b;
const BEZEL: u32 = 0x05050a;
const PANEL: u32 = 0x1e1e2e;
const LINE: u32 = 0x313244;
const TEXT: u32 = 0xcdd6f4;
const MUTED: u32 = 0x7f849c;
const ACCENT: u32 = 0x89b4fa;

struct Look {
    name: &'static str,
    color: u32,
    focused: bool,
}

thread_local! {
    /// What each demo window looks like, by handle.
    static LOOKS: RefCell<HashMap<Hwnd, Look>> = RefCell::new(HashMap::new());
    /// The bar's workspace dots: (count, active).
    static BAR: RefCell<(usize, usize)> = const { RefCell::new((2, 0)) };
}

struct DemoWin {
    hwnd: Hwnd,
    ws: usize,
    mon: usize,
    minimized: bool,
}

struct Demo {
    anim: Animator,
    style: AnimStyle,
    canvas: HWND,
    bar: HWND,
    /// Monitors, in canvas coordinates; the bar sits on the first.
    monitors: Vec<Rect>,
    bar_h: i32,
    wins: Vec<DemoWin>,
    active: usize,
    fullscreen: Option<Hwnd>,
}

thread_local! {
    /// The canvas's top-left on screen: demo rects are canvas coordinates.
    static ORIGIN: std::cell::Cell<(i32, i32)> = const { std::cell::Cell::new((0, 0)) };
}

fn to_screen(r: Rect) -> Rect {
    let (x, y) = ORIGIN.get();
    r.offset(x, y)
}

fn place_child(h: Hwnd, r: Rect) {
    let r = to_screen(r);
    unsafe {
        let _ = SetWindowPos(crate::platform::win::h(h), None, r.x, r.y, r.w, r.h, SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

impl Demo {
    fn area(&self, mon: usize) -> Rect {
        let m = self.monitors[mon];
        let bar = if mon == 0 { self.bar_h } else { 0 };
        Rect::new(m.x, m.y + bar, m.w, m.h - bar)
    }

    /// Where each tiled window of the active workspace goes (lattice's `layout_monitor`).
    fn layout(&self, ws: usize) -> Vec<(Hwnd, Rect, usize)> {
        let p = Params { gaps_inner: 8, gaps_outer: 10, ratio: 0.5, splits: &[] };
        let mut out = Vec::new();
        for mon in 0..self.monitors.len() {
            let tiled: Vec<Hwnd> = self.wins.iter().filter(|w| w.ws == ws && w.mon == mon && !w.minimized).map(|w| w.hwnd).collect();
            if let Some(f) = self.fullscreen.filter(|f| tiled.contains(f)) {
                out.push((f, self.area(mon), 0));
                continue;
            }
            for (i, (h, r)) in tiled.iter().zip(layout::tile(LayoutKind::Dwindle, self.area(mon), tiled.len(), &p)).enumerate() {
                out.push((*h, r, i));
            }
        }
        out
    }

    /// Where `h` appears now, in canvas coordinates.
    fn rect_now(&self, h: Hwnd) -> Rect {
        let (x, y) = ORIGIN.get();
        self.anim.current(h).map(|r| r.offset(-x, -y)).unwrap_or_else(|| child_rect(self.canvas, h))
    }

    /// lattice's `retile` + `place_tile`: every tile change glides, each window moved once.
    fn retile(&mut self) {
        let t = timing(self.style, Kind::Move);
        for (h, r, _) in self.layout(self.active) {
            if self.anim.target(h) != Some(to_screen(r)) && self.rect_now(h) != r {
                let from = self.rect_now(h);
                // The animator moves it when its preview lands.
                if !self.anim.glide(h, Glide::moving(to_screen(from), to_screen(r), t)) {
                    place_child(h, r);
                }
            }
        }
        if let Some(f) = self.fullscreen {
            raise(f);
        }
    }

    /// lattice's animated `show_workspace`.
    fn switch(&mut self, ws: usize) {
        let old = self.active;
        self.active = ws;
        BAR.with(|b| b.borrow_mut().1 = ws);
        unsafe {
            let _ = InvalidateRect(Some(self.bar), None, true);
        }
        let down = if ws > old { 1 } else { -1 };
        let slide = timing(self.style, Kind::Slide);
        for (h, target, order) in self.layout(ws) {
            let mon = self.wins.iter().find(|w| w.hwnd == h).unwrap().mon;
            let start = target.offset(0, down * self.monitors[mon].h);
            let clip = Some(to_screen(self.area(mon)));
            self.anim.glide(h, Glide::sliding(to_screen(start), to_screen(target), slide, clip).delay(WARMUP + slide.stagger * order.min(4) as u32));
            place_child(h, target);
            show(h, true);
        }
        for i in 0..self.wins.len() {
            let (h, w_ws, mon) = (self.wins[i].hwnd, self.wins[i].ws, self.wins[i].mon);
            if w_ws == old {
                let from = self.rect_now(h);
                let to = from.offset(0, -down * self.monitors[mon].h);
                let clip = Some(to_screen(self.area(mon)));
                if !self.anim.glide(h, Glide::sliding(to_screen(from), to_screen(to), slide, clip).hiding()) {
                    show(h, false);
                }
            }
        }
    }

    fn focus(&self, h: Hwnd) {
        LOOKS.with(|l| {
            for (k, v) in l.borrow_mut().iter_mut() {
                v.focused = *k == h;
            }
        });
        for w in &self.wins {
            unsafe {
                let _ = InvalidateRect(Some(crate::platform::win::h(w.hwnd)), None, false);
            }
        }
    }

    fn win(&mut self, h: Hwnd) -> &mut DemoWin {
        self.wins.iter_mut().find(|w| w.hwnd == h).unwrap()
    }

    fn swap(&mut self, a: Hwnd, b: Hwnd) {
        let (i, j) = (self.wins.iter().position(|w| w.hwnd == a).unwrap(), self.wins.iter().position(|w| w.hwnd == b).unwrap());
        self.wins.swap(i, j);
        self.retile();
        self.anim.raise(a);
    }

    fn tick(&mut self) {
        for (h, after) in self.anim.tick(Instant::now()) {
            if after == After::Hide {
                show(h, false);
            }
        }
    }
}

fn child_rect(canvas: HWND, h: Hwnd) -> Rect {
    let mut r = RECT::default();
    unsafe {
        let _ = GetWindowRect(crate::platform::win::h(h), &mut r);
        let mut pts = [windows::Win32::Foundation::POINT { x: r.left, y: r.top }, windows::Win32::Foundation::POINT { x: r.right, y: r.bottom }];
        MapWindowPoints(None, Some(canvas), &mut pts);
        Rect::new(pts[0].x, pts[0].y, pts[1].x - pts[0].x, pts[1].y - pts[0].y)
    }
}

fn raise(h: Hwnd) {
    unsafe {
        let _ = SetWindowPos(crate::platform::win::h(h), Some(HWND_TOP), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
}

fn show(h: Hwnd, on: bool) {
    unsafe {
        let _ = ShowWindow(crate::platform::win::h(h), if on { SW_SHOWNA } else { SW_HIDE });
    }
}

fn fill(dc: HDC, r: RECT, color: u32) {
    unsafe {
        let b = CreateSolidBrush(rgb(color));
        FillRect(dc, &r, b);
        let _ = DeleteObject(b.into());
    }
}

fn round_fill(dc: HDC, r: RECT, radius: i32, color: u32) {
    unsafe {
        let rgn = CreateRoundRectRgn(r.left, r.top, r.right + 1, r.bottom + 1, radius, radius);
        let b = CreateSolidBrush(rgb(color));
        let _ = FillRgn(dc, rgn, b);
        let _ = DeleteObject(b.into());
        let _ = DeleteObject(rgn.into());
    }
}

fn font(px: i32, weight: i32) -> HFONT {
    unsafe {
        CreateFontW(px, 0, 0, 0, weight, 0, 0, 0, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY, 0, w!("Segoe UI"))
    }
}

fn text(dc: HDC, s: &str, r: RECT, color: u32, f: HFONT, flags: DRAW_TEXT_FORMAT) {
    unsafe {
        let old = SelectObject(dc, f.into());
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, rgb(color));
        let mut wide: Vec<u16> = s.encode_utf16().collect();
        let mut r = r;
        DrawTextW(dc, &mut wide, &mut r, flags | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);
        SelectObject(dc, old);
    }
}

/// Paints into an offscreen bitmap first, so moving windows never flicker.
fn buffered(hwnd: HWND, paint: impl FnOnce(HDC, RECT)) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let dc = BeginPaint(hwnd, &mut ps);
        let mut r = RECT::default();
        let _ = GetClientRect(hwnd, &mut r);
        let mem = CreateCompatibleDC(Some(dc));
        let bmp = CreateCompatibleBitmap(dc, r.right.max(1), r.bottom.max(1));
        let old = SelectObject(mem, bmp.into());
        paint(mem, r);
        let _ = BitBlt(dc, 0, 0, r.right, r.bottom, Some(mem), 0, 0, SRCCOPY);
        SelectObject(mem, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
        let _ = EndPaint(hwnd, &ps);
    }
}

unsafe extern "system" fn canvas_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mons = MONITORS.with(|m| m.borrow().clone());
            buffered(hwnd, |dc, r| {
                fill(dc, r, BEZEL);
                for m in mons {
                    fill(dc, RECT { left: m.x, top: m.y, right: m.right(), bottom: m.bottom() }, WALL);
                }
            });
            LRESULT(0)
        }
        WM_CLOSE => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

thread_local! {
    static MONITORS: RefCell<Vec<Rect>> = const { RefCell::new(Vec::new()) };
}

unsafe extern "system" fn bar_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let (n, active) = BAR.with(|b| *b.borrow());
            buffered(hwnd, |dc, r| {
                fill(dc, r, PANEL);
                let f = font(15, 600);
                let mut x = 10;
                for i in 0..n {
                    let pill = RECT { left: x, top: 5, right: x + 30, bottom: r.bottom - 5 };
                    round_fill(dc, pill, 10, if i == active { ACCENT } else { LINE });
                    text(dc, &(i + 1).to_string(), pill, if i == active { PANEL } else { TEXT }, f, DT_CENTER);
                    x += 38;
                }
                let clock = RECT { left: r.right - 120, top: 0, right: r.right - 12, bottom: r.bottom };
                text(dc, "Thu 14:32", clock, MUTED, f, DT_RIGHT);
                unsafe {
                    let _ = DeleteObject(f.into());
                }
            });
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

unsafe extern "system" fn demo_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_SIZE => {
            // Rounded corners, like Windows 11 windows.
            let (w, h) = ((lp.0 & 0xffff) as i32, ((lp.0 >> 16) & 0xffff) as i32);
            unsafe {
                let rgn = CreateRoundRectRgn(0, 0, w + 1, h + 1, 14, 14);
                SetWindowRgn(hwnd, Some(rgn), true);
            }
            LRESULT(0)
        }
        WM_PAINT => {
            let look = LOOKS.with(|l| l.borrow().get(&(hwnd.0 as isize)).map(|l| (l.name, l.color, l.focused)));
            buffered(hwnd, |dc, r| {
                let Some((name, color, focused)) = look else { return };
                fill(dc, r, if focused { ACCENT } else { LINE });
                let inner = RECT { left: r.left + 2, top: r.top + 2, right: r.right - 2, bottom: r.bottom - 2 };
                round_fill(dc, inner, 12, PANEL);
                // Title bar: the app's colour badge and name.
                let badge = RECT { left: 14, top: 12, right: 32, bottom: 30 };
                round_fill(dc, badge, 6, color);
                let f = font(16, 600);
                text(dc, name, RECT { left: 42, top: 8, right: r.right - 12, bottom: 34 }, TEXT, f, DT_LEFT);
                // Some content: lines of "text" in the app's colour and greys.
                let mut y = 50;
                let mut i = 0;
                while y + 10 < r.bottom - 12 {
                    let width = [0.72, 0.55, 0.86, 0.4, 0.65][i % 5];
                    let right = 18 + ((r.right - 36) as f32 * width) as i32;
                    round_fill(dc, RECT { left: 18, top: y, right, bottom: y + 9 }, 8, if i % 4 == 0 { color } else { LINE });
                    y += 22;
                    i += 1;
                }
                unsafe {
                    let _ = DeleteObject(f.into());
                }
            });
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

fn register(class: PCWSTR, proc: WNDPROC) {
    unsafe {
        let instance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: proc,
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);
    }
}

/// A popup owned by the canvas (so it stays above it) at `r` in canvas coordinates.
fn child(owner: HWND, class: PCWSTR, r: Rect, visible: bool) -> Hwnd {
    let r = to_screen(r);
    unsafe {
        let style = WS_POPUP | if visible { WS_VISIBLE } else { WINDOW_STYLE(0) };
        CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, class, w!(""), style, r.x, r.y, r.w, r.h, Some(owner), None, None, None)
            .map(|h| h.0 as isize)
            .unwrap_or(0)
    }
}

/// Runs `scene` in `style` (see the module docs). Prints the canvas's screen rect, for recording.
pub fn run(args: &[String]) {
    let after = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).map(String::as_str);
    let scene = after("--animation-preview").unwrap_or("swap");
    if !SCENES.contains(&scene) {
        eprintln!("unknown scene '{scene}'; one of: {}", SCENES.join(", "));
        return;
    }
    let style = if args.iter().any(|a| a == "fast") { AnimStyle::Fast } else { AnimStyle::Fancy };
    let (cw, ch) = after("--size")
        .and_then(|s| s.split_once('x'))
        .and_then(|(a, b)| Some((a.parse::<i32>().ok()?, b.parse::<i32>().ok()?)))
        .unwrap_or((1280, 720));

    let two = scene == "move";
    let monitors = if two {
        let half = (cw - 8) / 2;
        vec![Rect::new(0, 0, half, ch), Rect::new(cw - half, 0, half, ch)]
    } else {
        vec![Rect::new(0, 0, cw, ch)]
    };
    MONITORS.with(|m| *m.borrow_mut() = monitors.clone());

    register(w!("lattice-anim-canvas"), Some(canvas_proc));
    register(w!("lattice-anim-bar"), Some(bar_proc));
    register(w!("lattice-anim-demo"), Some(demo_proc));
    let all = crate::platform::win::monitors();
    let Some(primary) = all.iter().find(|m| m.primary).or(all.first()) else { return };
    let (x, y) = (primary.work.x + (primary.work.w - cw) / 2, primary.work.y + (primary.work.h - ch) / 2);
    let title: Vec<u16> = TITLE.encode_utf16().chain(Some(0)).collect();
    let canvas = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("lattice-anim-canvas"),
            PCWSTR(title.as_ptr()),
            WS_POPUP | WS_CLIPCHILDREN | WS_VISIBLE,
            x,
            y,
            cw,
            ch,
            None,
            None,
            None,
            None,
        )
    };
    let Ok(canvas) = canvas else { return };
    ORIGIN.set((x, y));
    println!("canvas {x} {y} {cw} {ch}");

    let bar_h = 34;
    let bar = child(canvas, w!("lattice-anim-bar"), Rect::new(monitors[0].x, 0, monitors[0].w, bar_h), true);
    let apps: [(&str, u32); 6] =
        [("terminal", 0xa6e3a1), ("browser", 0x89b4fa), ("editor", 0xcba6f7), ("music", 0xf5c2e7), ("notes", 0xf9e2af), ("chat", 0xfab387)];
    // Which workspace and monitor each app starts on, per scene.
    let spots: Vec<(usize, usize)> = match scene {
        "move" => vec![(0, 0), (0, 0), (0, 0), (0, 1)],
        "workspace" => vec![(0, 0), (0, 0), (0, 0), (1, 0), (1, 0), (1, 0)],
        _ => vec![(0, 0), (0, 0), (0, 0)],
    };
    let mut demo = Demo {
        anim: Animator::with(super::Native::default(), || {}),
        style,
        canvas,
        bar: crate::platform::win::h(bar),
        monitors,
        bar_h,
        wins: Vec::new(),
        active: 0,
        fullscreen: None,
    };
    BAR.with(|b| *b.borrow_mut() = (if scene == "workspace" { 4 } else { 3 }, 0));
    for (i, (ws, mon)) in spots.into_iter().enumerate() {
        let h = child(canvas, w!("lattice-anim-demo"), Rect::new(0, 0, 10, 10), ws == 0);
        LOOKS.with(|l| l.borrow_mut().insert(h, Look { name: apps[i].0, color: apps[i].1, focused: false }));
        demo.wins.push(DemoWin { hwnd: h, ws, mon, minimized: false });
    }
    // Lay everything out where it starts, without animating.
    for ws in [1, 0] {
        for (h, r, _) in demo.layout(ws) {
            place_child(h, r);
        }
    }
    raise(bar);
    let ids: Vec<Hwnd> = demo.wins.iter().map(|w| w.hwnd).collect();
    demo.focus(ids[0]);

    // The script: (seconds from start, action). Each scene does a thing and undoes it.
    type Step = Box<dyn Fn(&mut Demo)>;
    let (a, b, c) = (ids[0], ids[1], ids[2]);
    let steps: Vec<(f32, Step)> = match scene {
        "move" => vec![
            (0.8, Box::new(move |d: &mut Demo| {
                d.focus(c);
                d.win(c).mon = 1;
                d.retile();
                d.anim.raise(c);
            })),
            (2.1, Box::new(move |d: &mut Demo| {
                d.win(c).mon = 0;
                d.retile();
                d.anim.raise(c);
            })),
        ],
        "swap" => vec![
            (0.8, Box::new(move |d: &mut Demo| d.swap(a, b))),
            (2.0, Box::new(move |d: &mut Demo| d.swap(a, c))),
        ],
        "workspace" => vec![
            (0.8, Box::new(|d: &mut Demo| d.switch(1))),
            (2.1, Box::new(|d: &mut Demo| d.switch(0))),
        ],
        "fullscreen" => vec![
            (0.8, Box::new(move |d: &mut Demo| {
                d.focus(b);
                d.fullscreen = Some(b);
                d.retile();
            })),
            (2.0, Box::new(|d: &mut Demo| {
                d.fullscreen = None;
                d.retile();
            })),
        ],
        _ => vec![
            (0.8, Box::new(move |d: &mut Demo| {
                // Windows animates the minimize itself; lattice closes the gap around it.
                show(b, false);
                d.win(b).minimized = true;
                d.focus(a);
                d.retile();
            })),
            (2.0, Box::new(move |d: &mut Demo| {
                d.win(b).minimized = false;
                show(b, true);
                d.focus(b);
                d.retile();
            })),
        ],
    };
    let end = steps.last().map_or(0.0, |s| s.0) + 1.3;

    let t0 = Instant::now();
    let mut next = 0;
    let mut msg = MSG::default();
    loop {
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    return;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let t = t0.elapsed().as_secs_f32();
        while next < steps.len() && t >= steps[next].0 {
            (steps[next].1)(&mut demo);
            next += 1;
        }
        demo.tick();
        if t >= end && !demo.anim.busy() {
            break;
        }
        // Pace on DWM's frames, like the real frame clock.
        if unsafe { windows::Win32::Graphics::Dwm::DwmFlush() }.is_err() {
            std::thread::sleep(Duration::from_millis(4));
        }
    }
    unsafe {
        let _ = DestroyWindow(canvas);
    }
}
