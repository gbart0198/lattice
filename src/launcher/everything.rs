//! A client for voidtools Everything's IPC (the protocol behind its SDK DLL, spoken directly so
//! there's no DLL to ship): a query goes to Everything's hidden window as `WM_COPYDATA`, and the
//! results come back the same way to a message-only window of ours.
//!
//! A [`Client`] belongs to the thread that made it: replies are window messages, pumped while
//! [`Client::query`] waits.

use std::cell::RefCell;
use std::ffi::c_void;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, FindWindowW, HWND_MESSAGE, MSG, MsgWaitForMultipleObjects,
    PM_REMOVE, PeekMessageW, QS_ALLINPUT, RegisterClassW, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_COPYDATA, WNDCLASSW,
};
use windows::core::{PCWSTR, w};

/// Everything 1.4's IPC window class, then 1.5 alpha's (it carries the instance name).
const WINDOW_CLASSES: [PCWSTR; 2] =
    [w!("EVERYTHING_TASKBAR_NOTIFICATION"), w!("EVERYTHING_TASKBAR_NOTIFICATION_(1.5a)")];
const COPYDATA_QUERY2W: usize = 18;
const REQUEST_FILE_NAME: u32 = 0x1;
const REQUEST_PATH: u32 = 0x2;
const ITEM_FOLDER: u32 = 0x1;
const ITEM_DRIVE: u32 = 0x2;
/// Everything's `EVERYTHING_SORT_DATE_MODIFIED_DESCENDING`.
pub const SORT_DATE_MODIFIED_DESC: u32 = 14;
/// How long a query may take before we give up on its reply.
const TIMEOUT: Duration = Duration::from_millis(1500);
/// Our replies' `dwData` is this plus a sequence number, so a late reply to an abandoned query
/// is told apart from the one we're waiting for.
const REPLY_BASE: u32 = 0x4C54_0000;

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
    /// It's running but didn't answer: still loading its database, hung, or elevated.
    NoReply,
}

thread_local! {
    /// The latest reply our window received: its id and bytes.
    static REPLY: RefCell<Option<(u32, Vec<u8>)>> = const { RefCell::new(None) };
}

pub struct Client {
    hwnd: HWND,
    seq: u32,
}

/// Whether Everything is running, without making a client.
pub fn running() -> bool {
    everything_window().is_some()
}

fn everything_window() -> Option<HWND> {
    WINDOW_CLASSES
        .iter()
        .find_map(|class| unsafe { FindWindowW(*class, PCWSTR::null()) }.ok().filter(|h| !h.is_invalid()))
}

