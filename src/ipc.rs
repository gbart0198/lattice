//! Scripting a running lattice: `lattice msg`, `lattice query` and `lattice subscribe`.
//!
//! Transport: the CLI makes a message-only window and sends its request as `WM_COPYDATA` to the
//! running lattice's hidden "lattice-control" window (`wParam` is the CLI's window). lattice
//! answers with a `WM_COPYDATA` back to that window, sent from a short-lived thread so a client
//! that stops pumping messages can never hold up the WM thread. Commands and queries go through
//! the WM thread's queue (`hooks.rs`), in order with keybinds, so `msg` then `query` sees the
//! command's result. Every payload is UTF-8 JSON.
//!
//! The control window doesn't lift UIPI's filter for `WM_COPYDATA`: a lower-integrity process
//! (a browser sandbox) must not be able to `exec` through lattice. An elevated lattice therefore
//! only takes requests from an elevated shell.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::io::Write;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::UI::WindowsAndMessaging::{
    ChangeWindowMessageFilterEx, CreateWindowExW, DefWindowProcW, DispatchMessageW, FindWindowW, HWND_MESSAGE, IsWindow, MSG, MSGFLT_ALLOW,
    MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, QS_ALLINPUT, RegisterClassW, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_COPYDATA, WNDCLASSW,
};
use windows::core::w;

use crate::config::{self, Command};
use crate::layout::Rect;
use crate::win::Hwnd;

/// `COPYDATASTRUCT::dwData` tags: client -> lattice request, lattice -> client reply and event.
pub const KIND_REQUEST: usize = 0x4C54_5251; // "LTRQ"
pub const KIND_REPLY: usize = 0x4C54_5250; // "LTRP"
pub const KIND_EVENT: usize = 0x4C54_4556; // "LTEV"
/// Bumped if the request format ever changes incompatibly.
const PROTOCOL: u32 = 1;
/// Requests are a command line or a query name; anything this big is not one.
const MAX_REQUEST: usize = 64 * 1024;

// ---------------------------------------------------------------------------------------------
// State, as `lattice query` prints it (and as `events.rs` diffs it)

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl From<Rect> for Area {
    fn from(r: Rect) -> Self {
        Area { x: r.x, y: r.y, width: r.w, height: r.h }
    }
}

/// Workspaces are 1-based (as `workspace N` takes them); monitors are 0-based, left to right.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct WorkspaceState {
    pub index: usize,
    pub active: bool,
    pub layout: &'static str,
    /// How many windows it holds, on all monitors.
    pub windows: usize,
    /// Monitors that hold at least one of its windows (a workspace spans every monitor).
    pub monitors: Vec<usize>,
    /// The window last focused on it.
    pub focused: Option<Hwnd>,
    pub fullscreen: Option<Hwnd>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct WindowState {
    pub hwnd: Hwnd,
    pub title: String,
    pub exe: String,
    pub class: String,
    pub workspace: usize,
    pub monitor: usize,
    pub floating: bool,
    pub minimized: bool,
    pub fullscreen: bool,
    pub focused: bool,
    /// Where it is (or is headed, mid-animation); skipped when the WM only diffs state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rect: Option<Area>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct MonitorState {
    pub index: usize,
    pub primary: bool,
    pub focused: bool,
    pub rect: Area,
    pub work_area: Area,
    pub scale: f32,
}

/// Everything a query can ask about, built by `Wm::state` on the WM thread.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub workspaces: Vec<WorkspaceState>,
    /// By workspace, then tiling order.
    pub windows: Vec<WindowState>,
    pub monitors: Vec<MonitorState>,
    /// 1-based.
    pub active_workspace: usize,
    pub focused_monitor: usize,
    pub focused_window: Option<Hwnd>,
}

impl State {
    pub fn window(&self, hwnd: Hwnd) -> Option<&WindowState> {
        self.windows.iter().find(|w| w.hwnd == hwnd)
    }

