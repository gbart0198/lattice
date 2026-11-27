//! Events for scripts: `[[hook]]` commands from the config, and `lattice subscribe` streams.
//! (Not to be confused with `hooks.rs`, which holds lattice's Win32 hooks.)
//!
//! The WM thread doesn't announce events as it goes: once per batch of work it diffs a
//! snapshot of its state against the last one (`Tracker`), so every path that switches a
//! workspace or moves a window is covered and a burst of changes yields one event each. The
//! events then go to one dispatcher thread, which starts hook processes and feeds subscribers:
//! a slow script or a stuck subscriber never holds up tiling.

use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use crate::config::Hook;
use crate::ipc::{State, WindowState};
use crate::win::Hwnd;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Startup,
    ConfigReloaded,
    WorkspaceChanged,
    LayoutChanged,
    FocusChanged,
    WindowOpened,
    WindowClosed,
    WindowMoved,
    MonitorChanged,
    DisplayChanged,
    SetupRestored,
}

const KINDS: [(Kind, &str); 11] = [
    (Kind::Startup, "startup"),
    (Kind::ConfigReloaded, "config-reloaded"),
    (Kind::WorkspaceChanged, "workspace-changed"),
    (Kind::LayoutChanged, "layout-changed"),
    (Kind::FocusChanged, "focus-changed"),
    (Kind::WindowOpened, "window-opened"),
    (Kind::WindowClosed, "window-closed"),
    (Kind::WindowMoved, "window-moved"),
    (Kind::MonitorChanged, "monitor-changed"),
    (Kind::DisplayChanged, "display-changed"),
    (Kind::SetupRestored, "setup-restored"),
];

pub const NAMES: &str = "startup, config-reloaded, workspace-changed, layout-changed, focus-changed, window-opened, \
                         window-closed, window-moved, monitor-changed, display-changed, setup-restored";

impl Kind {
    pub fn name(self) -> &'static str {
        KINDS.iter().find(|(k, _)| *k == self).map(|(_, n)| *n).expect("every kind is named")
    }

    pub fn parse(s: &str) -> Option<Kind> {
        let s = s.trim().to_ascii_lowercase();
        KINDS.iter().find(|(_, n)| *n == s).map(|(k, _)| *k)
    }
}

/// One event: its kind and flat details. As JSON each detail is a key; as environment
/// variables, `LATTICE_<KEY>` (and `LATTICE_EVENT` for the name).
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub kind: Kind,
    fields: Map<String, Value>,
}

impl Event {
    pub fn new(kind: Kind) -> Self {
        Event { kind, fields: Map::new() }
    }

    pub fn with(mut self, key: &str, v: impl Into<Value>) -> Self {
        self.fields.insert(key.into(), v.into());
        self
    }

    /// A setup restore has finished (`wm.rs` `check_restore`): `restore` is the id
    /// `lattice msg restore-setup` was answered with, `missing` the apps (by exe, one per spot)
    /// whose window never took its spot, `folded` the setup's monitors (from 1) that aren't
    /// connected and went on the primary.
    pub fn setup_restored(restore: u64, name: &str, in_place: usize, started: usize, missing: &[String], folded: &[usize]) -> Self {
        Event::new(Kind::SetupRestored)
            .with("restore", restore)
            .with("name", name)
            .with("complete", missing.is_empty())
            .with("in_place", in_place)
            .with("started", started)
            .with("missing", missing)
            .with("folded", folded)
    }

    fn window(self, w: &WindowState) -> Self {
        self.with("hwnd", w.hwnd)
            .with("title", w.title.as_str())
            .with("exe", w.exe.as_str())
            .with("class", w.class.as_str())
            .with("workspace", w.workspace)
            .with("monitor", w.monitor)
            .with("floating", w.floating)
    }

    pub fn to_json(&self) -> Value {
        let mut m = self.fields.clone();
        m.insert("event".into(), self.kind.name().into());
        Value::Object(m)
    }