impl Client {
    /// A client for this thread, or `None` if Windows won't make its reply window.
    pub fn new() -> Option<Client> {
        unsafe {
            let instance = GetModuleHandleW(None).ok()?;
            let class = w!("lattice_everything_reply");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                lpszClassName: class,
                ..Default::default()
            };
            // Fails harmlessly when another thread already registered it.
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class,
                PCWSTR::null(),
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
            .ok()?;
            Some(Client { hwnd, seq: 0 })
        }
    }

    /// Runs `search` (Everything's own syntax) and returns up to `max` results, sorted by `sort`.
    pub fn query(&mut self, search: &str, max: u32, sort: u32) -> Result<Vec<Item>, Error> {
        let everything = everything_window().ok_or(Error::NotRunning)?;
        self.seq = self.seq.wrapping_add(1) & 0xFFFF;
        let id = REPLY_BASE | self.seq;

        // EVERYTHING_IPC_QUERY2: seven DWORDs, then the search as nul-terminated UTF-16.
        let header = [
            self.hwnd.0 as usize as u32, // reply_hwnd: window handles fit in 32 bits
            id,                          // reply_copydata_message
            0,                           // search_flags: no match case / whole word / path / regex
            0,                           // offset
            max,                         // max_results
            REQUEST_FILE_NAME | REQUEST_PATH,
            sort,
        ];
        let mut buf: Vec<u8> = header.iter().flat_map(|v| v.to_le_bytes()).collect();
        buf.extend(search.encode_utf16().chain([0]).flat_map(u16::to_le_bytes));
        let cds = COPYDATASTRUCT {
            dwData: COPYDATA_QUERY2W,
            cbData: buf.len() as u32,
            lpData: buf.as_mut_ptr() as *mut c_void,
        };

        REPLY.with(|r| r.borrow_mut().take());
        let mut accepted = 0usize;
        let sent = unsafe {
            SendMessageTimeoutW(
                everything,
                WM_COPYDATA,
                WPARAM(self.hwnd.0 as usize),
                LPARAM(&cds as *const _ as isize),
                SMTO_ABORTIFHUNG,
                TIMEOUT.as_millis() as u32,
                Some(&mut accepted),
            )
        };
        if sent.0 == 0 || accepted == 0 {
            return Err(Error::NoReply);
        }

        // The reply is a sent message: pumping our queue lets Windows deliver it.
        let start = Instant::now();
        loop {
            if let Some(bytes) = REPLY.with(|r| {
                let mut r = r.borrow_mut();
                if r.as_ref().is_some_and(|(got, _)| *got == id) { r.take().map(|(_, b)| b) } else { None }
            }) {
                return parse_list(&bytes).ok_or(Error::NoReply);
            }
            let left = TIMEOUT.saturating_sub(start.elapsed());
            if left.is_zero() {
                return Err(Error::NoReply);
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
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_COPYDATA && lparam.0 != 0 {
        let cds = unsafe { &*(lparam.0 as *const COPYDATASTRUCT) };
        let id = cds.dwData as u32;
        if id & 0xFFFF_0000 == REPLY_BASE && !cds.lpData.is_null() {
            let bytes = unsafe { std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize) }.to_vec();
            REPLY.with(|r| *r.borrow_mut() = Some((id, bytes)));
            return LRESULT(1);
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Parses an `EVERYTHING_IPC_LIST2`: a header of five DWORDs (total, count, offset,
/// request_flags, sort), then `count` items of (flags, data_offset); each item's data holds the
/// requested fields in flag order, strings as a DWORD length then that many UTF-16 units and a nul.
fn parse_list(b: &[u8]) -> Option<Vec<Item>> {
    let dword = |at: usize| b.get(at..at + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]));
    let count = dword(4)? as usize;
    let flags = dword(12)?;
    if flags & (REQUEST_FILE_NAME | REQUEST_PATH) != REQUEST_FILE_NAME | REQUEST_PATH {
        return None;
    }
    let string = |at: usize| -> Option<(String, usize)> {
        let len = dword(at)? as usize;
        let units = b.get(at + 4..at + 4 + len * 2)?;
        let text: Vec<u16> = units.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        Some((String::from_utf16_lossy(&text), at + 4 + (len + 1) * 2))
    };
    let mut items = Vec::with_capacity(count);
    for i in 0..count {
        let at = 20 + i * 8;
        let (item_flags, data) = (dword(at)?, dword(at + 4)? as usize);
        let (name, next) = string(data)?;
        let (dir_path, _) = string(next)?;
        let (path, name_at) = if dir_path.is_empty() {
            (name, 0)
        } else {
            let sep = if dir_path.ends_with('\\') { "" } else { "\\" };
            let name_at = dir_path.len() + sep.len();
            (format!("{dir_path}{sep}{name}"), name_at)
        };
        items.push(Item { path, name_at, dir: item_flags & (ITEM_FOLDER | ITEM_DRIVE) != 0 });
    }
    Some(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push_str(buf: &mut Vec<u8>, s: &str) {
        let units: Vec<u16> = s.encode_utf16().collect();
        buf.extend((units.len() as u32).to_le_bytes());
        buf.extend(units.iter().chain([&0]).flat_map(|u| u.to_le_bytes()));
    }

    #[test]
    fn parses_a_reply() {
        let entries = [("report.pdf", "C:\\Users\\me\\Documents", 0u32), ("Projects", "D:\\", ITEM_FOLDER)];
        let mut buf = Vec::new();
        for v in [2u32, 2, 0, REQUEST_FILE_NAME | REQUEST_PATH, SORT_DATE_MODIFIED_DESC] {
            buf.extend(v.to_le_bytes());
        }
        let mut data = Vec::new();
        let data_start = 20 + entries.len() * 8;
        for (name, path, flags) in entries {
            buf.extend(flags.to_le_bytes());
            buf.extend(((data_start + data.len()) as u32).to_le_bytes());
            push_str(&mut data, name);
            push_str(&mut data, path);
        }
        buf.extend(data);
        let items = parse_list(&buf).unwrap();
        assert_eq!(items[0].path, "C:\\Users\\me\\Documents\\report.pdf");
        assert_eq!(&items[0].path[items[0].name_at..], "report.pdf");
        assert!(!items[0].dir);
        assert_eq!(items[1].path, "D:\\Projects");
        assert_eq!(&items[1].path[items[1].name_at..], "Projects");
        assert!(items[1].dir);
    }

    #[test]
    fn truncated_replies_are_rejected() {
        let mut buf = Vec::new();
        for v in [5u32, 5, 0, REQUEST_FILE_NAME | REQUEST_PATH, 0] {
            buf.extend(v.to_le_bytes());
        }
        assert!(parse_list(&buf).is_none());
    }
}
