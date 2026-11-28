//! The launcher on Windows: the app catalog (`shell:AppsFolder`, the Start menu's own
//! "All apps" list: desktop and Store apps alike), app icons, launching, and bringing the
//! launcher window to the front.
//!
//! `scan` and `icon` use COM: call them on a thread that ran `com_init`.

use std::ffi::c_void;

use windows::Win32::Foundation::{HWND, SIZE};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDIBits,
    GetObjectW, HBITMAP, HGDIOBJ,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, IBindCtx};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP, MOUSEEVENTF_MOVE, MOUSEINPUT, SendInput,
    VK_MENU,
};
use windows::Win32::UI::Shell::{
    BHID_EnumItems, FOLDERID_AppsFolder, IEnumShellItems, IShellItem, IShellItemImageFactory, KF_FLAG_DEFAULT, SIGDN,
    SIGDN_NORMALDISPLAY, SIGDN_PARENTRELATIVEPARSING, SIIGBF_ICONONLY, SHCreateItemFromParsingName, SHGetKnownFolderItem,
};
use windows::Win32::UI::WindowsAndMessaging::{
    ASFW_ANY, AllowSetForegroundWindow, GetForegroundWindow, IsWindowVisible, SetForegroundWindow,
};
use windows::core::{HSTRING, Interface};

use crate::launcher::platform::{App, Icon};

/// Prepares the calling thread for the shell's COM objects.
pub fn com_init() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
}

/// Every app in the Start menu's "All apps", sorted by name. Skips uninstallers and the readme
/// and help documents some installers add.
pub fn scan() -> Vec<App> {
    let mut apps = Vec::new();
    unsafe {
        let folder: IShellItem = match SHGetKnownFolderItem(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None) {
            Ok(f) => f,
            Err(e) => {
                log::error!("launcher: can't open the apps folder: {e}");
                return apps;
            }
        };
        let items: IEnumShellItems = match folder.BindToHandler(None::<&IBindCtx>, &BHID_EnumItems) {
            Ok(i) => i,
            Err(e) => {
                log::error!("launcher: can't list the apps folder: {e}");
                return apps;
            }
        };
        loop {
            let mut slot = [None];
            let mut fetched = 0u32;
            // S_FALSE (also Ok) with nothing fetched is the end of the list.
            if items.Next(&mut slot, Some(&mut fetched)).is_err() || fetched == 0 {
                break;
            }
            let Some(item) = slot[0].take() else { break };
            let (Some(name), Some(id)) =
                (display_name(&item, SIGDN_NORMALDISPLAY), display_name(&item, SIGDN_PARENTRELATIVEPARSING))
            else {
                continue;
            };
            if wanted(&name, &id) {
                apps.push(App { name: name.into(), id: id.into() });
            }
        }
    }
    apps.sort_by_cached_key(|a| a.name.to_lowercase());
    apps.dedup_by(|a, b| a.id == b.id);
    apps
}

unsafe fn display_name(item: &IShellItem, kind: SIGDN) -> Option<String> {
    unsafe {
        let p = item.GetDisplayName(kind).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const c_void));
        s.filter(|s| !s.trim().is_empty())
    }
}

fn wanted(name: &str, id: &str) -> bool {
    const DOCUMENTS: [&str; 9] = [".txt", ".chm", ".pdf", ".htm", ".html", ".rtf", ".md", ".log", ".ini"];
    let id = id.to_ascii_lowercase();
    !name.to_lowercase().contains("uninstall") && !DOCUMENTS.iter().any(|ext| id.ends_with(ext))
}

/// The program an app in the catalog runs, as a full path: a shortcut's target, or a packaged
/// app's executable from its manifest. None for the few that have neither (web apps, say).
pub fn target_exe(id: &str) -> Option<String> {
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::UI::Shell::IShellItem2;
    use windows::Win32::UI::Shell::PropertiesSystem::PSGetPropertyKeyFromName;
    // A path already (possibly under a known-folder GUID, which the shell resolves below).
    if !id.contains('!') && id.to_ascii_lowercase().ends_with(".exe") && !id.starts_with('{') {
        return Some(id.to_string());
    }
    unsafe {
        let item: IShellItem2 = SHCreateItemFromParsingName(&HSTRING::from(apps_folder_path(id)), None::<&IBindCtx>).ok()?;
        let prop = |name: &str| -> Option<String> {
            let mut key = PROPERTYKEY::default();
            PSGetPropertyKeyFromName(&HSTRING::from(name), &mut key).ok()?;
            let p = item.GetString(&key).ok()?;
            let s = p.to_string().ok();
            CoTaskMemFree(Some(p.0 as *const c_void));
            s.filter(|s| !s.trim().is_empty())
        };
        if let Some(target) = prop("System.Link.TargetParsingPath").filter(|t| t.to_ascii_lowercase().ends_with(".exe")) {
            return Some(target);
        }
        let dir = prop("System.AppUserModel.PackageInstallPath")?;
        let manifest = std::fs::read_to_string(std::path::Path::new(&dir).join("AppxManifest.xml")).ok()?;
        let exe = manifest_executable(&manifest, id.split_once('!')?.1)?;
        Some(format!("{}\\{}", dir.trim_end_matches('\\'), exe.replace('/', "\\")))
    }
}