    fn focused_json(&self) -> Value {
        let layout = self.workspaces.iter().find(|w| w.index == self.active_workspace).map(|w| w.layout);
        json!({
            "workspace": self.active_workspace,
            "monitor": self.focused_monitor,
            "layout": layout,
            "window": self.focused_window.and_then(|h| self.window(h)),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Query {
    Workspaces,
    Windows,
    Monitors,
    Focused,
    /// All of the above in one object.
    State,
}

pub const QUERY_NAMES: &str = "workspaces, windows, monitors, focused, state";

impl Query {
    pub fn parse(s: &str) -> Option<Query> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "workspaces" => Query::Workspaces,
            "windows" => Query::Windows,
            "monitors" => Query::Monitors,
            "focused" => Query::Focused,
            "state" | "all" => Query::State,
            _ => return None,
        })
    }
}

pub fn query_json(s: &State, q: Query) -> Value {
    match q {
        Query::Workspaces => json!(s.workspaces),
        Query::Windows => json!(s.windows),
        Query::Monitors => json!(s.monitors),
        Query::Focused => s.focused_json(),
        Query::State => json!({
            "workspaces": s.workspaces,
            "windows": s.windows,
            "monitors": s.monitors,
            "focused": s.focused_json(),
        }),
    }
}

// ---------------------------------------------------------------------------------------------
// Wire format

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct WireRequest {
    v: u32,
    id: u32,
    op: String,
    #[serde(default)]
    arg: String,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct WireReply {
    id: u32,
    ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Msg(Command),
    Query(Query),
    Subscribe,
}

fn encode_request(id: u32, op: &str, arg: &str) -> Vec<u8> {
    serde_json::to_vec(&WireRequest { v: PROTOCOL, id, op: op.into(), arg: arg.into() }).expect("request serializes")
}

/// A request as lattice reads it: the id to answer, and what to do or why it can't. Commands
/// are parsed again here, so a CLI from another version can't slip in something this one
/// doesn't understand.
fn decode_request(bytes: &[u8]) -> (u32, Result<Op, String>) {
    let req = match serde_json::from_slice::<WireRequest>(bytes) {
        Ok(r) => r,
        Err(e) => return (0, Err(format!("bad request: {e}"))),
    };
    if req.v != PROTOCOL {
        return (req.id, Err(format!("protocol {} isn't supported (this lattice speaks {PROTOCOL})", req.v)));
    }
    let op = match req.op.as_str() {
        "msg" => match config::parse_command(&req.arg) {
            Ok(Some(c)) => Ok(Op::Msg(c)),
            Ok(None) => Err("no command given".into()),
            Err(e) => Err(e),
        },
        "query" => Query::parse(&req.arg).map(Op::Query).ok_or_else(|| format!("unknown query '{}' (try {QUERY_NAMES})", req.arg)),
        "subscribe" => Ok(Op::Subscribe),
        other => Err(format!("unknown request '{other}'")),
    };
    (req.id, op)
}

fn encode_reply(id: u32, result: Result<Value, String>) -> Vec<u8> {
    let r = match result {
        Ok(data) => WireReply { id, ok: true, data: Some(data), error: None },
        Err(e) => WireReply { id, ok: false, data: None, error: Some(e) },
    };
    serde_json::to_vec(&r).expect("reply serializes")
}

fn decode_reply(bytes: &[u8]) -> Option<(u32, Result<Value, String>)> {
    let r = serde_json::from_slice::<WireReply>(bytes).ok()?;
    Some((r.id, if r.ok { Ok(r.data.unwrap_or(Value::Null)) } else { Err(r.error.unwrap_or_else(|| "failed".into())) }))
}

// ---------------------------------------------------------------------------------------------
// lattice's side

/// Who to answer: the client's window and its request id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReplyTo {
    pub hwnd: Hwnd,
    pub id: u32,
}

impl ReplyTo {
    /// Answers from a thread of its own: a client that stopped pumping messages costs that
    /// thread a timeout, never the WM thread.
    pub fn send(self, result: Result<Value, String>) {
        let bytes = encode_reply(self.id, result);
        let spawned = std::thread::Builder::new().name("lattice-ipc-reply".into()).spawn(move || {
            send_copydata(self.hwnd, KIND_REPLY, &bytes, Duration::from_secs(2));
        });
        if let Err(e) = spawned {
            log::warn!("ipc reply: {e}");
        }
    }
}

/// A request for the WM thread's queue.
#[derive(Debug)]
pub struct Request {
    pub reply: ReplyTo,
    pub op: Op,
}

/// What the control window's `WM_COPYDATA` handler does with a message.
pub enum Received {
    /// Not ours: let `DefWindowProc` have it.
    NotOurs,
    /// Answered already (bad requests, subscriptions).
    Handled,
    /// Queue it for the WM thread.
    Queue(Request),
}

/// Reads a `WM_COPYDATA` sent to the control window. Copies the payload out: it's only valid
/// while the message is being handled.
pub fn receive(wp: WPARAM, lp: LPARAM) -> Received {
    if lp.0 == 0 {
        return Received::NotOurs;
    }
    let cds = unsafe { &*(lp.0 as *const COPYDATASTRUCT) };
    if cds.dwData != KIND_REQUEST {
        return Received::NotOurs;
    }
    let bytes = if cds.lpData.is_null() || cds.cbData as usize > MAX_REQUEST {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize) }.to_vec()
    };
    let (id, op) = decode_request(&bytes);
    let reply = ReplyTo { hwnd: wp.0 as Hwnd, id };
    match op {
        Ok(Op::Subscribe) => {
            crate::events::subscribe(reply.hwnd);
            reply.send(Ok(Value::Null));
            Received::Handled
        }
        Ok(op) => Received::Queue(Request { reply, op }),
        Err(e) => {
            reply.send(Err(e));
            Received::Handled
        }
    }
}