    /// The details as environment variables for a hook.
    pub fn env(&self) -> Vec<(String, String)> {
        let mut v = vec![("LATTICE_EVENT".to_string(), self.kind.name().to_string())];
        v.extend(self.fields.iter().map(|(k, val)| {
            let text = match val {
                Value::String(s) => s.clone(),
                Value::Null => String::new(),
                // Lists as words, easy to `for` over in cmd: `Code.exe slack.exe`.
                Value::Array(a) => a.iter().map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_string)).collect::<Vec<_>>().join(" "),
                other => other.to_string(),
            };
            (format!("LATTICE_{}", k.to_ascii_uppercase()), text)
        }));
        v
    }
}

/// What changed between two snapshots, in the order a script would expect: windows coming and
/// going first, then where you are, then what has focus.
pub fn diff(prev: &State, next: &State) -> Vec<Event> {
    let mut out = Vec::new();
    let before: HashMap<Hwnd, &WindowState> = prev.windows.iter().map(|w| (w.hwnd, w)).collect();
    let after: HashMap<Hwnd, &WindowState> = next.windows.iter().map(|w| (w.hwnd, w)).collect();
    for w in &prev.windows {
        if !after.contains_key(&w.hwnd) {
            out.push(Event::new(Kind::WindowClosed).window(w));
        }
    }
    for w in &next.windows {
        match before.get(&w.hwnd) {
            None => out.push(Event::new(Kind::WindowOpened).window(w)),
            Some(old) if old.workspace != w.workspace || old.monitor != w.monitor => out.push(
                Event::new(Kind::WindowMoved)
                    .window(w)
                    .with("previous_workspace", old.workspace)
                    .with("previous_monitor", old.monitor),
            ),
            Some(_) => {}
        }
    }
    if prev.active_workspace != next.active_workspace {
        out.push(
            Event::new(Kind::WorkspaceChanged)
                .with("workspace", next.active_workspace)
                .with("previous_workspace", prev.active_workspace),
        );
    }
    let layout_of = |s: &State| s.workspaces.iter().find(|w| w.index == s.active_workspace).map(|w| w.layout);
    if prev.active_workspace == next.active_workspace && layout_of(prev) != layout_of(next) {
        // Only the workspace on screen: a reload resets every layout, but you see one.
        out.push(
            Event::new(Kind::LayoutChanged)
                .with("workspace", next.active_workspace)
                .with("layout", layout_of(next))
                .with("previous_layout", layout_of(prev)),
        );
    }
    let screens = |s: &State| s.monitors.iter().map(|m| (m.rect, m.primary)).collect::<Vec<_>>();
    if screens(prev) != screens(next) {
        out.push(Event::new(Kind::DisplayChanged).with("monitors", next.monitors.len()));
    }
    if prev.focused_monitor != next.focused_monitor {
        out.push(
            Event::new(Kind::MonitorChanged)
                .with("monitor", next.focused_monitor)
                .with("previous_monitor", prev.focused_monitor)
                .with("workspace", next.active_workspace),
        );
    }
    if let Some(w) = next.focused_window.filter(|h| prev.focused_window != Some(*h)).and_then(|h| next.window(h)) {
        out.push(Event::new(Kind::FocusChanged).window(w));
    }
    out
}

/// The WM's last snapshot, for `diff`.
#[derive(Default)]
pub struct Tracker {
    last: Option<State>,
}

impl Tracker {
    /// Events since the last call (none on the first: that only sets the baseline).
    pub fn update(&mut self, mut next: State) -> Vec<Event> {
        let Some(prev) = self.last.take() else {
            self.last = Some(next);
            return Vec::new();
        };
        let events = diff(&prev, &next);
        // Focus passing through the bar or the launcher and back isn't a change of focus.
        if next.focused_window.is_none() {
            next.focused_window = prev.focused_window.filter(|h| next.window(*h).is_some());
        }
        self.last = Some(next);
        events
    }

    /// Forget the baseline (nobody's listening, so there's nothing to keep current).
    pub fn reset(&mut self) {
        self.last = None;
    }
}

// ---------------------------------------------------------------------------------------------
// Hooks

#[derive(Clone, Debug, PartialEq)]
pub struct HookSpec {
    /// None: every event.
    pub on: Option<Vec<Kind>>,
    pub run: String,
}

