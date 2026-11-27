//! Configuration: TOML at %APPDATA%\lattice\config.toml, written from the built-in default on
//! first run. Keybinds and commands are parsed here so a bad entry is reported once, at load.

use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use crate::layout::LayoutKind;

pub const DEFAULT_CONFIG: &str = include_str!("default_config.toml");

#[derive(Deserialize, Clone, Debug)]
pub struct Config {
    #[serde(default)]
    pub general: General,
    #[serde(default)]
    pub bar: Bar,
    #[serde(default)]
    pub border: Border,
    #[serde(default)]
    pub launcher: Launcher,
    #[serde(default)]
    pub animations: Animations,
    #[serde(default)]
    pub rules: Vec<Rule>,
    #[serde(default = "default_keybinds")]
    pub keybinds: BTreeMap<String, String>,
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NewWindow {
    AfterFocused,
    End,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(default)]
pub struct General {
    pub layout: LayoutKind,
    pub workspaces: usize,
    pub gaps_inner: i32,
    pub gaps_outer: i32,
    pub split_ratio: f32,
    pub master_ratio: f32,
    pub new_window: NewWindow,
    pub float_unresizable: bool,
    pub follow_activation: bool,
    pub mouse_follows_focus: bool,
    /// Look for a newer release on GitHub once a day (the installed copy only).
    pub check_for_updates: bool,
}

impl Default for General {
    fn default() -> Self {
        Self {
            layout: LayoutKind::Dwindle,
            workspaces: 9,
            gaps_inner: 8,
            gaps_outer: 10,
            split_ratio: 0.5,
            master_ratio: 0.55,
            new_window: NewWindow::AfterFocused,
            float_unresizable: true,
            follow_activation: true,
            mouse_follows_focus: false,
            check_for_updates: true,
        }
    }
}

#[derive(Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Bar {
    pub enabled: bool,
    pub height: f32,
    pub font_size: f32,
    /// Font family for the bar (any installed font); empty uses the built-in Inter.
    pub font: String,
    pub clock_format: String,
    pub show_title: bool,
    pub show_cpu: bool,
    pub show_memory: bool,
    pub show_battery: bool,
    pub show_empty_workspaces: bool,
    /// How apps on a workspace are shown: "icons", "names" or "both".
    pub app_display: String,
    pub background: String,
    pub foreground: String,
    pub muted: String,
    pub accent: String,
    pub occupied: String,
}

impl Default for Bar {
    fn default() -> Self {
        Self {
            enabled: true,
            height: 30.0,
            font_size: 13.0,
            font: String::new(),
            clock_format: "%a %b %d  %H:%M".into(),
            show_title: true,
            show_cpu: true,
            show_memory: true,
            show_battery: true,
            show_empty_workspaces: false,
            app_display: "icons".into(),
            background: "#1e1e2e".into(),
            foreground: "#cdd6f4".into(),
            muted: "#7f849c".into(),
            accent: "#89b4fa".into(),
            occupied: "#313244".into(),
        }
    }
}

/// The glowing frame drawn around the focused window.
#[derive(Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Border {
    pub enabled: bool,
    /// Line width, logical pixels.
    pub width: f32,
    /// Corner radius; Windows 11 windows use 8.
    pub radius: f32,
    pub color: String,
    /// Glow blur size in logical pixels; 0 draws a flat line.
    pub glow: f32,
    /// Glow strength, 0-1.
    pub glow_opacity: f32,
}

impl Default for Border {
    fn default() -> Self {
        Self { enabled: true, width: 2.0, radius: 8.0, color: "#89b4fa".into(), glow: 14.0, glow_opacity: 0.55 }
    }
}

impl Border {
    /// How far the overlay extends past the window on each side, in logical pixels.
    pub fn margin(&self) -> f32 {
        if self.glow > 0.0 { self.glow * 1.5 } else { 1.0 }
    }
}

/// How animations look: "fancy" (slower, eased with a little overshoot, workspaces cascade in)
/// or "fast" (short and plain).
#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AnimStyle {
    Fancy,
    Fast,
}

/// Window animations (see `anim.rs`). They apply on reload: no restart.
#[derive(Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Animations {
    pub enabled: bool,
    pub style: AnimStyle,
}

impl Default for Animations {
    fn default() -> Self {
        Self { enabled: false, style: AnimStyle::Fancy }
    }
}