/// Sends `bytes` as `WM_COPYDATA` to `to`; false if it's gone, hung, or refused it.
pub fn send_copydata(to: Hwnd, kind: usize, bytes: &[u8], timeout: Duration) -> bool {
    let to = HWND(to as *mut c_void);
    let cds = COPYDATASTRUCT { dwData: kind, cbData: bytes.len() as u32, lpData: bytes.as_ptr() as *mut c_void };
    let mut result = 0usize;
    let ok = unsafe {
        SendMessageTimeoutW(to, WM_COPYDATA, WPARAM(0), LPARAM(&cds as *const _ as isize), SMTO_ABORTIFHUNG, timeout.as_millis() as u32, Some(&mut result))
    };
    ok.0 != 0 && result != 0
}

// ---------------------------------------------------------------------------------------------
// The CLI's side

thread_local! {
    /// `WM_COPYDATA` payloads that reached the client window: (kind, bytes).
    static INBOX: RefCell<VecDeque<(usize, Vec<u8>)>> = const { RefCell::new(VecDeque::new()) };
}

unsafe extern "system" fn client_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_COPYDATA && lp.0 != 0 {
        let cds = unsafe { &*(lp.0 as *const COPYDATASTRUCT) };
        if matches!(cds.dwData, KIND_REPLY | KIND_EVENT) {
            let bytes = if cds.lpData.is_null() {
                Vec::new()
            } else {
                unsafe { std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize) }.to_vec()
            };
            INBOX.with(|q| q.borrow_mut().push_back((cds.dwData, bytes)));
            return LRESULT(1);
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

struct Client {
    hwnd: HWND,
    control: HWND,
}

enum Waited<T> {
    Got(T),
    TimedOut,
    /// lattice's control window went away.
    Gone,
}

impl Client {
    fn connect() -> Result<Client, String> {
        let control = unsafe { FindWindowW(w!("lattice-display-listener"), w!("lattice-control")) }
            .ok()
            .filter(|h| !h.is_invalid())
            .ok_or("lattice isn't running")?;
        unsafe {
            let instance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).map_err(|e| e.to_string())?;
            let class = w!("lattice-ipc-client");
            RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(client_proc), hInstance: instance.into(), lpszClassName: class, ..Default::default() });
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class,
                None,
                WINDOW_STYLE::default(),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(instance.into()),
                None,
            )
            .map_err(|e| format!("couldn't create a window for replies: {e}"))?;
            // From an elevated shell to a lattice that isn't, Windows would drop the replies.
            let _ = ChangeWindowMessageFilterEx(hwnd, WM_COPYDATA, MSGFLT_ALLOW, None);
            Ok(Client { hwnd, control })
        }
    }

    /// Sends one request; Ok once lattice took it (the answer comes later, as a reply).
    fn send(&self, id: u32, op: &str, arg: &str) -> Result<(), String> {
        let bytes = encode_request(id, op, arg);
        let cds = COPYDATASTRUCT { dwData: KIND_REQUEST, cbData: bytes.len() as u32, lpData: bytes.as_ptr() as *mut c_void };
        let mut accepted = 0usize;
        let sent = unsafe {
            SendMessageTimeoutW(
                self.control,
                WM_COPYDATA,
                WPARAM(self.hwnd.0 as usize),
                LPARAM(&cds as *const _ as isize),
                SMTO_ABORTIFHUNG,
                5000,
                Some(&mut accepted),
            )
        };
        if sent.0 == 0 {
            return Err("lattice isn't responding (or runs elevated and this shell doesn't)".into());
        }
        if accepted == 0 {
            return Err("the running lattice doesn't take commands; it's older than this one (restart it)".into());
        }
        Ok(())
    }

    /// Pumps messages until `take` finds what it wants, the time is up, or lattice exits.
    fn wait<T>(&self, timeout: Option<Duration>, mut take: impl FnMut() -> Option<T>) -> Waited<T> {
        let start = Instant::now();
        loop {
            if let Some(v) = take() {
                return Waited::Got(v);
            }
            if !unsafe { IsWindow(Some(self.control)) }.as_bool() {
                return Waited::Gone;
            }
            let mut left = Duration::from_millis(500);
            if let Some(t) = timeout {
                let rest = t.saturating_sub(start.elapsed());
                if rest.is_zero() {
                    return Waited::TimedOut;
                }
                left = left.min(rest);
            }
            unsafe {
                MsgWaitForMultipleObjects(None, false, left.as_millis() as u32, QS_ALLINPUT);
                let mut msg = MSG::default();
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    DispatchMessageW(&msg);
                }
            }
        }
    }

    /// One request and its reply.
    fn call(&self, op: &str, arg: &str, timeout: Duration) -> Waited<Result<Value, String>> {
        const ID: u32 = 1;
        if let Err(e) = self.send(ID, op, arg) {
            return Waited::Got(Err(e));
        }
        self.wait(Some(timeout), || {
            INBOX.with(|q| {
                let mut q = q.borrow_mut();
                let at = q.iter().position(|(k, b)| *k == KIND_REPLY && decode_reply(b).is_some_and(|(id, _)| id == ID))?;
                q.remove(at).and_then(|(_, b)| decode_reply(&b)).map(|(_, r)| r)
            })
        })
    }
}

