//! Command mode's catalog: the host's commands (lattice's window-manager commands) and the
//! system ones (power, audio output, Wi-Fi), fuzzy-matched against what's typed. Pure Rust: the
//! Windows side that reads the devices and runs the actions is `control`.
//!
//! Arguments are entries of their own ("Go to workspace 3", "Layout: Monocle"), so typing
//! `move 3` or `layout mono` finds them with the same fuzzy match as everything else.

use std::sync::Arc;

use super::rank;

/// A command the host offers in command mode.
#[derive(Clone)]
pub struct HostCommand {
    pub title: String,
    pub detail: String,
    /// Other words it should be found by ("kill" for closing a window).
    pub aliases: Vec<String>,
    /// It acts on the window that had the focus before the launcher opened, so that window
    /// gets the focus back first.
    pub targets_window: bool,
    pub run: Arc<dyn Fn() + Send + Sync>,
}

/// Command mode's part of the launcher's options.
#[derive(Clone, Default)]
pub struct CommandOptions {
    /// Typed before a space to enter command mode; empty turns command mode off.
    pub prefix: String,
    pub host: Vec<HostCommand>,
}

/// What the system can do right now, read when command mode opens.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub sleep: bool,
    pub hibernate: bool,
    pub audio: Vec<AudioOutput>,
    /// None without a Wi-Fi radio.
    pub wifi: Option<Wifi>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioOutput {
    /// The MMDevice endpoint id.
    pub id: String,
    pub name: String,
    pub default: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Wifi {
    pub on: bool,
    /// Saved networks, in range ones when Windows lets us see them.
    pub networks: Vec<Network>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Network {
    /// The interface's GUID, as a u128.
    pub interface: u128,
    /// The saved profile's name (usually the SSID).
    pub profile: String,
    pub connected: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum System {
    Lock,
    Sleep,
    Hibernate,
    Restart,
    ShutDown,
    SignOut,
    AudioOutput(String),
    Wifi(bool),
    Connect { interface: u128, profile: String },
}

impl System {
    /// Restart, shut down and sign out take a second Enter, so a stray one can't end the session.
    pub fn needs_confirm(&self) -> bool {
        matches!(self, System::Restart | System::ShutDown | System::SignOut)
    }
}

#[derive(Clone)]
pub enum Action {
    Host(Arc<dyn Fn() + Send + Sync>, bool),
    System(System),
}

#[derive(Clone)]
pub struct Entry {
    /// Stable across rebuilds of the catalog, to keep the highlight and a pending confirm.
    pub key: String,
    pub title: String,
    pub detail: String,
    pub aliases: Vec<String>,
    pub action: Action,
}

impl Entry {
    fn system(key: &str, title: &str, detail: &str, aliases: &[&str], sys: System) -> Self {
        Entry {
            key: key.into(),
            title: title.into(),
            detail: detail.into(),
            aliases: aliases.iter().map(|a| a.to_string()).collect(),
            action: Action::System(sys),
        }
    }

    pub fn needs_confirm(&self) -> bool {
        matches!(&self.action, Action::System(s) if s.needs_confirm())
    }
}

/// Every command, in the order an empty search lists them.
pub fn catalog(host: &[HostCommand], sys: &Snapshot) -> Vec<Entry> {
    let mut out = vec![Entry::system("lock", "Lock", "Lock the PC", &["lock screen"], System::Lock)];
    if sys.sleep {
        out.push(Entry::system("sleep", "Sleep", "Put the PC to sleep", &["suspend", "standby"], System::Sleep));
    }
    if sys.hibernate {
        out.push(Entry::system("hibernate", "Hibernate", "Save the session to disk and power off", &[], System::Hibernate));
    }
    if let Some(w) = &sys.wifi {
        let (title, detail) = if w.on { ("Turn Wi-Fi off", "Wi-Fi is on") } else { ("Turn Wi-Fi on", "Wi-Fi is off") };
        out.push(Entry::system("wifi", title, detail, &["wifi", "wireless", "wlan", "toggle wifi"], System::Wifi(!w.on)));
    }
    for d in &sys.audio {
        out.push(Entry::system(
            &format!("audio-{}", d.id),
            &format!("Audio output: {}", d.name),
            if d.default { "Current output" } else { "Play sound through this device" },
            &["sound", "speakers", "headphones", "output device"],
            System::AudioOutput(d.id.clone()),
        ));
    }
    for (i, h) in host.iter().enumerate() {
        out.push(Entry {
            key: format!("host-{i}-{}", h.title),
            title: h.title.clone(),
            detail: h.detail.clone(),
            aliases: h.aliases.clone(),
            action: Action::Host(h.run.clone(), h.targets_window),
        });
    }
    if let Some(w) = sys.wifi.as_ref().filter(|w| w.on) {
        for n in &w.networks {
            out.push(Entry::system(
                &format!("wifi-{:x}-{}", n.interface, n.profile),
                &format!("Connect to Wi-Fi: {}", n.profile),
                if n.connected { "Connected" } else { "Saved network" },
                &["wifi", "wireless", "network"],
                System::Connect { interface: n.interface, profile: n.profile.clone() },
            ));
        }
    }
    out.push(Entry::system("restart", "Restart", "Restart the PC (asks to confirm)", &["reboot"], System::Restart));
    out.push(Entry::system("shutdown", "Shut down", "Power off the PC (asks to confirm)", &["power off", "turn off"], System::ShutDown));
    out.push(Entry::system("signout", "Sign out", "End your session (asks to confirm)", &["log off", "log out", "logout"], System::SignOut));
    out
}

/// Below a title match of the same strength: a hit on an alias is a weaker hint.
const ALIAS_PENALTY: i32 = 40;

/// The entries matching `query`, best first (ties in catalog order), as (index, matched byte
/// offsets in the title). A match on an alias highlights nothing.
pub fn rank(query: &str, entries: &[Entry], limit: usize) -> Vec<(usize, Vec<usize>)> {
    let mut scored: Vec<(i32, usize, Vec<usize>)> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let title = rank::fuzzy(query, &e.title).map(|m| (m.score, m.positions));
            let alias = e
                .aliases
                .iter()
                .filter_map(|a| rank::fuzzy(query, a))
                .map(|m| (m.score - ALIAS_PENALTY, Vec::new()))
                .max_by_key(|(s, _)| *s);
            let (score, positions) = match (title, alias) {
                (Some(t), Some(a)) if a.0 > t.0 => a,
                (Some(t), _) => t,
                (None, a) => a?,
            };
            Some((score, i, positions))
        })
        .collect();
    scored.sort_by_key(|&(score, ..)| std::cmp::Reverse(score));
    scored.truncate(limit);
    scored.into_iter().map(|(_, i, p)| (i, p)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(title: &str) -> HostCommand {
        HostCommand { title: title.into(), detail: String::new(), aliases: Vec::new(), targets_window: false, run: Arc::new(|| {}) }
    }

    fn sample() -> Vec<Entry> {
        let mut h = vec![host("Layout: Dwindle"), host("Layout: Master"), host("Layout: Monocle")];
        for n in 1..=5 {
            h.push(host(&format!("Go to workspace {n}")));
        }
        for n in 1..=5 {
            h.push(host(&format!("Move window to workspace {n}")));
        }
        h.push(host("Close window"));
        let sys = Snapshot {
            sleep: true,
            hibernate: false,
            audio: vec![AudioOutput { id: "{a}".into(), name: "Headphones".into(), default: false }],
            wifi: Some(Wifi { on: true, networks: vec![Network { interface: 1, profile: "Home".into(), connected: true }] }),
        };
        catalog(&h, &sys)
    }

    fn top(query: &str) -> String {
        let e = sample();
        rank(query, &e, 8).first().map(|&(i, _)| e[i].title.clone()).unwrap_or_default()
    }

    #[test]
    fn inline_arguments_find_expanded_entries() {
        assert_eq!(top("move 3"), "Move window to workspace 3");
        assert_eq!(top("layout monocle"), "Layout: Monocle");
        assert_eq!(top("layout mas"), "Layout: Master");
        assert_eq!(top("workspace 4"), "Go to workspace 4");
        assert_eq!(top("shut"), "Shut down");
        assert_eq!(top("ws 2"), "Go to workspace 2");
        assert_eq!(top("layout mono"), "Layout: Monocle");
    }

    #[test]
    fn aliases_match_without_highlight() {
        let e = sample();
        let hits = rank("reboot", &e, 8);
        assert_eq!(e[hits[0].0].title, "Restart");
        assert!(hits[0].1.is_empty());
        assert_eq!(top("log off"), "Sign out");
        assert_eq!(top("headphones"), "Audio output: Headphones");
    }

    #[test]
    fn empty_query_lists_in_order() {
        let e = sample();
        let hits = rank("", &e, 3);
        assert_eq!(hits.iter().map(|h| h.0).collect::<Vec<_>>(), vec![0, 1, 2]);
        assert_eq!(e[0].title, "Lock");
    }

    #[test]
    fn availability_shapes_the_catalog() {
        let e = catalog(&[], &Snapshot::default());
        let titles: Vec<&str> = e.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(titles, vec!["Lock", "Restart", "Shut down", "Sign out"]);
        let off = Snapshot { wifi: Some(Wifi { on: false, networks: vec![Network { interface: 1, profile: "x".into(), connected: false }] }), ..Default::default() };
        let titles: Vec<String> = catalog(&[], &off).into_iter().map(|e| e.title).collect();
        assert!(titles.contains(&"Turn Wi-Fi on".to_string()));
        assert!(!titles.iter().any(|t| t.starts_with("Connect")), "no networks to join with the radio off");
    }

    #[test]
    fn only_session_enders_confirm() {
        let e = sample();
        let confirm: Vec<&str> = e.iter().filter(|e| e.needs_confirm()).map(|e| e.title.as_str()).collect();
        assert_eq!(confirm, vec!["Restart", "Shut down", "Sign out"]);
    }
}