impl Animations {
    /// The style to animate with, or None when animations are off.
    pub fn active(&self) -> Option<AnimStyle> {
        self.enabled.then_some(self.style)
    }
}

/// The app launcher (`launcher` command). Its colours and font follow the bar's.
#[derive(Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Launcher {
    pub enabled: bool,
    /// Window width, logical pixels.
    pub width: f32,
    /// Result rows shown at once.
    pub max_results: usize,
    /// Typed before a space to search files and folders instead of apps.
    pub file_prefix: String,
    /// Typed before a space to search open windows instead of apps.
    pub window_prefix: String,
    /// Typed before a space to run commands (layouts, workspaces, power, audio, Wi-Fi).
    pub command_prefix: String,
    /// What Enter and Ctrl+Enter do with a file, and with a folder: "open", "reveal",
    /// "copy-path" or "exec <command line>" (with {path}, {dir}, {name}).
    pub file_enter: String,
    pub file_ctrl_enter: String,
    pub folder_enter: String,
    pub folder_ctrl_enter: String,
    /// Folders lattice indexes itself when Everything isn't running; empty means your user folder.
    pub index_roots: Vec<String>,
    /// Folder names that index skips, anywhere.
    pub index_exclude: Vec<String>,
}

impl Default for Launcher {
    fn default() -> Self {
        Self {
            enabled: true,
            width: 680.0,
            max_results: 8,
            file_prefix: "f".into(),
            window_prefix: "w".into(),
            command_prefix: ">".into(),
            file_enter: "open".into(),
            file_ctrl_enter: "reveal".into(),
            folder_enter: "open".into(),
            folder_ctrl_enter: "copy-path".into(),
            index_roots: Vec::new(),
            index_exclude: default_index_exclude(),
        }
    }
}

pub fn default_index_exclude() -> Vec<String> {
    [
        "node_modules", ".git", ".svn", ".hg", "target", "__pycache__", ".venv", "venv", ".cache", ".cargo",
        ".rustup", ".npm", ".nuget", ".gradle", ".m2", "AppData", "$Recycle.Bin",
    ]
    .map(String::from)
    .to_vec()
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct Rule {
    pub exe: Option<String>,
    pub class: Option<String>,
    pub title: Option<String>,
    #[serde(default)]
    pub float: bool,
    #[serde(default)]
    pub ignore: bool,
    pub workspace: Option<usize>,
    /// Force a redraw after lattice shows the window (for apps that come back black).
    #[serde(default)]
    pub repaint: bool,
}

impl Rule {
    pub fn matches(&self, exe: &str, class: &str, title: &str) -> bool {
        if self.exe.is_none() && self.class.is_none() && self.title.is_none() {
            return false;
        }
        let eq = |want: &Option<String>, have: &str| want.as_ref().is_none_or(|w| w.eq_ignore_ascii_case(have));
        let title_ok = self
            .title
            .as_ref()
            .is_none_or(|t| title.to_lowercase().contains(&t.to_lowercase()));
        eq(&self.exe, exe) && eq(&self.class, class) && title_ok
    }
}

/// The workspace (1-based, as written) the first matching rule with `workspace` pins a window to.
pub fn rule_workspace(rules: &[Rule], exe: &str, class: &str, title: &str) -> Option<usize> {
    rules.iter().filter(|r| r.workspace.is_some()).find(|r| r.matches(exe, class, title)).and_then(|r| r.workspace)
}

fn default_keybinds() -> BTreeMap<String, String> {
    #[derive(Deserialize)]
    struct OnlyKeys {
        keybinds: BTreeMap<String, String>,
    }
    toml::from_str::<OnlyKeys>(DEFAULT_CONFIG).expect("built-in keybinds parse").keybinds
}

impl Default for Config {
    fn default() -> Self {
        toml::from_str(DEFAULT_CONFIG).expect("built-in config parses")
    }
}

pub fn config_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    base.join("lattice")
}

pub fn config_path() -> PathBuf {
    if let Some(p) = std::env::var_os("LATTICE_CONFIG") {
        return PathBuf::from(p);
    }
    config_dir().join("config.toml")
}