/// One request to the running lattice from another of our own programs (the settings window):
/// `op` is "msg" or "query". Waits up to 5 s for the answer; blocks, so not on a UI thread.
pub fn request(op: &str, arg: &str) -> Result<Value, String> {
    let client = Client::connect()?;
    match client.call(op, arg, Duration::from_secs(5)) {
        Waited::Got(r) => r,
        Waited::TimedOut => Err("lattice didn't answer in time".into()),
        Waited::Gone => Err("lattice exited before answering".into()),
    }
}

/// `lattice msg` arguments back into one command line: a single argument is used as given;
/// several are joined, quoting any that hold spaces (the shell took their quotes off).
pub fn join_args(args: &[String]) -> String {
    if let [one] = args {
        return one.clone();
    }
    args.iter().map(|a| if a.contains(char::is_whitespace) { format!("\"{a}\"") } else { a.clone() }).collect::<Vec<_>>().join(" ")
}

/// Takes `--wait` out of `lattice msg` arguments: as a word of its own anywhere, or right
/// after the command in a line given as one argument (`"restore-setup --wait Dev"`).
fn take_wait(args: &[String]) -> (bool, Vec<String>) {
    let rest: Vec<String> = args.iter().filter(|a| *a != "--wait").cloned().collect();
    if rest.len() < args.len() {
        return (true, rest);
    }
    if let [one] = args {
        let (verb, tail) = one.trim().split_once(char::is_whitespace).unwrap_or((one.trim(), ""));
        let tail = tail.trim_start();
        if let Some(after) = tail.strip_prefix("--wait").filter(|a| a.is_empty() || a.starts_with(char::is_whitespace)) {
            return (true, vec![format!("{verb} {}", after.trim_start()).trim_end().to_string()]);
        }
    }
    (false, rest)
}

