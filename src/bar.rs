//! The top bar, drawn with Rune: workspaces (with the apps on each), the focused window's
//! title, and CPU / memory / battery / clock. It runs on the main thread; the WM thread
//! publishes a `BarModel` and wakes it, and clicks on a workspace go back as commands.

use std::sync::atomic::{AtomicU64, Ordering};
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use rune::{Align, App, AppSpec, Color, Cx, El, Image, ImageCache, SurfaceFit, TextStyle, Tx, WindowOptions, WindowsWindowOptions, col, el, row, text};

use crate::config::{self, Command, WsTarget};
use crate::shared::{self, BAR_GEN, BarModel};
use crate::sysinfo::{Sampler, SysInfo};
use crate::win;

struct Theme {
    bg: Color,
    fg: Color,
    muted: Color,
    accent: Color,
    occupied: Color,
}

static CFG: OnceLock<config::Bar> = OnceLock::new();
static THEME: OnceLock<Theme> = OnceLock::new();
static SYS: Mutex<Option<SysInfo>> = Mutex::new(None);
static SYS_GEN: AtomicU64 = AtomicU64::new(0);

struct State {
    model: BarModel,
    sys: SysInfo,
    model_gen: u64,
    sys_gen: u64,
    /// Decodes app icons off-thread and wakes the bar when they're ready.
    cache: Option<ImageCache>,
    /// App icon images by `AppIcon::key`, and the keys whose pixels have arrived.
    icons: HashMap<u64, Image>,
    icons_ready: HashSet<u64>,
}

/// Rune's (sans, mono) family lists for a configured font name: the font first, then the
/// built-in Inter if it's missing. Empty means Inter alone.
pub fn font_families(font: &str) -> (&'static str, &'static str) {
    let font = font.trim();
    if font.is_empty() {
        return ("Inter", "ui-monospace");
    }
    (font_list(font), "ui-monospace")
}

/// `"Name", Inter` as a `'static` CSS family list. Names are leaked once each (a handful per
/// process at most), since Rune's family slots take `&'static str`.
pub fn font_list(font: &str) -> &'static str {
    static CACHE: Mutex<Option<std::collections::HashMap<String, &'static str>>> = Mutex::new(None);
    let mut g = CACHE.lock().unwrap();
    let map = g.get_or_insert_with(Default::default);
    if let Some(s) = map.get(font) {
        return s;
    }
    let quoted = format!("\"{}\", Inter", font.replace('\\', "\\\\").replace('"', "\\\""));
    let leaked: &'static str = Box::leak(quoted.into_boxed_str());
    map.insert(font.to_string(), leaked);
    leaked
}

pub fn color(s: &str, fallback: Color) -> Color {
    let Some(h) = s.strip_prefix('#') else { return fallback };
    let byte = |i: usize| h.get(i..i + 2).and_then(|x| u8::from_str_radix(x, 16).ok());
    let (Some(r), Some(g), Some(b)) = (byte(0), byte(2), byte(4)) else { return fallback };
    let a = byte(6).map(|a| a as f32 / 255.0).unwrap_or(1.0);
    // Config colours are sRGB hex; Rune works in linear sRGB.
    let lin = |v: u8| {
        let c = v as f32 / 255.0;
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    Color { r: lin(r), g: lin(g), b: lin(b), a }
}

fn cfg() -> &'static config::Bar {
    CFG.get().expect("bar config set")
}

fn theme() -> &'static Theme {
    THEME.get().expect("bar theme set")
}

fn init() -> State {
    State {
        model: shared::bar_model(),
        sys: SysInfo::default(),
        model_gen: u64::MAX,
        sys_gen: u64::MAX,
        cache: None,
        icons: HashMap::new(),
        icons_ready: HashSet::new(),
    }
}

fn setup(s: &mut State, wake: rune::Waker) {
    shared::set_bar_waker(wake.clone());
    s.cache = Some(ImageCache::new(wake.clone()));
    std::thread::Builder::new()
        .name("lattice-sysinfo".into())
        .spawn(move || {
            let mut sampler = Sampler::new();
            loop {
                let info = sampler.sample(&cfg().clock_format);
                let mut slot = SYS.lock().unwrap();
                if slot.as_ref() != Some(&info) {
                    *slot = Some(info);
                    drop(slot);
                    SYS_GEN.fetch_add(1, Ordering::SeqCst);
                    wake();
                }
                std::thread::sleep(Duration::from_millis(1000));
            }
        })
        .expect("spawn sysinfo thread");
}

fn poll(s: &mut State, _t: f64, _tx: &mut Tx) -> bool {
    let mut changed = false;
    let g = BAR_GEN.load(Ordering::SeqCst);
    if g != s.model_gen {
        s.model_gen = g;
        s.model = shared::bar_model();
        sync_icons(s);
        changed = true;
    }
    // Icons decode off-thread; rebuild once each one lands.
    for (key, img) in &s.icons {
        if img.size().is_some() && s.icons_ready.insert(*key) {
            changed = true;
        }
    }
    let g = SYS_GEN.load(Ordering::SeqCst);
    if g != s.sys_gen {
        s.sys_gen = g;
        if let Some(info) = SYS.lock().unwrap().clone() {
            s.sys = info;
        }
        changed = true;
    }
    changed
}