impl HookSpec {
    fn wants(&self, k: Kind) -> bool {
        self.on.as_ref().is_none_or(|on| on.contains(&k))
    }
}

/// The config's hooks, checked: bad ones come back as errors and are left out.
pub fn compile(hooks: &[Hook]) -> (Vec<HookSpec>, Vec<String>) {
    let (mut ok, mut errors) = (Vec::new(), Vec::new());
    for h in hooks {
        let run = h.run.trim();
        if run.is_empty() {
            errors.push(format!("hook on {:?}: `run` is empty", h.on.names()));
            continue;
        }
        let names = h.on.names();
        if names.iter().any(|n| n.trim() == "*") {
            ok.push(HookSpec { on: None, run: run.into() });
            continue;
        }
        let mut kinds = Vec::new();
        let mut bad = false;
        for n in &names {
            match Kind::parse(n) {
                Some(k) => kinds.push(k),
                None => {
                    errors.push(format!("hook \"{run}\": unknown event '{n}' (events: {NAMES}, or \"*\")"));
                    bad = true;
                }
            }
        }
        if !bad && !kinds.is_empty() {
            ok.push(HookSpec { on: Some(kinds), run: run.into() });
        } else if kinds.is_empty() && !bad {
            errors.push(format!("hook \"{run}\": `on` names no event"));
        }
    }
    (ok, errors)
}

/// Runs a hook's command line through `cmd /C`, with the event in its environment and as one
/// line of JSON on stdin. Console programs share cmd's hidden console, so nothing flashes.
fn run_hook(run: &str, e: &Event) {
    use std::os::windows::process::CommandExt;
    use std::process::Stdio;
    let json = e.to_json().to_string();
    // `/S` with the line in quotes: cmd takes everything inside them literally.
    let child = crate::wm::background_command("cmd")
        .raw_arg(format!("/D /S /C \"{run}\""))
        .envs(e.env())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            log::error!("hook \"{run}\": {e}");
            return;
        }
    };
    let (run, name) = (run.to_string(), e.kind.name());
    // Writing and waiting on a thread of its own: a hook may take as long as it likes.
    let spawned = std::thread::Builder::new().name("lattice-hook".into()).spawn(move || {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = writeln!(stdin, "{json}");
        }
        match child.wait() {
            Ok(s) if !s.success() => log::warn!("hook \"{run}\" ({name}) exited with {s}"),
            Err(e) => log::warn!("hook \"{run}\" ({name}): {e}"),
            _ => {}
        }
    });
    if let Err(e) = spawned {
        log::warn!("hook thread: {e}");
    }
}

// ---------------------------------------------------------------------------------------------
// Dispatcher

enum Msg {
    Event(Event),
    Hooks(Vec<HookSpec>),
    Subscribe(Hwnd),
}

/// A burst of focus changes (Alt+Tab, clicking around) delivers only where focus settled.
const FOCUS_SETTLE: Duration = Duration::from_millis(100);

static HOOKS: AtomicUsize = AtomicUsize::new(0);
static SUBSCRIBERS: AtomicUsize = AtomicUsize::new(0);
static DISPATCHER: OnceLock<Sender<Msg>> = OnceLock::new();

