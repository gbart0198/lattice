//! Two threads:
//! - the keyboard thread owns the low-level keyboard hook and does nothing else, so the hook
//!   always answers within Windows' timeout (a slow hook gets silently removed);
//! - the WM thread runs a message loop with WinEvent hooks (window lifecycle). Hook callbacks
//!   only enqueue work; the loop hands it to `Wm` one item at a time, so callbacks that re-enter
//!   during a Win32 call can't corrupt state.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::RwLock;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config::{self, Command, Config, MOD_ALT, MOD_CTRL, MOD_SHIFT, MOD_SUPER};
use crate::shared::{self, WM_LATTICE_WAKE};
use crate::wm::{Event, Wm};

enum Item {
    Event(Event),
    Command(Command),
    /// From `lattice msg` / `lattice query` (`ipc.rs`): answered once handled.
    Ipc(crate::ipc::Request),
}

thread_local! {
    static QUEUE: RefCell<VecDeque<Item>> = const { RefCell::new(VecDeque::new()) };
    static WAKE_POSTED: Cell<bool> = const { Cell::new(false) };
    static SWALLOWED: RefCell<HashSet<u32>> = RefCell::new(HashSet::new());
}

static KEYMAP: RwLock<Option<HashMap<(u8, u32), Command>>> = RwLock::new(None);

fn push(item: Item) {
    QUEUE.with(|q| q.borrow_mut().push_back(item));
    if !WAKE_POSTED.with(|w| w.replace(true)) {
        shared::wake_wm();
    }
}

fn pop() -> Option<Item> {
    QUEUE.with(|q| q.borrow_mut().pop_front())
}

// ---------------------------------------------------------------------------------------------
// Keyboard

const VK_LWIN: i32 = 0x5B;
const VK_RWIN: i32 = 0x5C;
const VK_SHIFT: i32 = 0x10;
const VK_CONTROL: i32 = 0x11;
const VK_MENU: i32 = 0x12;
const VK_ESCAPE: u32 = 0x1B;
/// Unassigned virtual key, sent so that releasing Win after a swallowed Win+key combo
/// doesn't open the Start menu.
const VK_MASK: u16 = 0xE8;

fn down(vk: i32) -> bool {
    unsafe { (GetAsyncKeyState(vk) as u16 & 0x8000) != 0 }
}

fn current_mods() -> u8 {
    let mut m = 0;
    if down(VK_LWIN) || down(VK_RWIN) {
        m |= MOD_SUPER;
    }
    if down(VK_SHIFT) {
        m |= MOD_SHIFT;
    }
    if down(VK_CONTROL) {
        m |= MOD_CTRL;
    }
    if down(VK_MENU) {
        m |= MOD_ALT;
    }
    m
}

fn is_modifier(vk: u32) -> bool {
    matches!(vk, 0x10 | 0x11 | 0x12 | 0x5B | 0x5C | 0xA0..=0xA5)
}

fn send_mask_key() {
    let key = |flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(VK_MASK), dwFlags: flags, ..Default::default() } },
    };
    unsafe {
        SendInput(&[key(Default::default()), key(KEYEVENTF_KEYUP)], std::mem::size_of::<INPUT>() as i32);
    }
}

unsafe extern "system" fn keyboard_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let kb = unsafe { &*(lp.0 as *const KBDLLHOOKSTRUCT) };
        let msg = wp.0 as u32;
        let injected = (kb.flags.0 & LLKHF_INJECTED.0) != 0;
        let vk = kb.vkCode;
        if !injected && !is_modifier(vk) {
            if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
                let mods = current_mods();
                // Escape ends the tour wherever the focus is, unless the launcher is up (Escape
                // is how you close it). Only while a live tour is checking in.
                if vk == VK_ESCAPE && mods == 0 && shared::tour_active() && !crate::launcher::is_visible() {
                    shared::tour_escape();
                    SWALLOWED.with(|s| s.borrow_mut().insert(vk));
                    return LRESULT(1);
                }
                let cmd = KEYMAP.read().ok().and_then(|k| k.as_ref()?.get(&(mods, vk)).cloned());
                if let Some(cmd) = cmd {
                    SWALLOWED.with(|s| s.borrow_mut().insert(vk));
                    if mods & MOD_SUPER != 0 {
                        send_mask_key();
                    }
                    shared::send_command(cmd);
                    return LRESULT(1);
                }
            } else if (msg == WM_KEYUP || msg == WM_SYSKEYUP) && SWALLOWED.with(|s| s.borrow_mut().remove(&vk)) {
                return LRESULT(1);
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wp, lp) }
}