/// Loads the config, writing the default file first if none exists. Errors fall back to the
/// built-in defaults and are logged, so a typo never leaves you without keybinds.
pub fn load() -> Config {
    let path = config_path();
    if !path.exists() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match std::fs::write(&path, DEFAULT_CONFIG) {
            Ok(()) => log::info!("wrote default config to {}", path.display()),
            Err(e) => log::warn!("could not write default config to {}: {e}", path.display()),
        }
    }
    let mut cfg = match std::fs::read_to_string(&path) {
        Ok(text) => match toml::from_str::<Config>(&text) {
            Ok(c) => c,
            Err(e) => {
                log::error!("config error in {}: {e}; using built-in defaults", path.display());
                Config::default()
            }
        },
        Err(e) => {
            log::error!("could not read {}: {e}; using built-in defaults", path.display());
            Config::default()
        }
    };
    let g = &mut cfg.general;
    g.workspaces = g.workspaces.clamp(1, 10);
    g.split_ratio = g.split_ratio.clamp(0.1, 0.9);
    g.master_ratio = g.master_ratio.clamp(0.1, 0.9);
    g.gaps_inner = g.gaps_inner.max(0);
    g.gaps_outer = g.gaps_outer.max(0);
    cfg.bar.height = cfg.bar.height.clamp(16.0, 80.0);
    let b = &mut cfg.border;
    b.width = b.width.clamp(0.0, 20.0);
    b.radius = b.radius.clamp(0.0, 40.0);
    b.glow = b.glow.clamp(0.0, 60.0);
    b.glow_opacity = b.glow_opacity.clamp(0.0, 1.0);
    let l = &mut cfg.launcher;
    l.width = l.width.clamp(360.0, 1600.0);
    l.max_results = l.max_results.clamp(1, 20);
    cfg
}

// ---------------------------------------------------------------------------------------------
// Commands

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WsTarget {
    Index(usize),
    Next,
    Prev,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Focus(Dir),
    MoveWindow(Dir),
    Workspace(WsTarget),
    MoveToWorkspace { target: WsTarget, follow: bool },
    FocusMonitor(i32),
    MoveToMonitor(i32),
    Layout(Option<LayoutKind>),
    /// Expand the focused window toward a side by a ratio step (see `layout::resize_target`).
    ResizeDir(Dir, f32),
    ToggleFloat,
    ToggleFullscreen,
    Close,
    Retile,
    Reload,
    Exec(String),
    /// Show or hide the app launcher.
    Launcher,
    /// Show or hide the launcher searching every open window.
    WindowSearch,
    /// Switch to this window, its workspace and monitor (sent by the launcher, not bindable).
    FocusWindow(crate::win::Hwnd),
    /// Hand the launcher the window list (sent by it, for the window prefix; not bindable).
    ListWindows,
    /// Open the settings window.
    Settings,
    /// Open config.toml in a text editor.
    OpenConfig,
    /// Start the product tour, or end the one showing.
    Tour,
    Quit,
}

fn parse_dir(s: &str) -> Result<Dir, String> {
    match s {
        "left" | "l" | "west" => Ok(Dir::Left),
        "right" | "r" | "east" => Ok(Dir::Right),
        "up" | "u" | "north" => Ok(Dir::Up),
        "down" | "d" | "south" => Ok(Dir::Down),
        _ => Err(format!("unknown direction '{s}'")),
    }
}

fn parse_ws(s: &str) -> Result<WsTarget, String> {
    match s {
        "next" | "+1" => Ok(WsTarget::Next),
        "prev" | "previous" | "-1" => Ok(WsTarget::Prev),
        n => match n.parse::<usize>() {
            Ok(i) if i >= 1 => Ok(WsTarget::Index(i - 1)),
            _ => Err(format!("bad workspace '{n}'")),
        },
    }
}

fn parse_step(s: &str) -> Result<i32, String> {
    match s {
        "next" | "" => Ok(1),
        "prev" | "previous" => Ok(-1),
        _ => Err(format!("expected next|prev, got '{s}'")),
    }
}