/// The `Executable` of the `<Application Id="app_id">` in a package's AppxManifest.xml.
fn manifest_executable(xml: &str, app_id: &str) -> Option<String> {
    let attr = |tag: &str, name: &str| -> Option<String> {
        let at = tag.find(&format!(" {name}=\"")).or_else(|| tag.find(&format!("\n{name}=\""))).or_else(|| tag.find(&format!("\t{name}=\"")))?;
        let rest = &tag[at + name.len() + 3..];
        Some(rest[..rest.find('"')?].to_string())
    };
    let mut rest = xml;
    while let Some(i) = rest.find("<Application") {
        rest = &rest[i + "<Application".len()..];
        // `<Applications>` is the list, not one.
        if !rest.starts_with([' ', '\n', '\r', '\t']) {
            continue;
        }
        let tag = &rest[..rest.find('>').unwrap_or(rest.len())];
        if attr(tag, "Id").as_deref() == Some(app_id) {
            return attr(tag, "Executable").filter(|e| !e.is_empty());
        }
    }
    None
}

/// The app's icon at `size` pixels, as the Start menu draws it.
pub fn icon(id: &str, size: u32) -> Option<Icon> {
    shell_icon(&apps_folder_path(id), size)
}

/// A file's or folder's icon at `size` pixels, as Explorer draws it (never a thumbnail).
pub fn file_icon(path: &str, size: u32) -> Option<Icon> {
    shell_icon(path, size)
}

fn shell_icon(parsing_name: &str, size: u32) -> Option<Icon> {
    unsafe {
        let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(parsing_name), None::<&IBindCtx>).ok()?;
        let factory: IShellItemImageFactory = item.cast().ok()?;
        let side = size as i32;
        let bitmap = factory.GetImage(SIZE { cx: side, cy: side }, SIIGBF_ICONONLY).ok()?;
        let icon = bitmap_rgba(bitmap);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        icon
    }
}

/// A 32-bit bitmap's pixels as straight-alpha RGBA (it must be square).
unsafe fn bitmap_rgba(bitmap: HBITMAP) -> Option<Icon> {
    unsafe {
        let mut bm = BITMAP::default();
        let got = GetObjectW(HGDIOBJ(bitmap.0), std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut c_void));
        if got == 0 || bm.bmWidth <= 0 || bm.bmWidth != bm.bmHeight.abs() {
            return None;
        }
        let side = bm.bmWidth;
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: side,
                biHeight: -side, // top-down rows
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut px = vec![0u8; (side * side * 4) as usize];
        let dc = CreateCompatibleDC(None);
        let rows = GetDIBits(dc, bitmap, 0, side as u32, Some(px.as_mut_ptr() as *mut c_void), &mut info, DIB_RGB_COLORS);
        let _ = DeleteDC(dc);
        if rows != side {
            return None;
        }
        Some(Icon { size: side as u32, rgba: bgra_to_rgba(px).into() })
    }
}