// ---------------------------------------------------------------------------------------------
// WinEvents

const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
const EVENT_SYSTEM_MOVESIZEEND: u32 = 0x000B;
const EVENT_SYSTEM_MINIMIZESTART: u32 = 0x0016;
const EVENT_SYSTEM_MINIMIZEEND: u32 = 0x0017;
const EVENT_OBJECT_DESTROY: u32 = 0x8001;
const EVENT_OBJECT_SHOW: u32 = 0x8002;
const EVENT_OBJECT_HIDE: u32 = 0x8003;
const EVENT_OBJECT_LOCATIONCHANGE: u32 = 0x800B;
const EVENT_OBJECT_NAMECHANGE: u32 = 0x800C;
const EVENT_OBJECT_CLOAKED: u32 = 0x8017;
const EVENT_OBJECT_UNCLOAKED: u32 = 0x8018;
const WINEVENT_OUTOFCONTEXT: u32 = 0x0000;
const WINEVENT_SKIPOWNPROCESS: u32 = 0x0002;

unsafe extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _thread: u32,
    _time: u32,
) {
    // Only events about whole windows (OBJID_WINDOW, CHILDID_SELF).
    if hwnd.is_invalid() || id_object != 0 || id_child != 0 {
        return;
    }
    let h = hwnd.0 as isize;
    let ev = match event {
        EVENT_OBJECT_SHOW => Event::Show(h),
        EVENT_OBJECT_HIDE => Event::Hide(h),
        EVENT_OBJECT_DESTROY => Event::Destroy(h),
        EVENT_OBJECT_CLOAKED => Event::Cloaked(h),
        EVENT_OBJECT_UNCLOAKED => Event::Uncloaked(h),
        EVENT_OBJECT_NAMECHANGE => Event::NameChange(h),
        EVENT_OBJECT_LOCATIONCHANGE => Event::Location(h),
        EVENT_SYSTEM_FOREGROUND => Event::Foreground(h),
        EVENT_SYSTEM_MINIMIZESTART => Event::MinimizeStart(h),
        EVENT_SYSTEM_MINIMIZEEND => Event::MinimizeEnd(h),
        EVENT_SYSTEM_MOVESIZEEND => Event::MoveSizeEnd(h),
        _ => return,
    };
    push(Item::Event(ev));
}

// ---------------------------------------------------------------------------------------------
// Loop

fn install_keymap(cfg: &Config) {
    let map = config::build_keymap(cfg);
    log::info!("{} keybinds active", map.len());
    if let Ok(mut k) = KEYMAP.write() {
        *k = Some(map);
    }
}

/// Runs the keyboard hook on its own thread for the life of the process.
fn spawn_keyboard_thread() {
    std::thread::Builder::new()
        .name("lattice-keyboard".into())
        .spawn(|| unsafe {
            if let Err(e) = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), None, 0) {
                log::error!("keyboard hook failed: {e}; keybinds won't work");
                return;
            }
            // Low-level hooks are called through this thread's message loop.
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        })
        .expect("spawn keyboard thread");
}

