//! File search through voidtools Everything, a Windows file indexer, when it's running. The
//! client is the OS back end's (`platform::everything`; it only exists on Windows, elsewhere
//! Everything is never running); this is what it returns.

pub use super::platform::everything::{Client, running};

/// Everything's `EVERYTHING_SORT_DATE_MODIFIED_DESCENDING`.
pub const SORT_DATE_MODIFIED_DESC: u32 = 14;

/// One result: a file, folder or drive.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub path: String,
    /// Byte offset of the file name in `path`.
    pub name_at: usize,
    pub dir: bool,
}

#[derive(Debug, PartialEq)]
pub enum Error {
    /// Everything isn't running (or its IPC window isn't up yet).
    NotRunning,
    /// It turned the query down. Everything Lite does this to every query (it has no IPC), and
    /// the full version does while its database is loading.
    Refused,
    /// It took the query but no reply came: hung, or the reply was blocked.
    NoReply,
}