pub fn parse_command(s: &str) -> Result<Option<Command>, String> {
    let s = s.trim();
    let (head, rest) = s.split_once(char::is_whitespace).unwrap_or((s, ""));
    let rest = rest.trim();
    let arg = rest.to_ascii_lowercase();
    let cmd = match head.to_ascii_lowercase().as_str() {
        "none" | "" => return Ok(None),
        "focus" => Command::Focus(parse_dir(&arg)?),
        "move-window" | "move" | "swap" => Command::MoveWindow(parse_dir(&arg)?),
        "workspace" => Command::Workspace(parse_ws(&arg)?),
        "move-to-workspace" => Command::MoveToWorkspace { target: parse_ws(&arg)?, follow: true },
        "move-to-workspace-silent" => Command::MoveToWorkspace { target: parse_ws(&arg)?, follow: false },
        "focus-monitor" => Command::FocusMonitor(parse_step(&arg)?),
        "move-to-monitor" => Command::MoveToMonitor(parse_step(&arg)?),
        "layout" => Command::Layout(match arg.as_str() {
            "next" | "" => None,
            other => Some(LayoutKind::parse(other).ok_or_else(|| format!("unknown layout '{other}'"))?),
        }),
        "resize" => {
            let (first, amount) = arg.split_once(char::is_whitespace).unwrap_or((&arg, ""));
            let dir = parse_dir(first).map_err(|_| format!("resize takes left|right|up|down [amount], got '{arg}'"))?;
            Command::ResizeDir(dir, match amount.trim() {
                "" => 0.05,
                a => a.parse::<f32>().ok().filter(|a| *a > 0.0).ok_or_else(|| format!("bad resize amount '{a}'"))?,
            })
        }
        "toggle-float" | "togglefloating" => Command::ToggleFloat,
        "toggle-fullscreen" | "fullscreen" => Command::ToggleFullscreen,
        "close" | "kill" => Command::Close,
        "retile" => Command::Retile,
        "reload" => Command::Reload,
        "launcher" | "launch" => Command::Launcher,
        "window-search" | "windows" => Command::WindowSearch,
        "settings" => Command::Settings,
        "tour" => Command::Tour,
        "open-config" | "edit-config" => Command::OpenConfig,
        "quit" | "exit" => Command::Quit,
        "exec" => {
            if rest.is_empty() {
                return Err("exec needs a command line".into());
            }
            Command::Exec(rest.to_string())
        }
        other => return Err(format!("unknown command '{other}'")),
    };
    Ok(Some(cmd))
}

// ---------------------------------------------------------------------------------------------
// Key chords

pub const MOD_SUPER: u8 = 1;
pub const MOD_SHIFT: u8 = 2;
pub const MOD_CTRL: u8 = 4;
pub const MOD_ALT: u8 = 8;

fn key_vk(name: &str) -> Option<u32> {
    let n = name.to_ascii_lowercase();
    if n.len() == 1 {
        let c = n.as_bytes()[0];
        if c.is_ascii_lowercase() {
            return Some(c.to_ascii_uppercase() as u32);
        }
        if c.is_ascii_digit() {
            return Some(c as u32);
        }
    }
    if let Some(num) = n.strip_prefix('f').and_then(|d| d.parse::<u32>().ok())
        && (1..=24).contains(&num)
    {
        return Some(0x70 + num - 1);
    }
    Some(match n.as_str() {
        "return" | "enter" => 0x0D,
        "space" => 0x20,
        "tab" => 0x09,
        "escape" | "esc" => 0x1B,
        "backspace" => 0x08,
        "delete" | "del" => 0x2E,
        "insert" | "ins" => 0x2D,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" | "prior" => 0x21,
        "pagedown" | "next" => 0x22,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "semicolon" => 0xBA,
        "equal" | "plus" => 0xBB,
        "comma" => 0xBC,
        "minus" => 0xBD,
        "period" => 0xBE,
        "slash" => 0xBF,
        "grave" | "backtick" => 0xC0,
        "bracketleft" => 0xDB,
        "backslash" => 0xDC,
        "bracketright" => 0xDD,
        "quote" | "apostrophe" => 0xDE,
        _ => return None,
    })
}

pub fn parse_chord(s: &str) -> Result<(u8, u32), String> {
    let mut mods = 0u8;
    let mut key = None;
    for part in s.split('+').map(str::trim) {
        match part.to_ascii_lowercase().as_str() {
            "super" | "win" | "mod4" | "meta" => mods |= MOD_SUPER,
            "shift" => mods |= MOD_SHIFT,
            "ctrl" | "control" => mods |= MOD_CTRL,
            "alt" => mods |= MOD_ALT,
            other => {
                if key.is_some() {
                    return Err(format!("'{s}' has more than one non-modifier key"));
                }
                key = Some(key_vk(other).ok_or_else(|| format!("unknown key '{other}' in '{s}'"))?);
            }
        }
    }
    let key = key.ok_or_else(|| format!("'{s}' has no key"))?;
    if mods == 0 {
        return Err(format!("'{s}' needs at least one modifier"));
    }
    Ok((mods, key))
}

