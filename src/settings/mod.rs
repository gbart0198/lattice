//! The settings window (`lattice.exe --settings`, Win+Alt+Comma): a Rune app that edits
//! config.toml in place, keeping its comments, then tells the running lattice to reload.
//!
//! It runs as its own process. Lattice never manages lattice.exe windows, so this one floats
//! above everything, untiled, and stays on screen across workspace switches.

use rune::tok::{radius, space, type_};
use rune::{App, AppSpec, Color, ColorScheme, Cx, El, ScrollSpec, TextEdit, TextStyle, Tx, WindowOptions, col, el, row, text};
use rune_kit::{Button, ButtonVariant, Kit, SegmentedControl, Slider, Switch, TextInput};
use toml_edit::DocumentMut;

use crate::config::{self, Config};
use crate::update;
use crate::win;

mod setups_tab;

const TITLE: &str = "lattice settings";
const TABS: [&str; 6] = ["General", "Apps", "Setups", "Bar", "Border", "Keybinds"];
const LAYOUTS: [&str; 3] = ["dwindle", "master", "monocle"];
const NEW_WINDOW: [&str; 2] = ["after_focused", "end"];
const APP_DISPLAY: [&str; 3] = ["icons", "names", "both"];
const ANIM_STYLES: [&str; 2] = ["fancy", "fast"];

struct Bind {
    id: u64,
    chord: TextEdit,
    command: TextEdit,
}

/// An app->workspace rule on the Apps tab.
struct Assign {
    id: u64,
    /// Which of the config's assignment rules (see `assignment`) this row was loaded from;
    /// `None` for rows added here.
    origin: Option<usize>,
    exe: TextEdit,
    title: TextEdit,
    /// 1-based.
    ws: usize,
}

impl Assign {
    fn blank(&self) -> bool {
        self.exe.text.trim().is_empty() && self.title.text.trim().is_empty()
    }
}

struct S {
    tab: usize,
    // [general]
    layout: usize,
    workspaces: f32,
    gaps_inner: f32,
    gaps_outer: f32,
    split_ratio: f32,
    master_ratio: f32,
    new_window: usize,
    float_unresizable: bool,
    follow_activation: bool,
    mouse_follows_focus: bool,
    check_updates: bool,
    // [animations]
    anim_enabled: bool,
    anim_style: usize,
    /// Rules the Apps tab doesn't edit (float, ignore, ...).
    other_rules: usize,
    // [[rules]] that only pin an app to a workspace
    apps: Vec<Assign>,
    // [bar]
    bar_enabled: bool,
    bar_height: f32,
    font_size: f32,
    /// Bar font family; empty = built-in Inter.
    font: String,
    font_picker: rune_kit::ComboboxState,
    clock_format: TextEdit,
    show_title: bool,
    show_cpu: bool,
    show_memory: bool,
    show_battery: bool,
    show_empty: bool,
    app_display: usize,
    background: TextEdit,
    foreground: TextEdit,
    muted: TextEdit,
    accent: TextEdit,
    occupied: TextEdit,
    // [border]
    border_enabled: bool,
    border_width: f32,
    border_radius: f32,
    border_color: TextEdit,
    glow: f32,
    glow_opacity: f32,
    // [keybinds]
    binds: Vec<Bind>,
    next_id: u64,
    // [[setup]]: setups
    ed: setups_tab::Editor,
    // UI
    update: update::Status,
    update_gen: u64,
    status: String,
    status_ok: bool,
    restart_needed: bool,
    /// Bar + border values as last saved, to tell when a restart is needed.
    saved_ui: String,
}

fn idx(list: &[&str], v: &str) -> usize {
    list.iter().position(|x| *x == v).unwrap_or(0)
}

fn init() -> S {
    let cfg: Config = config::load();
    let g = &cfg.general;
    let b = &cfg.bar;
    let br = &cfg.border;
    // Keybinds in the file's own order (Config holds them sorted).
    let mut binds = Vec::new();
    let doc = std::fs::read_to_string(config::config_path()).ok().and_then(|t| t.parse::<DocumentMut>().ok());
    if let Some(table) = doc.as_ref().and_then(|d| d.get("keybinds")).and_then(|t| t.as_table()) {
        for (k, v) in table.iter() {
            binds.push((k.to_string(), v.as_str().unwrap_or_default().to_string()));
        }
    } else {
        binds = cfg.keybinds.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    }
    let apps: Vec<Assign> = doc
        .as_ref()
        .and_then(|d| d.get("rules"))
        .and_then(|r| r.as_array_of_tables())
        .map(|a| a.iter().filter_map(assignment).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(i, (exe, title, ws))| Assign {
            id: (binds.len() + i) as u64,
            origin: Some(i),
            exe: TextEdit::new(exe),
            title: TextEdit::new(title),
            ws,
        })
        .collect();
    let n = (binds.len() + apps.len()) as u64;
    let mut s = S {
        tab: 0,
        layout: idx(&LAYOUTS, g.layout.name()),
        workspaces: g.workspaces as f32,
        gaps_inner: g.gaps_inner as f32,
        gaps_outer: g.gaps_outer as f32,
        split_ratio: g.split_ratio,
        master_ratio: g.master_ratio,
        new_window: if g.new_window == config::NewWindow::End { 1 } else { 0 },
        float_unresizable: g.float_unresizable,
        follow_activation: g.follow_activation,
        mouse_follows_focus: g.mouse_follows_focus,
        check_updates: g.check_for_updates,
        anim_enabled: cfg.animations.enabled,
        anim_style: if cfg.animations.style == config::AnimStyle::Fast { 1 } else { 0 },
        other_rules: cfg.rules.len().saturating_sub(apps.len()),
        apps,
        bar_enabled: b.enabled,
        bar_height: b.height,
        font_size: b.font_size,
        font: b.font.trim().to_string(),
        font_picker: rune_kit::ComboboxState::default(),
        clock_format: TextEdit::new(b.clock_format.clone()),
        show_title: b.show_title,
        show_cpu: b.show_cpu,
        show_memory: b.show_memory,
        show_battery: b.show_battery,
        show_empty: b.show_empty_workspaces,
        app_display: idx(&APP_DISPLAY, &b.app_display),
        background: TextEdit::new(b.background.clone()),
        foreground: TextEdit::new(b.foreground.clone()),
        muted: TextEdit::new(b.muted.clone()),
        accent: TextEdit::new(b.accent.clone()),
        occupied: TextEdit::new(b.occupied.clone()),
        border_enabled: br.enabled,
        border_width: br.width,
        border_radius: br.radius,
        border_color: TextEdit::new(br.color.clone()),
        glow: br.glow,
        glow_opacity: br.glow_opacity,
        binds: binds
            .into_iter()
            .enumerate()
            .map(|(i, (k, v))| Bind { id: i as u64, chord: TextEdit::new(k), command: TextEdit::new(v) })
            .collect(),
        next_id: n,
        ed: setups_tab::Editor::load(&cfg),
        update: update::Status::Unknown,
        update_gen: 0,
        status: String::new(),
        status_ok: true,
        restart_needed: false,
        saved_ui: String::new(),
    };
    s.saved_ui = ui_fingerprint(&s);
    s
}