/// Starts decoding icons new to the model and forgets ones no window uses any more.
fn sync_icons(s: &mut State) {
    let Some(cache) = &s.cache else { return };
    let mut live = HashSet::new();
    for app in s.model.workspaces.iter().flat_map(|w| &w.apps) {
        if let Some(icon) = &app.icon {
            live.insert(icon.key);
            s.icons
                .entry(icon.key)
                .or_insert_with(|| cache.rgba(crate::icon::SIZE, crate::icon::SIZE, icon.rgba.clone()));
        }
    }
    s.icons.retain(|k, _| live.contains(k));
    s.icons_ready.retain(|k| live.contains(k));
}

/// One app on a workspace pill: its icon, its name, or both (`bar.app_display`).
fn app_chip(s: &State, app: &shared::AppInfo, color: Color) -> El {
    let c = cfg();
    let size = c.font_size + 3.0;
    let icon = app
        .icon
        .as_ref()
        .and_then(|i| s.icons.get(&i.key))
        .filter(|img| img.size().is_some())
        .map(|img| el().size(size, size).pixels_fit(img.pixels(), SurfaceFit::Fit));
    let name = || text(app.name.to_string()).max_w(120.0).truncate().font(TextStyle::new(c.font_size - 1.0, 500.0)).color(color);
    match (c.app_display.as_str(), icon) {
        ("names", _) | (_, None) => name(),
        ("both", Some(icon)) => row().gap(4.0).center_y().child(icon).child(name()),
        (_, Some(icon)) => icon.label(app.name.to_string()),
    }
}

fn workspace_pill(s: &State, ws: &shared::WsInfo, height: f32) -> El {
    let t = theme();
    let c = cfg();
    let fill = if ws.focused {
        t.accent
    } else if ws.visible {
        t.occupied.mix(t.accent, 0.25)
    } else {
        t.occupied
    };
    let (num_color, app_color) = if ws.focused {
        (t.bg, t.bg)
    } else if ws.visible {
        (t.fg, t.fg)
    } else {
        (t.fg, t.muted)
    };
    let n = ws.number;
    let mut pill = row()
        .key(format!("ws-{n}"))
        .role("button")
        .label(format!("Workspace {n}"))
        .hit()
        .h(height)
        .px(9.0)
        .gap(6.0)
        .center_y()
        .radius(height / 2.0)
        .fill(fill)
        .on_press(move |_s: &mut State| shared::send_command(Command::Workspace(WsTarget::Index(n - 1))))
        .child(text(n.to_string()).font(TextStyle::new(c.font_size, 700.0)).color(num_color));
    if !ws.apps.is_empty() {
        let gap = if c.app_display == "icons" { 5.0 } else { 8.0 };
        let apps = ws.apps.iter().fold(row().gap(gap).center_y(), |r, a| r.child(app_chip(s, a, app_color)));
        pill = pill.child(apps);
    }
    pill
}

/// A small diagram of the active workspace's layout; clicking it cycles to the next one
/// (same as `layout next`).
fn layout_button(layout: &str, height: f32) -> El {
    let t = theme();
    let (w, h) = (17.0_f32, 12.0_f32);
    let gap = 1.5;
    let line = 1.2;
    // Tiles as (x, y, width, height) within a w x h box.
    let tiles: Vec<(f32, f32, f32, f32)> = match layout {
        "master" => {
            let mw = (w - gap) * 0.55;
            let sx = mw + gap;
            let sh = (h - gap) / 2.0;
            vec![(0.0, 0.0, mw, h), (sx, 0.0, w - sx, sh), (sx, sh + gap, w - sx, sh)]
        }
        "monocle" => vec![(0.0, 0.0, w, h)],
        // dwindle: half, then the right half split, then its bottom split again.
        _ => {
            let hw = (w - gap) / 2.0;
            let rx = hw + gap;
            let th = (h - gap) / 2.0;
            let bw = (w - rx - gap) / 2.0;
            vec![
                (0.0, 0.0, hw, h),
                (rx, 0.0, w - rx, th),
                (rx, th + gap, bw, th),
                (rx + bw + gap, th + gap, bw, th),
            ]
        }
    };
    let diagram = tiles.into_iter().fold(el().size(w, h), |d, (x, y, tw, th)| {
        d.child(el().absolute(x, y).size(tw, th).radius(1.5).border(line, t.fg))
    });
    row()
        .key("layout")
        .role("button")
        .label(format!("Layout: {layout} (click for next)"))
        .hit()
        .h(height)
        .px(7.0)
        .center()
        .radius(height / 2.0)
        .fill(t.occupied)
        .on_press(|_s: &mut State| shared::send_command(Command::Layout(None)))
        .child(diagram)
}