/// How long `msg restore-setup --wait` waits for `setup-restored`: lattice gives up on apps
/// after 60 s, so this only runs out if something is badly wrong.
const RESTORE_TIMEOUT: Duration = Duration::from_secs(90);

/// `msg restore-setup --wait`, after lattice took the command: waits for the
/// `setup-restored` event of restore `id` and prints it. Exit code 0 if every app took its
/// spot, else 1.
fn wait_restored(client: &Client, id: u64, fail: impl Fn(i32, &str) -> i32) -> i32 {
    let found = client.wait(Some(RESTORE_TIMEOUT), || {
        INBOX.with(|q| {
            let mut q = q.borrow_mut();
            let events: Vec<_> = q.drain(..).filter(|(k, _)| *k == KIND_EVENT).collect();
            events
                .into_iter()
                .filter_map(|(_, b)| serde_json::from_slice::<Value>(&b).ok())
                .find(|v| v["event"] == "setup-restored" && v["restore"] == id)
        })
    });
    match found {
        Waited::Got(v) => {
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{v}");
            let _ = out.flush();
            let missing: Vec<&str> = v["missing"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
            if missing.is_empty() { 0 } else { fail(1, &format!("never appeared: {}", missing.join(", "))) }
        }
        Waited::Gone => fail(1, "lattice exited before the setup was restored"),
        Waited::TimedOut => fail(1, "the setup wasn't restored in time"),
    }
}

/// `lattice msg|query|subscribe ...`; returns the process exit code.
pub fn cli(args: &[String]) -> i32 {
    let (verb, rest) = match args.split_first() {
        Some((v, r)) => (v.as_str(), r),
        None => return 2,
    };
    let fail = |code: i32, e: &str| {
        eprintln!("lattice {verb}: {e}");
        code
    };
    match verb {
        "msg" => {
            let (wait, rest) = take_wait(rest);
            let line = join_args(&rest);
            let cmd = match config::parse_command(&line) {
                Ok(Some(c)) => c,
                Ok(None) => return fail(2, "give a command, as in a keybind (e.g. `lattice msg workspace 3`)"),
                Err(e) => return fail(2, &e),
            };
            if wait && !matches!(cmd, Command::RestoreSetup(_)) {
                return fail(2, "--wait only goes with restore-setup");
            }
            let client = match Client::connect() {
                Ok(c) => c,
                Err(e) => return fail(1, &e),
            };
            // Subscribed before the restore starts, so its `setup-restored` can't be missed
            // (a restore with nothing to start finishes in the same batch).
            if wait {
                match client.call("subscribe", "", Duration::from_secs(10)) {
                    Waited::Got(Ok(_)) => {}
                    Waited::Got(Err(e)) => return fail(1, &e),
                    Waited::Gone => return fail(1, "lattice exited"),
                    Waited::TimedOut => return fail(1, "lattice didn't answer in time"),
                }
            }
            match client.call("msg", &line, Duration::from_secs(10)) {
                Waited::Got(Ok(data)) if wait => match data["restore"].as_u64() {
                    Some(id) => wait_restored(&client, id, fail),
                    None => fail(1, "the running lattice can't wait for a restore; it's older than this one (restart it)"),
                },
                Waited::Got(Ok(_)) => 0,
                Waited::Got(Err(e)) => fail(1, &e),
                // `quit` takes lattice down before its answer can leave.
                Waited::Gone if cmd == Command::Quit => 0,
                Waited::Gone => fail(1, "lattice exited before answering"),
                Waited::TimedOut => fail(1, "lattice didn't answer in time"),
            }
        }
        "query" => {
            let what = rest.first().map(String::as_str).unwrap_or("");
            if Query::parse(what).is_none() {
                return fail(2, &format!("what to query? one of {QUERY_NAMES}"));
            }
            let client = match Client::connect() {
                Ok(c) => c,
                Err(e) => return fail(1, &e),
            };
            match client.call("query", what, Duration::from_secs(10)) {
                Waited::Got(Ok(v)) => {
                    let text = serde_json::to_string_pretty(&v).unwrap_or_default();
                    let mut out = std::io::stdout().lock();
                    let _ = writeln!(out, "{text}");
                    let _ = out.flush();
                    0
                }
                Waited::Got(Err(e)) => fail(1, &e),
                Waited::Gone => fail(1, "lattice exited before answering"),
                Waited::TimedOut => fail(1, "lattice didn't answer in time"),
            }
        }
        "subscribe" => {
            let mut only = Vec::new();
            for name in rest {
                match crate::events::Kind::parse(name) {
                    Some(k) => only.push(k.name()),
                    None => return fail(2, &format!("unknown event '{name}' (events: {})", crate::events::NAMES)),
                }
            }
            let client = match Client::connect() {
                Ok(c) => c,
                Err(e) => return fail(1, &e),
            };
            match client.call("subscribe", "", Duration::from_secs(10)) {
                Waited::Got(Ok(_)) => {}
                Waited::Got(Err(e)) => return fail(1, &e),
                Waited::Gone => return fail(1, "lattice exited"),
                Waited::TimedOut => return fail(1, "lattice didn't answer in time"),
            }
            // One JSON object per line until lattice exits or whoever reads us goes away.
            let mut out = std::io::stdout().lock();
            let printing = client.wait(None, || {
                let events: Vec<Vec<u8>> = INBOX.with(|q| {
                    let mut q = q.borrow_mut();
                    let all: Vec<_> = q.drain(..).collect();
                    all.into_iter().filter(|(k, _)| *k == KIND_EVENT).map(|(_, b)| b).collect()
                });
                for bytes in events {
                    let Ok(v) = serde_json::from_slice::<Value>(&bytes) else { continue };
                    if !only.is_empty() && !v["event"].as_str().is_some_and(|e| only.contains(&e)) {
                        continue;
                    }
                    if writeln!(out, "{v}").and_then(|_| out.flush()).is_err() {
                        return Some(());
                    }
                }
                None
            });
            match printing {
                Waited::Gone => fail(1, "lattice exited"),
                _ => 0,
            }
        }
        _ => fail(2, "unknown subcommand"),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::config::WsTarget;

    #[test]
    fn wait_flag() {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(take_wait(&v(&["restore-setup", "--wait", "Dev"])), (true, v(&["restore-setup", "Dev"])));
        assert_eq!(take_wait(&v(&["restore-setup", "Deep Work", "--wait"])), (true, v(&["restore-setup", "Deep Work"])));
        assert_eq!(take_wait(&v(&["restore-setup --wait Deep Work"])), (true, v(&["restore-setup Deep Work"])));
        assert_eq!(take_wait(&v(&["restore-setup --wait"])), (true, v(&["restore-setup"])));
        assert_eq!(take_wait(&v(&["restore-setup --waiting"])), (false, v(&["restore-setup --waiting"])));
        assert_eq!(take_wait(&v(&["workspace", "3"])), (false, v(&["workspace", "3"])));
    }

    #[test]
    fn requests_round_trip() {
        let (id, op) = decode_request(&encode_request(7, "msg", "workspace 3"));
        assert_eq!((id, op), (7, Ok(Op::Msg(Command::Workspace(WsTarget::Index(2))))));
        let (_, op) = decode_request(&encode_request(1, "query", "Windows"));
        assert_eq!(op, Ok(Op::Query(Query::Windows)));
        let (_, op) = decode_request(&encode_request(1, "subscribe", ""));
        assert_eq!(op, Ok(Op::Subscribe));
    }

    #[test]
    fn bad_requests_say_why() {
        assert!(decode_request(&encode_request(3, "msg", "workspace zero")).1.unwrap_err().contains("bad workspace"));
        assert_eq!(decode_request(&encode_request(3, "msg", "none")).0, 3);
        assert!(decode_request(&encode_request(3, "msg", "none")).1.is_err());
        assert!(decode_request(&encode_request(3, "query", "everything")).1.unwrap_err().contains("unknown query"));
        assert!(decode_request(&encode_request(3, "dance", "")).1.is_err());
        assert!(decode_request(b"not json").1.is_err());
        let future = br#"{"v":2,"id":9,"op":"msg","arg":"reload"}"#;
        assert_eq!(decode_request(future).0, 9);
        assert!(decode_request(future).1.unwrap_err().contains("protocol"));
    }

    #[test]
    fn replies_round_trip() {
        assert_eq!(decode_reply(&encode_reply(4, Ok(json!([1, 2])))), Some((4, Ok(json!([1, 2])))));
        assert_eq!(decode_reply(&encode_reply(5, Err("nope".into()))), Some((5, Err("nope".into()))));
        assert_eq!(decode_reply(b"{}"), None);
    }

    #[test]
    fn msg_args_join() {
        let s = |v: &[&str]| v.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        assert_eq!(join_args(&s(&["workspace", "3"])), "workspace 3");
        assert_eq!(join_args(&s(&["exec wt.exe -d C:\\"])), "exec wt.exe -d C:\\");
        assert_eq!(join_args(&s(&["exec", "notepad", "C:\\my file.txt"])), "exec notepad \"C:\\my file.txt\"");
    }

    /// Two workspaces on two monitors; `events.rs` diffs variations of it.
    pub(crate) fn sample() -> State {
        let win = |hwnd, ws, mon, focused| WindowState {
            hwnd,
            title: format!("window {hwnd}"),
            exe: "app.exe".into(),
            class: "AppClass".into(),
            workspace: ws,
            monitor: mon,
            floating: false,
            minimized: false,
            fullscreen: false,
            focused,
            rect: None,
        };
        let ws = |index, active, windows, monitors: Vec<usize>, focused| WorkspaceState {
            index,
            active,
            layout: "dwindle",
            windows,
            monitors,
            focused,
            fullscreen: None,
        };
        let r = Area { x: 0, y: 0, width: 1920, height: 1080 };
        State {
            workspaces: vec![ws(1, true, 2, vec![0], Some(100)), ws(2, false, 1, vec![1], Some(300))],
            windows: vec![win(100, 1, 0, true), win(200, 1, 0, false), win(300, 2, 1, false)],
            monitors: vec![
                MonitorState { index: 0, primary: true, focused: true, rect: r, work_area: r, scale: 1.0 },
                MonitorState { index: 1, primary: false, focused: false, rect: Area { x: 1920, ..r }, work_area: r, scale: 1.5 },
            ],
            active_workspace: 1,
            focused_monitor: 0,
            focused_window: Some(100),
        }
    }

    #[test]
    fn query_output() {
        let s = sample();
        let ws = query_json(&s, Query::Workspaces);
        assert_eq!(
            ws[0],
            json!({"index": 1, "active": true, "layout": "dwindle", "windows": 2, "monitors": [0], "focused": 100, "fullscreen": null})
        );
        let wins = query_json(&s, Query::Windows);
        assert_eq!(wins.as_array().unwrap().len(), 3);
        assert_eq!(wins[2]["workspace"], 2);
        assert_eq!(wins[2]["monitor"], 1);
        assert!(wins[0].get("rect").is_none(), "no rect unless asked for");
        let mons = query_json(&s, Query::Monitors);
        assert_eq!(mons[1]["rect"], json!({"x": 1920, "y": 0, "width": 1920, "height": 1080}));
        assert_eq!(mons[1]["scale"], 1.5);
        let f = query_json(&s, Query::Focused);
        assert_eq!(f["workspace"], 1);
        assert_eq!(f["layout"], "dwindle");
        assert_eq!(f["window"]["hwnd"], 100);
        assert_eq!(f["window"]["title"], "window 100");
        let all = query_json(&s, Query::State);
        for key in ["workspaces", "windows", "monitors", "focused"] {
            assert!(all.get(key).is_some(), "{key}");
        }
        let none = State { focused_window: None, ..s };
        assert_eq!(query_json(&none, Query::Focused)["window"], Value::Null);
    }

    #[test]
    fn query_names() {
        assert_eq!(Query::parse("all"), Some(Query::State));
        assert_eq!(Query::parse(" focused "), Some(Query::Focused));
        assert_eq!(Query::parse("nope"), None);
    }
}