/// Everything that only takes effect after a lattice restart (bar + border).
fn ui_fingerprint(s: &S) -> String {
    format!(
        "{}|{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}",
        s.font,
        s.bar_enabled, s.bar_height, s.font_size, s.clock_format.text, s.show_title, s.show_cpu, s.show_memory,
        s.show_battery, s.show_empty, s.app_display, s.background.text, s.foreground.text, s.muted.text,
        s.accent.text, s.occupied.text, s.border_enabled, s.border_width, s.border_radius, s.border_color.text,
        s.glow, s.glow_opacity, 0
    )
}

fn valid_hex(s: &str) -> bool {
    let h = s.trim().strip_prefix('#').unwrap_or("");
    (h.len() == 6 || h.len() == 8) && h.chars().all(|c| c.is_ascii_hexdigit())
}

fn bind_errors(b: &Bind) -> Option<String> {
    let chord = b.chord.text.trim();
    let cmd = b.command.text.trim();
    if chord.is_empty() && cmd.is_empty() {
        return None; // blank rows are dropped on save
    }
    if let Err(e) = config::parse_chord(chord) {
        return Some(e);
    }
    config::parse_command(cmd).err()
}

/// The app->workspace rules the Apps tab owns: `workspace` plus `exe` and/or `title`, and
/// nothing else. Rules with other keys (a floating app pinned to a workspace, a class match)
/// are left to the config file, untouched. Returns (exe, title, workspace).
fn assignment(t: &toml_edit::Table) -> Option<(String, String, usize)> {
    if t.iter().any(|(k, _)| !matches!(k, "exe" | "title" | "workspace")) {
        return None;
    }
    let ws = t.get("workspace")?.as_integer().filter(|n| *n >= 1)? as usize;
    let field = |k: &str| match t.get(k) {
        None => Some(String::new()),
        Some(v) => v.as_str().map(str::to_string),
    };
    let (exe, title) = (field("exe")?, field("title")?);
    (!exe.is_empty() || !title.is_empty()).then_some((exe, title, ws))
}

/// An exe matcher that can never match: a path, or a name without ".exe".
fn bad_exe(exe: &str) -> bool {
    let exe = exe.trim();
    !exe.is_empty() && (exe.contains(['\\', '/']) || !exe.to_ascii_lowercase().ends_with(".exe"))
}

fn assign_error(a: &Assign, workspaces: usize) -> Option<String> {
    if a.blank() {
        return None; // blank rows are dropped on save
    }
    if bad_exe(&a.exe.text) {
        return Some("use the program's file name, like Discord.exe".into());
    }
    if !(1..=workspaces).contains(&a.ws) {
        return Some(format!("there are only {workspaces} workspaces"));
    }
    None
}

/// Writes the Apps tab back into `[[rules]]`: rows loaded from a rule update it in place (its
/// comments stay), rules whose row was removed go, new rows are appended. Every other rule is
/// left exactly as it was.
fn write_assignments(doc: &mut DocumentMut, apps: &[Assign]) -> Result<(), String> {
    let apps: Vec<&Assign> = apps.iter().filter(|a| !a.blank()).collect();
    if doc.get("rules").is_none() {
        if apps.is_empty() {
            return Ok(());
        }
        doc["rules"] = toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new());
    }
    let arr = doc["rules"]
        .as_array_of_tables_mut()
        .ok_or("Write the rules in config.toml as [[rules]] tables to edit apps here")?;
    let owned: Vec<usize> = arr.iter().enumerate().filter(|(_, t)| assignment(t).is_some()).map(|(i, _)| i).collect();
    let mut gone = Vec::new();
    for (k, &i) in owned.iter().enumerate() {
        match apps.iter().find(|a| a.origin == Some(k)) {
            Some(a) => fill_rule(arr.get_mut(i).expect("owned rule"), a),
            None => gone.push(i),
        }
    }
    for i in gone.into_iter().rev() {
        // The first rule carries the comment block that documents rules: hand it on.
        if i == 0 && arr.len() > 1 {
            let prefix = arr.get(0).and_then(|t| t.decor().prefix().cloned());
            if let (Some(p), Some(next)) = (prefix, arr.get_mut(1)) {
                next.decor_mut().set_prefix(p);
            }
        }
        arr.remove(i);
    }
    for a in apps.iter().filter(|a| a.origin.is_none_or(|k| k >= owned.len())) {
        let mut t = toml_edit::Table::new();
        fill_rule(&mut t, a);
        arr.push(t);
    }
    if arr.is_empty() {
        doc.remove("rules");
    }
    Ok(())
}

fn fill_rule(t: &mut toml_edit::Table, a: &Assign) {
    for (key, v) in [("exe", a.exe.text.trim()), ("title", a.title.text.trim())] {
        if v.is_empty() {
            t.remove(key);
        } else {
            put(t, key, v);
        }
    }
    put(t, "workspace", a.ws as i64);
}

/// Sets `key`, keeping the old value's trailing comment.
fn put(t: &mut toml_edit::Table, key: &str, v: impl Into<toml_edit::Value>) {
    let mut new: toml_edit::Value = v.into();
    if let Some(old) = t.get(key).and_then(|i| i.as_value()) {
        *new.decor_mut() = old.decor().clone();
    }
    t[key] = toml_edit::Item::Value(new);
}

// ---------------------------------------------------------------------------------------------
// Saving

fn set(doc: &mut DocumentMut, table: &str, key: &str, v: impl Into<toml_edit::Value>) {
    if doc.get(table).and_then(|t| t.as_table()).is_none() {
        doc[table] = toml_edit::table();
    }
    let item = &mut doc[table][key];
    let mut new: toml_edit::Value = v.into();
    // Keep any trailing comment on the line.
    if let Some(old) = item.as_value() {
        *new.decor_mut() = old.decor().clone();
    }
    *item = toml_edit::Item::Value(new);
}

fn round2(v: f32) -> f64 {
    ((v as f64) * 100.0).round() / 100.0
}