fn stat(label: &str, value: String) -> El {
    let t = theme();
    let size = cfg().font_size;
    row()
        .gap(4.0)
        .center_y()
        .child(text(label.to_string()).font(TextStyle::new(size - 1.0, 500.0)).color(t.muted))
        .child(text(value).font(TextStyle::new(size, 600.0).features("'tnum' 1")).color(t.fg))
}

fn view(s: &State, _cx: &mut Cx<State>) -> El {
    let t = theme();
    let c = cfg();
    let pill_h = (c.height - 8.0).max(14.0);

    let mut spaces = row().gap(6.0).center_y();
    for ws in &s.model.workspaces {
        if ws.visible || !ws.apps.is_empty() || c.show_empty_workspaces {
            spaces = spaces.child(workspace_pill(s, ws, pill_h));
        }
    }
    let title = if c.show_title {
        text(s.model.title.clone())
            .w_full()
            .align(Align::Center)
            .truncate()
            .font(TextStyle::new(c.font_size, 500.0))
            .color(t.fg)
    } else {
        text(String::new())
    };
    let middle = row().grow(1.0).shrink().center_y().px(12.0).child(title);

    let mut right = row().gap(14.0).center_y().child(layout_button(&s.model.layout, pill_h));
    if c.show_cpu {
        right = right.child(stat("CPU", format!("{:>2}%", s.sys.cpu)));
    }
    if c.show_memory {
        right = right.child(stat("MEM", format!("{:>2}%", s.sys.mem)));
    }
    if c.show_battery
        && let Some((pct, charging)) = s.sys.battery
    {
        right = right.child(stat(if charging { "AC" } else { "BAT" }, format!("{pct}%")));
    }
    right = right.child(text(s.sys.clock.clone()).font(TextStyle::new(c.font_size, 650.0).features("'tnum' 1")).color(t.fg));

    let left = row().gap(10.0).center_y().child(spaces);
    col().w_full().h_full().fill(t.bg).child(
        row().w_full().h_full().px(8.0).gap(8.0).center_y().child(left).child(middle).child(right),
    )
}

/// Runs the bar on the calling thread until the bar quits.
pub fn run(bar_cfg: config::Bar) {
    let Some(spec) = window(bar_cfg) else { return };
    let windows: Vec<rune::AnyWindowSpec> = vec![spec];
    rune::run_windows(move |_displays| windows);
}

/// The bar's window, for a Rune event loop shared with other windows (the launcher). None when
/// the bar is turned off or there's no monitor. Closing it quits that loop.
pub fn window(bar_cfg: config::Bar) -> Option<rune::AnyWindowSpec> {
    if !bar_cfg.enabled {
        return None;
    }
    bar_spec(bar_cfg).map(|spec| spec.boxed())
}

fn bar_spec(bar_cfg: config::Bar) -> Option<rune::WindowSpec<State>> {
    let fallback = config::Bar::default();
    let _ = THEME.set(Theme {
        bg: color(&bar_cfg.background, color(&fallback.background, Color::default())),
        fg: color(&bar_cfg.foreground, color(&fallback.foreground, Color::default())),
        muted: color(&bar_cfg.muted, color(&fallback.muted, Color::default())),
        accent: color(&bar_cfg.accent, color(&fallback.accent, Color::default())),
        occupied: color(&bar_cfg.occupied, color(&fallback.occupied, Color::default())),
    });
    let height = bar_cfg.height;
    let families = font_families(&bar_cfg.font);
    let _ = CFG.set(bar_cfg);

    let monitors = win::monitors();
    let Some(primary) = monitors.iter().find(|m| m.primary).or(monitors.first()).cloned() else {
        log::error!("no monitors found; bar disabled");
        return None;
    };
    let work = primary.work;
    let phys_h = (height * primary.scale).round() as u32;
    log::info!("bar at ({}, {}) {}x{} (scale {})", work.x, work.y, work.w, phys_h, primary.scale);

    let options = WindowOptions {
        borderless: true,
        always_on_top: true,
        resizable: false,
        position: Some((work.x, work.y)),
        physical_size: Some((work.w as u32, phys_h)),
        min_size: Some((0.0, 0.0)),
        ..Default::default()
    };
    let app = App::new(AppSpec::<State> {
        setup: Some(setup),
        poll: Some(poll),
        min_size: Some((1.0, 1.0)),
        families,
        ..AppSpec::new("lattice-bar", (work.w as f32 / primary.scale, height), init, view)
    })
    .window_options(options)
    .windows_window(WindowsWindowOptions::default().tool_window(true).no_activate(true))
    .open_inactive()
    // Closing the bar quits lattice, and Rune may end the process right after, or on
    // logoff/shutdown: put every window lattice hid back on screen first.
    .on_close(|_, tx| {
        shared::restore_hidden();
        tx.quit();
    })
    .on_quit(|_, _| shared::restore_hidden());
    Some(rune::WindowSpec::from_app("bar", app))
}
