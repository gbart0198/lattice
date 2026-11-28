//! The transport under `lattice msg/query/subscribe` on Windows. The CLI makes a message-only
//! window and sends its request as `WM_COPYDATA` to the running lattice's hidden
//! "lattice-control" window (`wParam` is the CLI's window). lattice answers with a
//! `WM_COPYDATA` back to that window. What's in the messages is `crate::ipc`'s.
//!
//! The control window doesn't lift UIPI's filter for `WM_COPYDATA`: a lower-integrity process
//! (a browser sandbox) must not be able to `exec` through lattice. An elevated lattice therefore
//! only takes requests from an elevated shell.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::UI::WindowsAndMessaging::{
    ChangeWindowMessageFilterEx, CreateWindowExW, DefWindowProcW, DispatchMessageW, FindWindowW, HWND_MESSAGE, IsWindow, MSG, MSGFLT_ALLOW,
    MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, QS_ALLINPUT, RegisterClassW, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_COPYDATA, WNDCLASSW,
};
use windows::core::w;

use crate::ipc::{Inbox, KIND_EVENT, KIND_REPLY, KIND_REQUEST, MAX_REQUEST, Received, Waited};

/// Who a reply or event goes to: the client's window.
pub type Peer = isize;

// ---------------------------------------------------------------------------------------------
// lattice's side

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
    // `wParam` is the client's window.
    crate::ipc::received(wp.0 as Peer, &bytes)
}

/// Sends `bytes` as `WM_COPYDATA` to `to`; false if it's gone, hung, or refused it.
pub fn send(to: Peer, kind: usize, bytes: &[u8], timeout: Duration) -> bool {
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
    static INBOX: RefCell<Inbox> = const { RefCell::new(VecDeque::new()) };
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

/// The CLI's window for replies, and lattice's control window.
pub struct Client {
    hwnd: HWND,
    control: HWND,
}

impl Client {
    pub fn connect() -> Result<Client, String> {
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
    pub fn send(&self, bytes: &[u8]) -> Result<(), String> {
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
    pub fn wait<T>(&self, timeout: Option<Duration>, mut take: impl FnMut(&mut Inbox) -> Option<T>) -> Waited<T> {
        let start = Instant::now();
        loop {
            if let Some(v) = INBOX.with(|q| take(&mut q.borrow_mut())) {
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
}