fn save(s: &mut S) -> Result<String, String> {
    for c in [&s.background, &s.foreground, &s.muted, &s.accent, &s.occupied, &s.border_color] {
        if !valid_hex(&c.text) {
            return Err(format!("'{}' isn't a colour like #89b4fa", c.text.trim()));
        }
    }
    let mut chords = std::collections::HashSet::new();
    for b in &s.binds {
        if let Some(e) = bind_errors(b) {
            return Err(format!("Keybind \"{}\": {e}", b.chord.text.trim()));
        }
        let c = b.chord.text.trim().to_ascii_lowercase();
        if !c.is_empty() && !chords.insert(c.clone()) {
            return Err(format!("\"{c}\" is bound twice"));
        }
    }
    let workspaces = s.workspaces.round() as usize;
    let mut apps = std::collections::HashSet::new();
    for a in &s.apps {
        let name = if a.exe.text.trim().is_empty() { a.title.text.trim() } else { a.exe.text.trim() };
        if let Some(e) = assign_error(a, workspaces) {
            return Err(format!("App \"{name}\": {e}"));
        }
        if !a.blank() && !apps.insert((a.exe.text.trim().to_ascii_lowercase(), a.title.text.trim().to_lowercase())) {
            return Err(format!("\"{name}\" is listed twice"));
        }
    }
    let path = config::config_path();
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| config::DEFAULT_CONFIG.to_string());
    let mut doc: DocumentMut = text.parse().map_err(|e| format!("config.toml doesn't parse: {e}"))?;

    set(&mut doc, "general", "layout", LAYOUTS[s.layout]);
    set(&mut doc, "general", "workspaces", s.workspaces.round() as i64);
    set(&mut doc, "general", "gaps_inner", s.gaps_inner.round() as i64);
    set(&mut doc, "general", "gaps_outer", s.gaps_outer.round() as i64);
    set(&mut doc, "general", "split_ratio", round2(s.split_ratio));
    set(&mut doc, "general", "master_ratio", round2(s.master_ratio));
    set(&mut doc, "general", "new_window", NEW_WINDOW[s.new_window]);
    set(&mut doc, "general", "float_unresizable", s.float_unresizable);
    set(&mut doc, "general", "follow_activation", s.follow_activation);
    set(&mut doc, "general", "mouse_follows_focus", s.mouse_follows_focus);
    set(&mut doc, "general", "check_for_updates", s.check_updates);

    set(&mut doc, "animations", "enabled", s.anim_enabled);
    set(&mut doc, "animations", "style", ANIM_STYLES[s.anim_style]);

    set(&mut doc, "bar", "enabled", s.bar_enabled);
    set(&mut doc, "bar", "height", s.bar_height.round() as i64);
    set(&mut doc, "bar", "font_size", s.font_size.round() as i64);
    set(&mut doc, "bar", "font", s.font.as_str());
    set(&mut doc, "bar", "clock_format", s.clock_format.text.as_str());
    set(&mut doc, "bar", "show_title", s.show_title);
    set(&mut doc, "bar", "show_cpu", s.show_cpu);
    set(&mut doc, "bar", "show_memory", s.show_memory);
    set(&mut doc, "bar", "show_battery", s.show_battery);
    set(&mut doc, "bar", "show_empty_workspaces", s.show_empty);
    set(&mut doc, "bar", "app_display", APP_DISPLAY[s.app_display]);
    set(&mut doc, "bar", "background", s.background.text.trim());
    set(&mut doc, "bar", "foreground", s.foreground.text.trim());
    set(&mut doc, "bar", "muted", s.muted.text.trim());
    set(&mut doc, "bar", "accent", s.accent.text.trim());
    set(&mut doc, "bar", "occupied", s.occupied.text.trim());

    set(&mut doc, "border", "enabled", s.border_enabled);
    set(&mut doc, "border", "width", s.border_width.round() as i64);
    set(&mut doc, "border", "radius", s.border_radius.round() as i64);
    set(&mut doc, "border", "color", s.border_color.text.trim());
    set(&mut doc, "border", "glow", s.glow.round() as i64);
    set(&mut doc, "border", "glow_opacity", round2(s.glow_opacity));

    // Keybinds: rewrite the table's entries (its header comments stay).
    if doc.get("keybinds").and_then(|t| t.as_table()).is_none() {
        doc["keybinds"] = toml_edit::table();
    }
    let table = doc["keybinds"].as_table_mut().expect("keybinds table");
    table.clear();
    for b in &s.binds {
        let (c, m) = (b.chord.text.trim(), b.command.text.trim());
        if !c.is_empty() {
            table.insert(c, toml_edit::value(m));
        }
    }

    write_assignments(&mut doc, &s.apps)?;
    setups_tab::validate(s)?;
    setups_tab::write(&mut doc, &s.ed);

    // Make sure what we write is a config lattice accepts before touching the file.
    let out = doc.to_string();
    toml::from_str::<Config>(&out).map_err(|e| format!("Not saved: {e}"))?;
    std::fs::write(&path, out).map_err(|e| format!("Couldn't write {}: {e}", path.display()))?;

    let reloaded = notify_reload();
    let ui = ui_fingerprint(s);
    if ui != s.saved_ui {
        s.restart_needed = true;
        s.saved_ui = ui;
    }
    Ok(match (reloaded, s.restart_needed) {
        (false, _) => "Saved; applies when lattice starts.".into(),
        (true, true) => "Saved. Bar and border changes need a restart.".into(),
        (true, false) => "Saved and applied.".into(),
    })
}

// ---------------------------------------------------------------------------------------------
// Talking to the running lattice (its hidden "lattice-control" window)

fn control_window() -> Option<windows::Win32::Foundation::HWND> {
    use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
    use windows::core::w;
    unsafe { FindWindowW(w!("lattice-display-listener"), w!("lattice-control")).ok().filter(|h| !h.is_invalid()) }
}

fn notify_reload() -> bool {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
    match control_window() {
        Some(h) => unsafe { PostMessageW(Some(h), WM_APP + 2, WPARAM(0), LPARAM(0)).is_ok() },
        None => false,
    }
}