/// Hidden window that hears display broadcasts (message-only windows don't get them):
/// resolution/rotation changes, monitors added or removed, taskbar/work-area changes.
unsafe extern "system" fn display_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    const SPI_SETWORKAREA: usize = 0x002F;
    // Scripts (install/uninstall) ask lattice to quit cleanly by closing this window: same
    // path as Win+Shift+E, so every hidden window is shown again before exit.
    if msg == shared::WM_TOUR_STATUS {
        return LRESULT(shared::tour_status(wp.0 != 0) as isize);
    }
    if msg == shared::WM_TOUR_RECT {
        return LRESULT(shared::tour_rect(wp.0) as isize);
    }
    if msg == WM_APP + 2 {
        // The settings window saved config.toml.
        push(Item::Command(Command::Reload));
        return LRESULT(0);
    }
    if msg == WM_COPYDATA {
        match crate::ipc::receive(wp, lp) {
            crate::ipc::Received::Queue(req) => push(Item::Ipc(req)),
            crate::ipc::Received::Handled => {}
            crate::ipc::Received::NotOurs => return unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
        }
        return LRESULT(1);
    }
    if msg == WM_CLOSE {
        push(Item::Command(Command::Quit));
        return LRESULT(0);
    }
    if msg == WM_DISPLAYCHANGE || (msg == WM_SETTINGCHANGE && wp.0 == SPI_SETWORKAREA) {
        log::info!("display configuration changed; re-reading monitors");
        push(Item::Command(Command::Retile));
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

fn create_display_listener() {
    use windows::core::w;
    unsafe {
        let instance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap_or_default();
        let class = WNDCLASSW {
            lpfnWndProc: Some(display_proc),
            hInstance: instance.into(),
            lpszClassName: w!("lattice-display-listener"),
            ..Default::default()
        };
        RegisterClassW(&class);
        let created = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            w!("lattice-display-listener"),
            w!("lattice-control"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        );
        if let Err(e) = created {
            log::warn!("display listener: {e}; monitor changes need `retile`");
        }
    }
}

pub fn run(cfg: Config) {
    unsafe {
        // Make sure this thread has a message queue before anyone posts to it.
        let mut msg = MSG::default();
        let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
        shared::set_wm_thread(GetCurrentThreadId());
    }
    install_keymap(&cfg);
    crate::events::set_hooks(&cfg.hooks);
    spawn_keyboard_thread();
    create_display_listener();

    let ranges = [
        (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
        (EVENT_SYSTEM_MOVESIZEEND, EVENT_SYSTEM_MOVESIZEEND),
        (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND),
        (EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE),
        (EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_NAMECHANGE),
        (EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED),
    ];
    let hooks: Vec<HWINEVENTHOOK> = ranges
        .iter()
        .map(|&(a, b)| unsafe {
            SetWinEventHook(a, b, None, Some(win_event_proc), 0, 0, WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS)
        })
        .collect();

    let mut wm = Wm::new(cfg);
    wm.start_events();
    log::info!("lattice running");

    let mut msg = MSG::default();
    loop {
        let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if r.0 <= 0 {
            break;
        }
        if msg.message != WM_LATTICE_WAKE {
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        WAKE_POSTED.with(|w| w.set(false));
        for cmd in shared::take_commands() {
            push(Item::Command(cmd));
        }
        while let Some(item) = pop() {
            match item {
                Item::Event(e) => wm.event(e),
                Item::Command(c) => wm.command(c),
                Item::Ipc(req) => {
                    use crate::ipc::{Op, query_json};
                    match req.op {
                        // A setup that doesn't exist is the one command a script can
                        // name wrong that parses fine, so it's refused here (exit 1).
                        Op::Msg(Command::RestoreSetup(name)) if config::find_setup(&wm.config().setups, &name).is_none() => {
                            req.reply.send(Err(format!("no setup called '{name}'")));
                        }
                        // So is a window lattice doesn't manage (`close 1234`).
                        Op::Msg(c) => match wm.check_command(&c) {
                            Ok(()) => {
                                let restore = matches!(c, Command::RestoreSetup(_));
                                wm.command(c);
                                // A restore's id, for `--wait` to know its `setup-restored`.
                                let data = if restore { serde_json::json!({ "restore": wm.last_restore() }) } else { serde_json::Value::Null };
                                req.reply.send(Ok(data));
                            }
                            Err(e) => req.reply.send(Err(e)),
                        },
                        Op::Query(q) => req.reply.send(Ok(query_json(&wm.state(true), q))),
                        // Answered by `ipc::receive`; never queued.
                        Op::Subscribe => {}
                    }
                }
            }
            if wm.reload_keymap {
                wm.reload_keymap = false;
                install_keymap(wm.config());
            }
            if wm.quit {
                break;
            }
        }
        if wm.quit {
            break;
        }
        // A frame of any running animations (the frame clock's wakeups land here too).
        wm.animate();
        // Once per batch, not per item: a drag queues dozens of location changes, and a
        // workspace switch hides and shows every window on two workspaces.
        wm.update_border();
        wm.update_bar();
        wm.emit_events();
        wm.save_state();
        shared::flush_hidden();
    }

    log::info!("shutting down");
    unsafe {
        for h in hooks {
            let _ = UnhookWinEvent(h);
        }
    }
    wm.shutdown();
}