/// The shell hands icons over as BGRA, premultiplied, and older icons without any alpha at all.
fn bgra_to_rgba(mut px: Vec<u8>) -> Vec<u8> {
    let has_alpha = px.as_chunks::<4>().0.iter().any(|p| p[3] != 0);
    // Straight alpha has colour brighter than its alpha somewhere; premultiplied never does.
    let premultiplied = has_alpha && px.as_chunks::<4>().0.iter().all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]);
    for p in px.as_chunks_mut::<4>().0 {
        p.swap(0, 2);
        if !has_alpha {
            p[3] = 255;
        } else if premultiplied && p[3] != 0 && p[3] != 255 {
            let a = p[3] as u32;
            for c in &mut p[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    px
}

fn apps_folder_path(id: &str) -> String {
    format!("shell:AppsFolder\\{id}")
}

/// Starts the app the way the Start menu does: the running shell launches it, so the app gets
/// the shell's environment, not ours. `clean_env` is kept from the short-lived explorer.exe
/// that hands the request over, in case it ends up launching the app itself.
pub fn launch(id: &str, clean_env: &[String]) {
    unsafe {
        // We hold the foreground (the launcher had it): let the new app take it.
        let _ = AllowSetForegroundWindow(ASFW_ANY);
    }
    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new("explorer.exe");
    // Quoted by hand: explorer parses its own command line, and ids can contain spaces.
    cmd.raw_arg(format!("\"{}\"", apps_folder_path(id)));
    for var in clean_env {
        cmd.env_remove(var);
    }
    if let Err(e) = cmd.spawn() {
        log::error!("launcher: launch {id}: {e}");
    }
}

/// Runs `explorer.exe` with `arg` (quoted by the caller): Explorer opens files with their
/// default app and folders in a window, and the app gets the shell's environment, not ours.
fn explorer(arg: String, clean_env: &[String]) {
    unsafe {
        let _ = AllowSetForegroundWindow(ASFW_ANY);
    }
    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new("explorer.exe");
    cmd.raw_arg(&arg);
    for var in clean_env {
        cmd.env_remove(var);
    }
    if let Err(e) = cmd.spawn() {
        log::error!("launcher: explorer {arg}: {e}");
    }
}

/// Opens a file with its default app, or a folder in Explorer.
pub fn open_path(path: &str, clean_env: &[String]) {
    explorer(format!("\"{path}\""), clean_env);
}

/// Opens the folder holding `path` in Explorer with it selected.
pub fn reveal(path: &str, clean_env: &[String]) {
    explorer(format!("/select,\"{path}\""), clean_env);
}

/// Runs a command line as the Run dialog would (`start` resolves App Paths such as `code`).
pub fn exec(line: &str, clean_env: &[String]) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    unsafe {
        let _ = AllowSetForegroundWindow(ASFW_ANY);
    }
    let mut cmd = std::process::Command::new("cmd");
    cmd.raw_arg(format!("/C start \"\" {line}")).creation_flags(CREATE_NO_WINDOW);
    for var in clean_env {
        cmd.env_remove(var);
    }
    if let Err(e) = cmd.spawn() {
        log::error!("launcher: exec '{line}': {e}");
    }
}

/// Puts `text` on the clipboard as Unicode text.
pub fn copy_text(text: &str) {
    use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
    use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
    use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
    use windows::Win32::System::Ole::CF_UNICODETEXT;
    let units: Vec<u16> = text.encode_utf16().chain([0]).collect();
    unsafe {
        // Another app may hold the clipboard for a moment.
        let mut opened = false;
        for _ in 0..10 {
            if OpenClipboard(None).is_ok() {
                opened = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        if !opened {
            log::warn!("launcher: clipboard busy; path not copied");
            return;
        }
        let _ = EmptyClipboard();
        let copied = (|| -> windows::core::Result<()> {
            let mem: HGLOBAL = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2)?;
            let dst = GlobalLock(mem) as *mut u16;
            if dst.is_null() {
                let _ = GlobalFree(Some(mem));
                return Err(windows::core::Error::from_thread());
            }
            std::ptr::copy_nonoverlapping(units.as_ptr(), dst, units.len());
            let _ = GlobalUnlock(mem);
            // On success the clipboard owns the memory.
            if let Err(e) = SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(mem.0))) {
                let _ = GlobalFree(Some(mem));
                return Err(e);
            }
            Ok(())
        })();
        let _ = CloseClipboard();
        if let Err(e) = copied {
            log::warn!("launcher: copying to the clipboard: {e}");
        }
    }
}

/// Makes `hwnd` the foreground window, escalating past Windows' foreground lock: a plain
/// request, then after a synthetic (zero) mouse move, then after a synthetic Alt tap.
pub fn bring_to_front(hwnd: isize) {
    let h = HWND(hwnd as *mut c_void);
    let size = std::mem::size_of::<INPUT>() as i32;
    let try_front = || unsafe { SetForegroundWindow(h).as_bool() && GetForegroundWindow() == h };
    unsafe {
        if !IsWindowVisible(h).as_bool() || try_front() {
            return;
        }
        let nudge = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 { mi: MOUSEINPUT { dwFlags: MOUSEEVENTF_MOVE, ..Default::default() } },
        };
        SendInput(&[nudge], size);
        if try_front() {
            return;
        }
        let alt = |flags| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VK_MENU, dwFlags: flags, ..Default::default() } },
        };
        SendInput(&[alt(Default::default()), alt(KEYEVENTF_KEYUP)], size);
        let _ = SetForegroundWindow(h);
    }
}