/// Quits the running lattice the clean way (restores hidden windows) and starts it again.
fn restart_lattice() -> Result<(), String> {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};
    if let Some(h) = control_window() {
        unsafe {
            let _ = PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        let start = std::time::Instant::now();
        while control_window().is_some() && start.elapsed() < std::time::Duration::from_secs(5) {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        // The process lingers a moment after its window goes; let its single-instance lock go.
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    std::process::Command::new(exe).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// View

struct P {
    t: rune::tok::Theme,
    accent: Color,
    hair: f32,
}

fn kit(p: &P) -> Kit {
    let mut k = Kit::neutral(true);
    k.c.bg_accent = p.accent;
    k.c.border_focus = p.accent;
    k
}

fn section(p: &P, key: &str, title: &str, rows: Vec<El>) -> El {
    let n = rows.len();
    let mut card = col()
        .key("card")
        .padding(space::S1)
        .fill(p.t.card)
        .radius(radius::LG)
        .border(p.hair, p.t.card_border);
    for (i, r) in rows.into_iter().enumerate() {
        card = card.child(r);
        if i + 1 < n {
            card = card.child(el().key(format!("sep{i}")).h(p.hair).ml(space::S3).mr(space::S3).fill(p.t.separator));
        }
    }
    col()
        .key(key.to_string())
        .gap(space::S2)
        .child(text(title.to_string()).key("header").font(type_::SECTION).color(p.t.text_secondary).ml(space::S3))
        .child(card)
}

fn setting_row(p: &P, key: &str, label: &str, hint: Option<&str>, control: El) -> El {
    let mut labels = col().gap(2.0).shrink().child(text(label.to_string()).font(type_::BODY).color(p.t.text));
    if let Some(h) = hint {
        labels = labels.child(text(h.to_string()).font(TextStyle::new(11.5, 400.0)).color(p.t.text_secondary));
    }
    row()
        .key(key.to_string())
        .min_h(44.0)
        .py(space::S2)
        .px(space::S3)
        .gap(space::S3)
        .center_y()
        .justify_between()
        .child(labels)
        .child(control)
}

fn toggle(p: &P, key: &str, label: &str, hint: Option<&str>, on: bool, acc: fn(&mut S) -> &mut bool) -> El {
    let k = kit(p);
    let sw = Switch::new(on).label(label.to_string()).hide_label().on_toggle(move |s: &mut S| {
        let v = acc(s);
        *v = !*v;
    });
    setting_row(p, key, label, hint, sw.build(&k))
}

#[allow(clippy::too_many_arguments)]
fn slider(p: &P, key: &str, label: &str, v: f32, lo: f32, hi: f32, step: f32, fmt: fn(f32) -> String, acc: fn(&mut S) -> &mut f32) -> El {
    let k = kit(p);
    let control = row()
        .gap(space::S3)
        .center_y()
        .child(
            Slider::new(format!("{key}-slider"), v)
                .range(lo..=hi)
                .step(step)
                .label(label.to_string())
                .hide_label()
                .width(200.0)
                .on_change(move |s: &mut S, v, _| *acc(s) = v)
                .build(&k),
        )
        .child(text(fmt(v)).w(48.0).font(type_::MONO_CAPTION).color(p.t.text_secondary).align(rune::Align::End));
    setting_row(p, key, label, None, control)
}

fn choice(p: &P, key: &str, label: &str, hint: Option<&str>, options: &[&str], current: usize, acc: fn(&mut S) -> &mut usize) -> El {
    let k = kit(p);
    let seg = SegmentedControl::new(options, Some(current)).label(label.to_string()).fit().on_pick(move |s: &mut S, i, _| *acc(s) = i);
    setting_row(p, key, label, hint, seg.build(&k))
}

fn text_field(p: &P, key: &str, label: &str, hint: Option<&str>, edit: &TextEdit, width: f32, invalid: bool, acc: fn(&mut S) -> &mut TextEdit) -> El {
    let k = kit(p);
    let input = TextInput::new(edit).label(label.to_string()).width(Some(width)).invalid(invalid).on_edit(acc).build(&k);
    setting_row(p, key, label, hint, input)
}

fn color_field(p: &P, key: &str, label: &str, edit: &TextEdit, acc: fn(&mut S) -> &mut TextEdit) -> El {
    let ok = valid_hex(&edit.text);
    let swatch = el()
        .size(22.0, 22.0)
        .radius(6.0)
        .border(p.hair, p.t.card_border)
        .fill(if ok { crate::bar::color(edit.text.trim(), Color::default()) } else { Color::TRANSPARENT });
    let k = kit(p);
    let input = TextInput::new(edit).label(label.to_string()).width(Some(110.0)).invalid(!ok).on_edit(acc).build(&k);
    setting_row(p, key, label, None, row().gap(space::S2).center_y().child(swatch).child(input))
}

fn px(v: f32) -> String {
    format!("{} px", v.round() as i64)
}
fn pct(v: f32) -> String {
    format!("{:.0}%", v * 100.0)
}
fn count(v: f32) -> String {
    format!("{}", v.round() as i64)
}

fn general_tab(s: &S, p: &P) -> Vec<El> {
    vec![
        section(p, "layout-sec", "Tiling", vec![
            choice(p, "layout", "Default layout", Some("Win+\\ cycles it per workspace"), &LAYOUTS, s.layout, |s| &mut s.layout),
            choice(p, "new-window", "New windows go", None, &["after focused", "at the end"], s.new_window, |s| &mut s.new_window),
            slider(p, "split", "Dwindle split", s.split_ratio, 0.1, 0.9, 0.05, pct, |s| &mut s.split_ratio),
            slider(p, "master", "Master width", s.master_ratio, 0.1, 0.9, 0.05, pct, |s| &mut s.master_ratio),
        ]),
        section(p, "gaps-sec", "Spacing", vec![
            slider(p, "gaps-inner", "Gap between windows", s.gaps_inner, 0.0, 40.0, 1.0, px, |s| &mut s.gaps_inner),
            slider(p, "gaps-outer", "Gap at screen edges", s.gaps_outer, 0.0, 60.0, 1.0, px, |s| &mut s.gaps_outer),
        ]),
        section(p, "ws-sec", "Workspaces", vec![
            slider(p, "workspaces", "Number of workspaces", s.workspaces, 1.0, 10.0, 1.0, count, |s| &mut s.workspaces),
            toggle(p, "follow", "Follow app activation", Some("Jump to apps that ask for attention"), s.follow_activation, |s| &mut s.follow_activation),
        ]),
        section(p, "anim-sec", "Animations", vec![
            toggle(p, "anim-on", "Animate windows", Some("Moves, swaps and workspace switches"), s.anim_enabled, |s| &mut s.anim_enabled),
            choice(p, "anim-style", "Style", Some(if s.anim_style == 0 { "Smooth, slight overshoot" } else { "Quick and plain" }), &ANIM_STYLES, s.anim_style, |s| &mut s.anim_style),
        ]),
        section(p, "behavior-sec", "Behaviour", vec![
            toggle(p, "float-unres", "Float fixed-size windows", Some("Dialogs and other non-resizable windows"), s.float_unresizable, |s| &mut s.float_unresizable),
            toggle(p, "mouse-follow", "Mouse follows focus", Some("Cursor moves with keyboard focus"), s.mouse_follows_focus, |s| &mut s.mouse_follows_focus),
            setting_row(p, "rules", &format!("Other window rules ({})", s.other_rules), Some("Float or ignore apps in config.toml"), open_config_button(p)),
        ]),
        section(p, "update-sec", "Updates", vec![
            toggle(p, "check-updates", "Check for updates", Some("Daily; never installs on its own"), s.check_updates, |s| &mut s.check_updates),
            update_row(s, p),
        ]),
    ]
}

/// "lattice x.y.z" with what the last check found, and the button for the next step.
fn update_row(s: &S, p: &P) -> El {
    let k = kit(p);
    let installed = update::installed();
    let check = || Button::new("Check now").variant(ButtonVariant::Outline).on_press(|_: &mut S| update::check_async()).build(&k);
    let (hint, control): (String, El) = match &s.update {
        update::Status::Unknown => ("Not checked yet".into(), check()),
        update::Status::Checking => ("Checking GitHub…".into(), el()),
        update::Status::UpToDate => ("Up to date".into(), check()),
        update::Status::CheckFailed(e) => (format!("Check failed: {e}"), check()),
        update::Status::Available(r) if !installed => (
            format!("{} is out; get it from GitHub releases", r.version_str()),
            check(),
        ),
        update::Status::Available(r) | update::Status::InstallFailed(r, _) => {
            let hint = match &s.update {
                update::Status::InstallFailed(_, e) => format!("Update failed: {e}"),
                _ => format!("{} is available", r.version_str()),
            };
            let button = Button::new(format!("Install {}", r.version_str()))
                .variant(ButtonVariant::Submit)
                .on_press(|_: &mut S| update::install_async())
                .build(&k);
            (hint, button)
        }
        update::Status::Downloading(r) => (format!("Downloading {}…", r.version_str()), el()),
        update::Status::Installing(r) => (format!("Installing {}; lattice will restart", r.version_str()), el()),
    };
    setting_row(p, "update", &format!("lattice {}", env!("CARGO_PKG_VERSION")), Some(&hint), control)
}

fn setup(_s: &mut S, wake: rune::Waker) {
    setups_tab::set_waker(wake.clone());
    update::set_waker(wake);
}

fn poll(s: &mut S, _t: f64, _tx: &mut Tx) -> bool {
    let setups = setups_tab::poll(s);
    let g = update::GEN.load(std::sync::atomic::Ordering::SeqCst);
    if g == s.update_gen {
        return setups;
    }
    s.update_gen = g;
    s.update = update::status();
    true
}

fn bar_tab(s: &S, p: &P, overlays: &mut Vec<El>) -> Vec<El> {
    vec![
        section(p, "bar-sec", "Top bar", vec![
            toggle(p, "bar-on", "Show the bar", None, s.bar_enabled, |s| &mut s.bar_enabled),
            slider(p, "bar-h", "Height", s.bar_height, 16.0, 60.0, 1.0, px, |s| &mut s.bar_height),
            slider(p, "font", "Font size", s.font_size, 9.0, 20.0, 1.0, px, |s| &mut s.font_size),
            font_row(s, p, overlays),
            font_preview(s, p),
            choice(p, "app-display", "Apps on workspaces", None, &APP_DISPLAY, s.app_display, |s| &mut s.app_display),
            text_field(p, "clock", "Clock format", Some("%H %M %S %I %p %a %b %d %m %Y"), &s.clock_format, 200.0, false, |s| &mut s.clock_format),
        ]),
        section(p, "bar-show-sec", "Show", vec![
            toggle(p, "show-title", "Window title", None, s.show_title, |s| &mut s.show_title),
            toggle(p, "show-cpu", "CPU", None, s.show_cpu, |s| &mut s.show_cpu),
            toggle(p, "show-mem", "Memory", None, s.show_memory, |s| &mut s.show_memory),
            toggle(p, "show-bat", "Battery", None, s.show_battery, |s| &mut s.show_battery),
            toggle(p, "show-empty", "Empty workspaces", None, s.show_empty, |s| &mut s.show_empty),
        ]),
        section(p, "bar-colors", "Colours", vec![
            color_field(p, "c-bg", "Background", &s.background, |s| &mut s.background),
            color_field(p, "c-fg", "Text", &s.foreground, |s| &mut s.foreground),
            color_field(p, "c-muted", "Muted text", &s.muted, |s| &mut s.muted),
            color_field(p, "c-accent", "Accent (active workspace)", &s.accent, |s| &mut s.accent),
            color_field(p, "c-occ", "Workspace pills", &s.occupied, |s| &mut s.occupied),
        ]),
    ]
}

fn border_tab(s: &S, p: &P) -> Vec<El> {
    // Live preview of the glow, drawn the same way the overlay draws it.
    let color = crate::bar::color(s.border_color.text.trim(), p.accent);
    let (w, h) = (260.0, 120.0);
    let m = s.glow.max(4.0) * 1.5;
    let mut preview = el().size(w + 2.0 * m, h + 2.0 * m).child(el().absolute(m, m).size(w, h).radius(s.border_radius).fill(p.t.card));
    if s.border_enabled {
        if s.glow > 0.0 && s.glow_opacity > 0.0 {
            let n = s.glow.ceil() as i32;
            for d in (1..=n).rev() {
                let t = d as f32 / (n as f32 + 1.0);
                let a = s.glow_opacity * (1.0 - t).powi(2);
                let df = d as f32;
                preview = preview.child(el().absolute(m - df, m - df).size(w + 2.0 * df, h + 2.0 * df).radius(s.border_radius + df).border(1.0, color.alpha(a)));
            }
        }
        if s.border_width > 0.0 {
            preview = preview.child(el().absolute(m, m).size(w, h).radius(s.border_radius).border(s.border_width, color));
        }
    }
    vec![
        section(p, "border-preview", "Preview", vec![row().key("pv").py(space::S3).center().w_full().child(preview)]),
        section(p, "border-sec", "Focus border", vec![
            toggle(p, "border-on", "Glow around the focused window", None, s.border_enabled, |s| &mut s.border_enabled),
            color_field(p, "border-color", "Colour", &s.border_color, |s| &mut s.border_color),
            slider(p, "border-w", "Line width", s.border_width, 0.0, 8.0, 1.0, px, |s| &mut s.border_width),
            slider(p, "border-r", "Corner radius", s.border_radius, 0.0, 20.0, 1.0, px, |s| &mut s.border_radius),
            slider(p, "glow", "Glow size", s.glow, 0.0, 40.0, 1.0, px, |s| &mut s.glow),
            slider(p, "glow-op", "Glow strength", s.glow_opacity, 0.0, 1.0, 0.05, pct, |s| &mut s.glow_opacity),
        ]),
    ]
}

fn keybinds_tab(s: &S, p: &P) -> Vec<El> {
    let k = kit(p);
    let mut rows = Vec::new();
    for (i, b) in s.binds.iter().enumerate() {
        let err = bind_errors(b);
        let chord = TextInput::new(&b.chord)
            .label("Keys")
            .placeholder("super+shift+x")
            .width(Some(190.0))
            .invalid(err.is_some() && config::parse_chord(b.chord.text.trim()).is_err())
            .on_edit(move |s: &mut S| &mut s.binds[i].chord)
            .build(&k);
        let command = TextInput::new(&b.command)
            .label("Command")
            .placeholder("workspace 1")
            .width(Some(250.0))
            .invalid(err.is_some() && config::parse_chord(b.chord.text.trim()).is_ok())
            .on_edit(move |s: &mut S| &mut s.binds[i].command)
            .build(&k);
        let remove = Button::new("Remove").variant(ButtonVariant::GhostMuted).on_press(move |s: &mut S| {
            if i < s.binds.len() {
                s.binds.remove(i);
            }
        });
        let mut line = col().key(format!("bind-{}", b.id)).px(space::S3).py(space::S1).gap(2.0).child(
            row().gap(space::S2).center_y().child(chord).child(command).child(remove.build(&k)),
        );
        if let Some(e) = err {
            line = line.child(text(e).font(TextStyle::new(11.5, 500.0)).color(crate::bar::color("#f38ba8", Color::WHITE)));
        }
        rows.push(line);
    }
    let add = Button::new("Add keybind").on_press(|s: &mut S| {
        s.binds.push(Bind { id: s.next_id, chord: TextEdit::new(""), command: TextEdit::new("") });
        s.next_id += 1;
    });
    rows.push(row().key("add").px(space::S3).py(space::S2).child(add.build(&k)));
    vec![
        col().key("kb-help").gap(4.0).mx(space::S3).w_full().child(
            text("Modifiers: super, shift, ctrl, alt.  Commands: focus / move-window left|right|up|down, workspace N|next|prev, move-to-workspace N, layout next, resize left|right|up|down [amount], toggle-float, toggle-fullscreen, close, launcher, window-search, setup-picker, restore-setup <name>, settings, open-config, reload, quit, exec <program>.".to_string())
                .w_full()
                .wrap()
                .font(TextStyle::new(11.5, 400.0))
                .color(p.t.text_secondary),
        ),
        section(p, "kb-sec", &format!("Keybinds ({})", s.binds.len()), rows),
    ]
}

fn apps_tab(s: &S, p: &P) -> Vec<El> {
    let k = kit(p);
    let workspaces = s.workspaces.round() as usize;
    let mut rows = Vec::new();
    for (i, a) in s.apps.iter().enumerate() {
        let err = assign_error(a, workspaces);
        let exe = TextInput::new(&a.exe)
            .label("Program")
            .placeholder("Discord.exe")
            .width(Some(200.0))
            .invalid(bad_exe(&a.exe.text))
            .on_edit(move |s: &mut S| &mut s.apps[i].exe)
            .build(&k);
        let title = TextInput::new(&a.title)
            .label("Title contains")
            .placeholder("title contains (optional)")
            .width(Some(170.0))
            .on_edit(move |s: &mut S| &mut s.apps[i].title)
            .build(&k);
        let step = |label: &str, d: isize, disabled: bool| {
            Button::new(label)
                .variant(ButtonVariant::Ghost)
                .disabled(disabled)
                .on_press(move |s: &mut S| {
                    let max = (s.workspaces.round() as usize).max(1);
                    if let Some(a) = s.apps.get_mut(i) {
                        a.ws = a.ws.saturating_add_signed(d).clamp(1, max);
                    }
                })
                .build(&k)
        };
        let ws = row()
            .gap(2.0)
            .center_y()
            .child(text("Workspace").font(TextStyle::new(11.5, 400.0)).color(p.t.text_secondary).mr(space::S1))
            .child(step("-", -1, a.ws <= 1))
            .child(text(a.ws.to_string()).w(20.0).align(rune::Align::Center).font(type_::BODY).color(p.t.text))
            .child(step("+", 1, a.ws >= workspaces));
        let remove = Button::new("Remove").variant(ButtonVariant::GhostMuted).on_press(move |s: &mut S| {
            if i < s.apps.len() {
                s.apps.remove(i);
            }
        });
        let mut line = col().key(format!("app-{}", a.id)).px(space::S3).py(space::S1).gap(2.0).child(
            row().gap(space::S2).center_y().child(exe).child(title).child(ws).child(remove.build(&k)),
        );
        if let Some(e) = err {
            line = line.child(text(e).font(TextStyle::new(11.5, 500.0)).color(crate::bar::color("#f38ba8", Color::WHITE)));
        }
        rows.push(line);
    }
    let add = Button::new("Add app").on_press(|s: &mut S| {
        let ws = (s.workspaces.round() as usize).clamp(1, 2);
        s.apps.push(Assign { id: s.next_id, origin: None, exe: TextEdit::new(""), title: TextEdit::new(""), ws });
        s.next_id += 1;
    });
    rows.push(row().key("add").px(space::S3).py(space::S2).child(add.build(&k)));
    vec![
        col().key("apps-help").gap(4.0).mx(space::S3).w_full().child(
            text("Send an app's windows to a workspace, matched by program file name (see lattice --list) and optional title text. Saving also moves its open windows.".to_string())
                .w_full()
                .wrap()
                .font(TextStyle::new(11.5, 400.0))
                .color(p.t.text_secondary),
        ),
        section(p, "apps-sec", &format!("App workspaces ({})", s.apps.len()), rows),
    ]
}

const DEFAULT_FONT_ID: &str = "__default__";

/// Font dropdown listing every installed family (searchable). Fonts are enumerated only
/// while the dropdown is open, so opening the settings window stays fast.
fn font_row(s: &S, p: &P, overlays: &mut Vec<El>) -> El {
    let k = kit(p);
    let current_id = if s.font.is_empty() { DEFAULT_FONT_ID } else { s.font.as_str() };
    let default_label = "Default (Inter)";
    let options = if s.font_picker.is_open() {
        let mut opts = vec![rune_kit::ComboboxOption::new(DEFAULT_FONT_ID, default_label)];
        for fam in rune::text::installed_families() {
            let mut opt = rune_kit::ComboboxOption::new(fam.name(), fam.name());
            let n = fam.weights().len();
            let weights = if n == 1 { "1 weight".to_owned() } else { format!("{n} weights") };
            opt.detail = if fam.monospace() { format!("Mono · {weights}") } else { weights };
            opts.push(opt);
        }
        opts
    } else {
        vec![rune_kit::ComboboxOption::new(current_id, if s.font.is_empty() { default_label } else { s.font.as_str() })]
    };
    let picker = rune_kit::Combobox::new("font-picker", &s.font_picker, &options, |s: &mut S| &mut s.font_picker)
        .label("Font")
        .current(Some(current_id))
        .placeholder(default_label)
        .search_placeholder("Search installed fonts…")
        .width(240.0)
        .placement(rune_kit::Placement::BottomEnd)
        .on_choose(|s: &mut S, chosen: &str, _tx: &mut Tx| {
            s.font = if chosen == DEFAULT_FONT_ID { String::new() } else { chosen.to_string() };
            s.font_picker.close();
        })
        .build(&k, overlays);
    setting_row(p, "font-family", "Font", Some("Any installed font"), picker)
}

/// A line of bar-like text drawn in the chosen font and size.
fn font_preview(s: &S, p: &P) -> El {
    let family = if s.font.is_empty() { rune::Family::Sans } else { rune::Family::Named(crate::bar::font_list(&s.font)) };
    let size = s.font_size.round().clamp(9.0, 20.0);
    let sample = text("1  2  3      Tue Nov 24  16:20      CPU 12%  MEM 42%")
        .font(TextStyle::new(size, 600.0).family(family))
        .color(p.t.text);
    setting_row(
        p,
        "font-preview",
        "Preview",
        None,
        el().px(space::S3).py(space::S2).radius(radius::MD).fill(crate::bar::color("#1e1e2e", Color::BLACK)).child(sample),
    )
}

fn open_config_button(p: &P) -> El {
    Button::new("Open config file").variant(ButtonVariant::Outline).on_press(|_: &mut S| open_config_file()).build(&kit(p))
}

fn open_config_file() {
    let path = config::config_path();
    let _ = std::process::Command::new("notepad.exe").arg(path).spawn();
}

fn view(s: &S, cx: &mut Cx<S>) -> El {
    let p = P { t: rune::tok::theme(true), accent: crate::bar::color("#89b4fa", Color::WHITE), hair: 1.0 / cx.env().scale };
    let k = kit(&p);
    let header = row()
        .key("header")
        .px(space::S6)
        .pt(space::S6)
        .pb(space::S3)
        .center_y()
        .gap(space::S3)
        .child(el().size(28.0, 28.0).radius(7.0).gradient(p.accent, crate::bar::color("#cba6f7", Color::WHITE)))
        .child(
            col().gap(2.0).grow(1.0)
                .child(text("lattice settings").font(TextStyle::new(18.0, 700.0)).color(p.t.text))
                .child(text(config::config_path().display().to_string()).font(TextStyle::new(11.5, 400.0)).color(p.t.text_secondary)),
        );
    let tabs = row().key("tabs").px(space::S6).pb(space::S3).child(
        SegmentedControl::new(&TABS, Some(s.tab)).label("Section").as_tabs().fit().on_pick(|s: &mut S, i, _| s.tab = i).build(&k),
    );
    // Popups (the font dropdown) draw here, above the scrolling content.
    let mut overlays: Vec<El> = Vec::new();
    let body = match s.tab {
        0 => general_tab(s, &p),
        1 => apps_tab(s, &p),
        2 => setups_tab::tab(s, &p, &mut overlays),
        3 => bar_tab(s, &p, &mut overlays),
        4 => border_tab(s, &p),
        _ => keybinds_tab(s, &p),
    };
    let content = col()
        .key(format!("content-{}", s.tab))
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec::default())
        .px(space::S6)
        .pb(space::S6)
        .gap(space::S6)
        .children(body);

    let status_color = if s.status_ok { p.t.text_secondary } else { crate::bar::color("#f38ba8", Color::WHITE) };
    let mut footer = row()
        .key("footer")
        .px(space::S6)
        .py(space::S3)
        .gap(space::S2)
        .center_y()
        .fill(p.t.card)
        .child(text(s.status.clone()).grow(1.0).shrink().truncate().font(TextStyle::new(12.0, 500.0)).color(status_color))
        .child(open_config_button(&p));
    if s.restart_needed {
        footer = footer.child(
            Button::new("Restart lattice")
                .variant(ButtonVariant::Warning)
                .on_press(|s: &mut S| match restart_lattice() {
                    Ok(()) => {
                        s.restart_needed = false;
                        s.status = "Restarted with the new settings.".into();
                        s.status_ok = true;
                    }
                    Err(e) => {
                        s.status = format!("Restart failed: {e}");
                        s.status_ok = false;
                    }
                })
                .build(&k),
        );
    }
    footer = footer.child(
        Button::new("Save")
            .variant(ButtonVariant::Submit)
            .on_press_tx(|s: &mut S, _tx: &mut Tx| match save(s) {
                Ok(msg) => {
                    s.status = msg;
                    s.status_ok = true;
                }
                Err(e) => {
                    s.status = e;
                    s.status_ok = false;
                }
            })
            .build(&k),
    );

    let window = col().key("window").w_full().h_full().fill(p.t.bg).child(header).child(tabs).child(content).child(footer);
    rune_kit::root(window, overlays)
}

// ---------------------------------------------------------------------------------------------
// Entry

pub fn run() {
    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, SW_RESTORE, SetForegroundWindow, ShowWindow};
    use windows::core::{HSTRING, w};
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // One settings window: a second Win+Alt+Comma brings the first to the front.
        let guard = CreateMutexW(None, true, w!("Local\\lattice-settings"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if let Ok(h) = FindWindowW(None, &HSTRING::from(TITLE))
                && !h.is_invalid()
            {
                let _ = ShowWindow(h, SW_RESTORE);
                let _ = SetForegroundWindow(h);
            }
            return;
        }
        std::mem::forget(guard);
    }

    // Centre on the primary monitor's work area.
    let (lw, lh) = (760.0_f32, 700.0_f32);
    let monitors = win::monitors();
    let primary = monitors.iter().find(|m| m.primary).or(monitors.first());
    let position = primary.map(|m| {
        let (pw, ph) = ((lw * m.scale) as i32, (lh * m.scale) as i32);
        (m.work.x + (m.work.w - pw).max(0) / 2, m.work.y + (m.work.h - ph).max(0) / 2)
    });
    App::new(AppSpec {
        setup: Some(setup),
        poll: Some(poll),
        min_size: Some((560.0, 420.0)),
        // For headless previews: `--snap x.png --scene bar`.
        scenes: &[
            ("apps", |s: &mut S| {
                s.tab = 1;
                s.apps.push(Assign { id: 9000, origin: None, exe: TextEdit::new("Discord.exe"), title: TextEdit::new(""), ws: 3 });
            }),
            ("workspaces", |s: &mut S| {
                s.tab = 2;
                setups_tab::sample(s);
            }),
            ("workspaces-drag", |s: &mut S| {
                s.tab = 2;
                setups_tab::sample(s);
                setups_tab::sample_drag(s);
            }),
            ("workspaces-resize", |s: &mut S| {
                s.tab = 2;
                setups_tab::sample(s);
                setups_tab::sample_resize(s);
            }),
            ("workspaces-master", |s: &mut S| {
                s.tab = 2;
                setups_tab::sample(s);
                setups_tab::sample_master(s);
            }),
            ("workspaces-add", |s: &mut S| {
                s.tab = 2;
                setups_tab::sample(s);
                setups_tab::sample_add(s);
            }),
            ("workspaces-monitor", |s: &mut S| {
                s.tab = 2;
                setups_tab::sample(s);
                setups_tab::sample_monitor(s);
            }),
            ("workspaces-list", |s: &mut S| {
                s.tab = 2;
                setups_tab::sample(s);
                setups_tab::sample_list(s);
            }),
            ("workspaces-none", |s: &mut S| {
                s.tab = 2;
                setups_tab::sample(s);
                setups_tab::sample_none(s);
            }),
            ("workspaces-add-ws", |s: &mut S| {
                s.tab = 2;
                setups_tab::sample(s);
                setups_tab::sample_add_ws(s);
            }),
            ("workspaces-empty", |s: &mut S| {
                s.tab = 2;
                s.ed.list.clear();
                s.ed.sel = None;
            }),
            ("bar", |s: &mut S| s.tab = 3),
            ("border", |s: &mut S| s.tab = 4),
            ("keybinds", |s: &mut S| s.tab = 5),
            ("fonts", |s: &mut S| {
                s.tab = 3;
                s.font_picker.step(rune_kit::ComboboxEvent::Toggle { current: Some(DEFAULT_FONT_ID) });
            }),
            ("segoe", |s: &mut S| {
                s.tab = 3;
                s.font = "Segoe UI".into();
            }),
        ],
        ..AppSpec::new(TITLE, (lw, lh), init, view)
    })
        .window_options(WindowOptions { always_on_top: true, position, ..Default::default() })
        .color_scheme(ColorScheme::Dark)
        .run();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_edits_values_keeps_comments_and_validates_keybinds() {
        let dir = std::env::temp_dir().join(format!("lattice-settings-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, config::DEFAULT_CONFIG).unwrap();
        // SAFETY: the only test that touches LATTICE_CONFIG.
        unsafe { std::env::set_var("LATTICE_CONFIG", &path) };

        let mut s = init();
        // Animations apply on reload: turning them on asks for no restart.
        s.anim_enabled = true;
        s.anim_style = 1;
        save(&mut s).expect("save animations");
        let cfg: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(cfg.animations.active(), Some(config::AnimStyle::Fast));
        assert!(!s.restart_needed, "animations apply without a restart");

        let before = s.binds.len();
        s.gaps_inner = 20.0;
        s.glow_opacity = 0.7;
        s.layout = 1;
        s.font = "Segoe UI".into();
        s.binds.push(Bind { id: 999, chord: TextEdit::new("super+shift+t"), command: TextEdit::new("exec wt.exe") });
        save(&mut s).expect("save");

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("gaps_inner = 20"));
        assert!(text.contains("glow_opacity = 0.7"));
        assert!(text.contains("layout = \"master\""));
        assert!(text.contains("font = \"Segoe UI\""));
        assert!(text.contains("# Gap between tiled windows"), "comments survive");
        let cfg: Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg.keybinds.len(), before + 1);
        assert_eq!(cfg.keybinds["super+shift+t"], "exec wt.exe");
        assert!(s.restart_needed, "border change needs a restart");

        s.binds.push(Bind { id: 1000, chord: TextEdit::new("super+banana"), command: TextEdit::new("close") });
        assert!(save(&mut s).is_err(), "bad chord blocks saving");
        assert!(!std::fs::read_to_string(&path).unwrap().contains("banana"));
        let _ = std::fs::remove_dir_all(dir);
    }

    fn app(origin: Option<usize>, exe: &str, ws: usize) -> Assign {
        Assign { id: 0, origin, exe: TextEdit::new(exe), title: TextEdit::new(""), ws }
    }

    const RULES: &str = r#"
# Window rules header
[[rules]]
exe = "Discord.exe"   # chat
workspace = 3

[[rules]]
exe = "Calculator.exe"
float = true

[[rules]]
exe = "Spotify.exe"
workspace = 4
float = true

# music
[[rules]]
exe = "Foobar.exe"
workspace = 5

[keybinds]
"super+q" = "close"
"#;

    #[test]
    fn only_plain_workspace_rules_are_assignments() {
        let doc: DocumentMut = RULES.parse().unwrap();
        let found: Vec<_> = doc["rules"].as_array_of_tables().unwrap().iter().filter_map(assignment).collect();
        assert_eq!(found, vec![("Discord.exe".into(), String::new(), 3), ("Foobar.exe".into(), String::new(), 5)]);
        let t: DocumentMut = "title = \"Teams\"\nworkspace = 2".parse().unwrap();
        assert_eq!(assignment(t.as_table()), Some((String::new(), "Teams".into(), 2)));
        for bad in ["exe = \"a.exe\"", "exe = \"a.exe\"\nworkspace = 0", "workspace = 2", "class = \"x\"\nworkspace = 2"] {
            let t: DocumentMut = bad.parse().unwrap();
            assert_eq!(assignment(t.as_table()), None, "{bad}");
        }
    }

    #[test]
    fn writing_assignments_leaves_other_rules_alone() {
        let mut doc: DocumentMut = RULES.parse().unwrap();
        // Discord moves to 2, Foobar is removed, Slack is added, the blank row is dropped.
        write_assignments(&mut doc, &[app(Some(0), "Discord.exe", 2), app(None, "Slack.exe", 6), app(None, "", 1)]).unwrap();
        let out = doc.to_string();
        let cfg: Config = toml::from_str(&out).unwrap();
        let got: Vec<_> = cfg.rules.iter().map(|r| (r.exe.clone().unwrap(), r.workspace, r.float)).collect();
        assert_eq!(got, vec![
            ("Discord.exe".into(), Some(2), false),
            ("Calculator.exe".into(), None, true),
            ("Spotify.exe".into(), Some(4), true),
            ("Slack.exe".into(), Some(6), false),
        ]);
        assert!(out.contains("# Window rules header"));
        assert!(out.contains("exe = \"Discord.exe\"   # chat\nworkspace = 2"), "trailing comment kept:\n{out}");
        assert!(!out.contains("Foobar") && !out.contains("# music"));
        assert!(out.find("Slack.exe").unwrap() < out.find("[keybinds]").unwrap(), "new rules go with the others:\n{out}");
        assert_eq!(cfg.keybinds.len(), 1);

        // Removing the first rule keeps the header comment.
        let mut doc: DocumentMut = RULES.parse().unwrap();
        write_assignments(&mut doc, &[app(Some(1), "Foobar.exe", 5)]).unwrap();
        let out = doc.to_string();
        assert!(!out.contains("Discord") && out.contains("# Window rules header"), "{out}");
        assert!(out.find("# Window rules header").unwrap() < out.find("Calculator").unwrap());

        // No rules yet.
        let mut doc: DocumentMut = "[general]\nworkspaces = 9\n".parse().unwrap();
        write_assignments(&mut doc, &[app(None, "Discord.exe", 3)]).unwrap();
        let cfg: Config = toml::from_str(&doc.to_string()).unwrap();
        assert_eq!(cfg.rules[0].workspace, Some(3));
    }

    #[test]
    fn assignment_validation() {
        assert_eq!(assign_error(&app(None, "", 99), 9), None, "blank rows are ignored");
        assert!(assign_error(&app(None, "Discord.exe", 3), 9).is_none());
        assert!(assign_error(&app(None, "Discord", 3), 9).is_some());
        assert!(assign_error(&app(None, r"C:\x\Discord.exe", 3), 9).is_some());
        assert!(assign_error(&app(None, "Discord.exe", 10), 9).is_some());
    }
}
