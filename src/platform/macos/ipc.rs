//! The transport under `lattice msg/query/subscribe` on macOS: not written yet (a Unix socket).
//! The CLI says so and exits 1; lattice takes no requests.

use std::time::Duration;

use crate::ipc::{Inbox, Waited};

/// Who a reply or event goes to.
pub type Peer = isize;

pub fn send(_to: Peer, _kind: usize, _bytes: &[u8], _timeout: Duration) -> bool {
    false
}

pub struct Client;

impl Client {
    pub fn connect() -> Result<Client, String> {
        Err("lattice msg, query and subscribe aren't available on macOS yet".into())
    }

    pub fn send(&self, _bytes: &[u8]) -> Result<(), String> {
        Err("not connected".into())
    }

    pub fn wait<T>(&self, _timeout: Option<Duration>, _take: impl FnMut(&mut Inbox) -> Option<T>) -> Waited<T> {
        Waited::Gone
    }
}
