//! The WM thread's loop. The OS back end (`platform::hooks`) supplies what happens: window
//! events, keys and requests arrive through its callbacks, which only queue work here
//! (`push_*`); the loop hands it to `Wm` one item at a time, so callbacks that re-enter during
//! an OS call can't corrupt state. Keybinds are looked up here too (`key_down`), for the back
//! end's keyboard hook.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::sync::RwLock;

use crate::config::{self, Command, Config};
use crate::platform::hooks as os;
use crate::shared;
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
}

static KEYMAP: RwLock<Option<HashMap<(u8, u32), Command>>> = RwLock::new(None);

fn push(item: Item) {
    QUEUE.with(|q| q.borrow_mut().push_back(item));
    if !WAKE_POSTED.with(|w| w.replace(true)) {
        shared::wake_wm();
    }
}

/// Queues a window event. On the WM thread (the back end's callbacks run there).
pub fn push_event(e: Event) {
    push(Item::Event(e));
}

/// Queues a command. On the WM thread; other threads use `shared::send_command`.
pub fn push_command(c: Command) {
    push(Item::Command(c));
}

/// Queues a `lattice msg` / `query` request. On the WM thread.
pub fn push_ipc(r: crate::ipc::Request) {
    push(Item::Ipc(r));
}

fn pop() -> Option<Item> {
    QUEUE.with(|q| q.borrow_mut().pop_front())
}

/// What the keyboard hook does with a key.
pub enum Key {
    /// Not lattice's: the app gets it.
    Pass,
    /// Taken (Escape for the tour), nothing to run.
    Swallow,
    /// A keybind: taken, and the hook sends the command once it has done its OS-side work.
    Run(Command),
}

/// A key (not a modifier, not one lattice injected) went down with `mods` held: `key` as the
/// keymap names it, `escape` when it's Escape. Called on the keyboard thread.
pub fn key_down(mods: u8, key: u32, escape: bool) -> Key {
    // Escape ends the tour wherever the focus is, unless the launcher is up (Escape is how you
    // close it). Only while a live tour is checking in.
    if escape && mods == 0 && shared::tour_active() && !crate::launcher::is_visible() {
        shared::tour_escape();
        return Key::Swallow;
    }
    match KEYMAP.read().ok().and_then(|k| k.as_ref()?.get(&(mods, key)).cloned()) {
        Some(cmd) => Key::Run(cmd),
        None => Key::Pass,
    }
}

fn install_keymap(cfg: &Config) {
    let map = config::build_keymap(cfg);
    log::info!("{} keybinds active", map.len());
    if let Ok(mut k) = KEYMAP.write() {
        *k = Some(map);
    }
}

pub fn run(cfg: Config) {
    os::init();
    install_keymap(&cfg);
    crate::events::set_hooks(&cfg.hooks);
    let sources = os::install();

    let mut wm = Wm::new(cfg);
    wm.start_events();
    log::info!("lattice running");

    while os::wait() {
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
    os::uninstall(sources);
    wm.shutdown();
}