/// Takes `hwnd` out of DWM's cloak if it's in it: on screen with what it last presented.
pub fn uncloak(hwnd: isize) {
    use windows::Win32::Graphics::Dwm::{DWMWA_CLOAK, DWMWA_CLOAKED, DwmGetWindowAttribute, DwmSetWindowAttribute};
    let h = HWND(hwnd as *mut c_void);
    unsafe {
        let mut cloaked = 0u32;
        let read = DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut c_void, 4);
        if read.is_ok() && cloaked != 0 {
            let off = 0i32; // a BOOL
            let _ = DwmSetWindowAttribute(h, DWMWA_CLOAK, &off as *const i32 as *const c_void, 4);
        }
    }
}

/// A window's rect on screen as DWM draws it (no invisible resize borders), in physical pixels.
pub fn window_rect(hwnd: isize) -> Option<(i32, i32, i32, i32)> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
    let mut r = RECT::default();
    unsafe {
        DwmGetWindowAttribute(
            HWND(hwnd as *mut c_void),
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut r as *mut RECT as *mut c_void,
            std::mem::size_of::<RECT>() as u32,
        )
        .ok()?;
    }
    Some((r.left, r.top, r.right - r.left, r.bottom - r.top))
}

/// The foreground window (0 for none).
pub fn foreground() -> isize {
    unsafe { GetForegroundWindow().0 as isize }
}

/// Whether `hwnd` is the foreground window.
pub fn is_foreground(hwnd: isize) -> bool {
    unsafe { GetForegroundWindow().0 as isize == hwnd }
}

/// Windows 11 chrome for the borderless launcher: rounded corners and a hairline border in
/// `border` (sRGB), drawn by DWM outside our pixels so they cost nothing to paint.
pub fn style_window(hwnd: isize, border: [u8; 3]) {
    use windows::Win32::Graphics::Dwm::{
        DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
    };
    let h = HWND(hwnd as *mut c_void);
    let corner = DWMWCP_ROUND;
    // COLORREF is 0x00BBGGRR.
    let colour = border[0] as u32 | (border[1] as u32) << 8 | (border[2] as u32) << 16;
    unsafe {
        let _ = DwmSetWindowAttribute(h, DWMWA_WINDOW_CORNER_PREFERENCE, &corner as *const _ as *const c_void, 4);
        let _ = DwmSetWindowAttribute(h, DWMWA_BORDER_COLOR, &colour as *const _ as *const c_void, 4);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premultiplied_icons_are_unpremultiplied() {
        // Half-transparent pure red, premultiplied, as BGRA.
        let out = bgra_to_rgba(vec![0, 0, 128, 128]);
        assert_eq!(out, vec![255, 0, 0, 128]);
    }

    #[test]
    fn straight_alpha_is_kept() {
        // Bright colour over low alpha can't be premultiplied.
        let out = bgra_to_rgba(vec![200, 0, 0, 10]);
        assert_eq!(out, vec![0, 0, 200, 10]);
    }

    #[test]
    fn alphaless_bitmaps_are_opaque() {
        let out = bgra_to_rgba(vec![1, 2, 3, 0]);
        assert_eq!(out, vec![3, 2, 1, 255]);
    }

    #[test]
    fn documents_and_uninstallers_are_skipped() {
        assert!(wanted("Firefox", "308046B0AF4A39CB"));
        assert!(!wanted("Uninstall Foo", "C:\\foo\\unins000.exe"));
        assert!(!wanted("Foo Readme", "{7C5A40EF}\\Foo\\readme.txt"));
        assert!(wanted("Steam Game", "steam.url"), "Steam games are .url shortcuts");
    }

    #[test]
    fn manifests_name_the_executable() {
        let xml = r#"<Package><Applications>
  <Application Id="App" Executable="WindowsTerminal.exe" EntryPoint="Windows.FullTrustApplication">
  </Application>
  <Application
    Id="Other" Executable="PaintApp\mspaint.exe"/>
</Applications></Package>"#;
        assert_eq!(manifest_executable(xml, "App").as_deref(), Some("WindowsTerminal.exe"));
        assert_eq!(manifest_executable(xml, "Other").as_deref(), Some("PaintApp\\mspaint.exe"));
        assert_eq!(manifest_executable(xml, "Missing"), None);
    }
}
