//! voidtools Everything is a Windows program: on macOS it's never running.

use crate::launcher::everything::{Error, Item};

pub struct Client;

pub fn running() -> bool {
    false
}

impl Client {
    pub fn new() -> Option<Client> {
        None
    }

    pub fn query(&mut self, _search: &str, _max: u32, _sort: u32) -> Result<Vec<Item>, Error> {
        Err(Error::NotRunning)
    }
}