/// Chord -> command table used by the keyboard hook.
pub fn build_keymap(cfg: &Config) -> HashMap<(u8, u32), Command> {
    let mut map = HashMap::new();
    for (chord, cmd) in &cfg.keybinds {
        let parsed = parse_chord(chord).and_then(|c| parse_command(cmd).map(|cmd| (c, cmd)));
        match parsed {
            Ok((c, Some(cmd))) => {
                map.insert(c, cmd);
            }
            Ok((_, None)) => {}
            Err(e) => log::error!("keybind \"{chord}\" = \"{cmd}\": {e}"),
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_parses_and_all_binds_are_valid() {
        let cfg = Config::default();
        for (chord, cmd) in &cfg.keybinds {
            parse_chord(chord).unwrap_or_else(|e| panic!("{e}"));
            parse_command(cmd).unwrap_or_else(|e| panic!("{e}"));
        }
        assert_eq!(build_keymap(&cfg).len(), cfg.keybinds.len());
    }

    #[test]
    fn chords() {
        assert_eq!(parse_chord("super+shift+1"), Ok((MOD_SUPER | MOD_SHIFT, b'1' as u32)));
        assert_eq!(parse_chord("Win+Return"), Ok((MOD_SUPER, 0x0D)));
        assert!(parse_chord("q").is_err());
    }

    #[test]
    fn window_search_command() {
        assert_eq!(parse_command("window-search"), Ok(Some(Command::WindowSearch)));
        assert_eq!(parse_command("windows"), Ok(Some(Command::WindowSearch)));
        let cfg = Config::default();
        assert_eq!(build_keymap(&cfg).get(&parse_chord("super+j").unwrap()), Some(&Command::WindowSearch));
    }

    #[test]
    fn resize_commands() {
        assert_eq!(parse_command("resize right"), Ok(Some(Command::ResizeDir(Dir::Right, 0.05))));
        assert_eq!(parse_command("resize up 0.1"), Ok(Some(Command::ResizeDir(Dir::Up, 0.1))));
        assert!(parse_command("resize grow").is_err());
        assert!(parse_command("resize -0.1").is_err());
        assert!(parse_command("resize left -1").is_err());
        assert!(parse_command("resize sideways").is_err());
    }

    #[test]
    fn animations_default_off_and_parse() {
        assert_eq!(Config::default().animations.active(), None);
        let on = toml::from_str::<Config>("[animations]\nenabled = true\nstyle = \"fast\"").unwrap();
        assert_eq!(on.animations.active(), Some(AnimStyle::Fast));
        let style_only = toml::from_str::<Config>("[animations]\nenabled = true").unwrap();
        assert_eq!(style_only.animations.active(), Some(AnimStyle::Fancy));
        assert!(toml::from_str::<Config>("[animations]\nstyle = \"wobbly\"").is_err());
    }

    #[test]
    fn rules() {
        let r = Rule { exe: Some("calc.exe".into()), ..Default::default() };
        assert!(r.matches("Calc.exe", "x", "y"));
        assert!(!r.matches("notepad.exe", "x", "y"));
    }

    #[test]
    fn first_workspace_rule_wins() {
        let rules = toml::from_str::<Config>(
            r#"
            [[rules]]
            exe = "Discord.exe"
            float = true
            [[rules]]
            exe = "discord.exe"
            title = "updater"
            workspace = 5
            [[rules]]
            exe = "Discord.exe"
            workspace = 3
            "#,
        )
        .unwrap()
        .rules;
        assert_eq!(rule_workspace(&rules, "Discord.exe", "Chrome_WidgetWin_1", "#general - Discord"), Some(3));
        assert_eq!(rule_workspace(&rules, "Discord.exe", "x", "Discord Updater"), Some(5));
        assert_eq!(rule_workspace(&rules, "notepad.exe", "x", "y"), None);
    }
}