fn dispatcher() -> &'static Sender<Msg> {
    DISPATCHER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Msg>();
        std::thread::Builder::new()
            .name("lattice-events".into())
            .spawn(move || {
                let mut hooks: Vec<HookSpec> = Vec::new();
                let mut subs: Vec<Hwnd> = Vec::new();
                let mut focus: Option<(Event, Instant)> = None;
                loop {
                    let msg = match &focus {
                        Some((_, due)) => rx.recv_timeout(due.saturating_duration_since(Instant::now())),
                        None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                    };
                    match msg {
                        Ok(Msg::Event(e)) if e.kind == Kind::FocusChanged => focus = Some((e, Instant::now() + FOCUS_SETTLE)),
                        Ok(Msg::Event(e)) => {
                            // Anything else lands after the focus change before it, in order.
                            if let Some((f, _)) = focus.take() {
                                deliver(&f, &hooks, &mut subs);
                            }
                            deliver(&e, &hooks, &mut subs);
                        }
                        Ok(Msg::Hooks(h)) => hooks = h,
                        Ok(Msg::Subscribe(h)) => subs.push(h),
                        Err(RecvTimeoutError::Timeout) => {
                            if let Some((f, _)) = focus.take() {
                                deliver(&f, &hooks, &mut subs);
                            }
                        }
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
            .expect("spawn events thread");
        tx
    })
}

fn deliver(e: &Event, hooks: &[HookSpec], subs: &mut Vec<Hwnd>) {
    for h in hooks.iter().filter(|h| h.wants(e.kind)) {
        run_hook(&h.run, e);
    }
    if subs.is_empty() {
        return;
    }
    let bytes = e.to_json().to_string().into_bytes();
    // A subscriber that's gone (or stuck for half a second) is dropped.
    subs.retain(|h| crate::ipc::send_copydata(*h, crate::ipc::KIND_EVENT, &bytes, Duration::from_millis(500)));
    SUBSCRIBERS.store(subs.len(), Ordering::Relaxed);
}

/// Anyone to tell? When not, the WM skips building snapshots.
pub fn listening() -> bool {
    HOOKS.load(Ordering::Relaxed) + SUBSCRIBERS.load(Ordering::Relaxed) > 0
}

/// Installs the config's hooks (at startup and on reload), reporting bad ones.
pub fn set_hooks(hooks: &[Hook]) {
    let (specs, errors) = compile(hooks);
    for e in errors {
        log::error!("config: {e}");
    }
    if !specs.is_empty() {
        log::info!("{} hook(s) active", specs.len());
    }
    HOOKS.store(specs.len(), Ordering::Relaxed);
    if specs.is_empty() && DISPATCHER.get().is_none() {
        return;
    }
    let _ = dispatcher().send(Msg::Hooks(specs));
}

/// A `lattice subscribe` client's window, to send every event to.
pub fn subscribe(hwnd: Hwnd) {
    SUBSCRIBERS.fetch_add(1, Ordering::Relaxed);
    let _ = dispatcher().send(Msg::Subscribe(hwnd));
}

pub fn emit(e: Event) {
    if listening() {
        let _ = dispatcher().send(Msg::Event(e));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HookOn;
    use crate::ipc::tests::sample;

    fn kinds(v: &[Event]) -> Vec<Kind> {
        v.iter().map(|e| e.kind).collect()
    }

    #[test]
    fn names_round_trip() {
        for (k, n) in KINDS {
            assert_eq!(Kind::parse(n), Some(k));
            assert_eq!(k.name(), n);
            assert!(NAMES.contains(n));
        }
        assert_eq!(Kind::parse("Workspace-Changed"), Some(Kind::WorkspaceChanged));
        assert_eq!(Kind::parse("workspace"), None);
    }

    #[test]
    fn no_change_no_events() {
        assert!(diff(&sample(), &sample()).is_empty());
    }

    #[test]
    fn workspace_switch() {
        let prev = sample();
        let mut next = sample();
        next.active_workspace = 2;
        next.focused_monitor = 1;
        next.focused_window = Some(300);
        let ev = diff(&prev, &next);
        assert_eq!(kinds(&ev), [Kind::WorkspaceChanged, Kind::MonitorChanged, Kind::FocusChanged]);
        assert_eq!(ev[0].to_json(), serde_json::json!({"event": "workspace-changed", "workspace": 2, "previous_workspace": 1}));
        assert_eq!(ev[2].to_json()["hwnd"], 300);
    }

    #[test]
    fn windows_open_close_move() {
        let prev = sample();
        let mut next = sample();
        next.windows.retain(|w| w.hwnd != 200);
        next.windows[1].workspace = 1; // 300 moves to workspace 1
        let mut new = next.windows[0].clone();
        new.hwnd = 400;
        new.focused = false;
        next.windows.push(new);
        let ev = diff(&prev, &next);
        assert_eq!(kinds(&ev), [Kind::WindowClosed, Kind::WindowMoved, Kind::WindowOpened]);
        assert_eq!(ev[0].to_json()["hwnd"], 200);
        assert_eq!(ev[0].to_json()["title"], "window 200");
        let moved = ev[1].to_json();
        assert_eq!((moved["workspace"].clone(), moved["previous_workspace"].clone()), (1.into(), 2.into()));
        assert_eq!(ev[2].to_json()["hwnd"], 400);
    }

    #[test]
    fn layout_and_display() {
        let prev = sample();
        let mut next = sample();
        next.workspaces[0].layout = "monocle";
        next.workspaces[1].layout = "master"; // off screen: not announced
        next.monitors.pop();
        let ev = diff(&prev, &next);
        assert_eq!(kinds(&ev), [Kind::LayoutChanged, Kind::DisplayChanged]);
        assert_eq!(ev[0].to_json()["layout"], "monocle");
        assert_eq!(ev[0].to_json()["previous_layout"], "dwindle");
        assert_eq!(ev[1].to_json()["monitors"], 1);
    }

    #[test]
    fn focus_through_the_bar_is_not_a_change() {
        let mut t = Tracker::default();
        assert!(t.update(sample()).is_empty(), "first snapshot is the baseline");
        let away = State { focused_window: None, ..sample() };
        assert!(t.update(away).is_empty());
        assert!(t.update(sample()).is_empty(), "back to the same window");
        let other = State { focused_window: Some(200), ..sample() };
        assert_eq!(kinds(&t.update(other)), [Kind::FocusChanged]);
    }

    #[test]
    fn env_vars() {
        let e = Event::new(Kind::WindowOpened).window(&sample().windows[0]).with("previous_workspace", Value::Null);
        let env: HashMap<String, String> = e.env().into_iter().collect();
        assert_eq!(env["LATTICE_EVENT"], "window-opened");
        assert_eq!(env["LATTICE_HWND"], "100");
        assert_eq!(env["LATTICE_TITLE"], "window 100");
        assert_eq!(env["LATTICE_EXE"], "app.exe");
        assert_eq!(env["LATTICE_WORKSPACE"], "1");
        assert_eq!(env["LATTICE_FLOATING"], "false");
        assert_eq!(env["LATTICE_PREVIOUS_WORKSPACE"], "");
    }

    #[test]
    fn setup_restored_fields() {
        let e = Event::setup_restored(3, "Dev", 1, 2, &["slack.exe".into(), "Code.exe".into()], &[2]);
        assert_eq!(
            e.to_json(),
            serde_json::json!({
                "event": "setup-restored", "restore": 3, "name": "Dev", "complete": false,
                "in_place": 1, "started": 2, "missing": ["slack.exe", "Code.exe"], "folded": [2],
            })
        );
        let env: HashMap<String, String> = e.env().into_iter().collect();
        assert_eq!(env["LATTICE_MISSING"], "slack.exe Code.exe");
        assert_eq!(env["LATTICE_FOLDED"], "2");
        assert_eq!(env["LATTICE_COMPLETE"], "false");
        let done = Event::setup_restored(4, "Dev", 3, 0, &[], &[]);
        assert_eq!(done.to_json()["complete"], true);
        assert_eq!(done.env().into_iter().find(|(k, _)| k == "LATTICE_MISSING").map(|(_, v)| v), Some(String::new()));
    }

    #[test]
    fn hooks_compile() {
        let hook = |on: HookOn, run: &str| Hook { on, run: run.into() };
        let one = |s: &str| HookOn::One(s.into());
        let (ok, errors) = compile(&[
            hook(one("workspace-changed"), "a"),
            hook(HookOn::Many(vec!["window-opened".into(), "window-closed".into()]), "b"),
            hook(one("*"), "c"),
            hook(one("workspace-change"), "typo"),
            hook(one("startup"), "  "),
            hook(HookOn::Many(vec![]), "empty"),
        ]);
        assert_eq!(ok.len(), 3);
        assert_eq!(errors.len(), 3, "{errors:?}");
        assert!(ok[0].wants(Kind::WorkspaceChanged) && !ok[0].wants(Kind::FocusChanged));
        assert!(ok[1].wants(Kind::WindowClosed));
        assert!(ok[2].wants(Kind::Startup) && ok[2].wants(Kind::DisplayChanged));
        assert!(errors[0].contains("workspace-change"));
    }
}
