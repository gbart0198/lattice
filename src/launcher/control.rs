//! The Windows side of command mode: what the system offers (sleep states, audio outputs,
//! Wi-Fi) and running the system commands.
//!
//! - power: `LockWorkStation`, `SetSuspendState` and `ExitWindowsEx` (with SeShutdownPrivilege);
//! - audio: MMDevice to list outputs, and the shell's undocumented `IPolicyConfig` to set the
//!   default one, as Windows' own sound settings and every output switcher do;
//! - Wi-Fi: the WinRT `Windows.Devices.Radios` radio (no admin needed, unlike the WLAN API's
//!   radio state) and the WLAN API for saved networks and connecting.
//!
//! `snapshot` uses COM: call it on a thread that ran `system::com_init`.

use std::ffi::c_void;
use std::time::Duration;

use windows::Win32::Foundation::{HANDLE, LPARAM, LUID, WPARAM};
use windows::Win32::Security::{
    AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED, SE_SHUTDOWN_NAME,
    TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::{GUID, HRESULT, Interface, PCWSTR};

use super::commands::{AudioOutput, Network, Snapshot, System, Wifi};

/// What the system offers now. Each part that fails is left out (and logged).
pub fn snapshot() -> Snapshot {
    let (sleep, hibernate) = power_states();
    Snapshot {
        sleep,
        hibernate,
        audio: audio_outputs().unwrap_or_else(|e| {
            log::warn!("launcher: listing audio outputs: {e}");
            Vec::new()
        }),
        wifi: wifi(),
    }
}

/// Runs a system command. Power commands wait a moment so the launcher is off screen first.
pub fn run(cmd: &System) {
    let result = match cmd {
        System::Lock => unsafe { windows::Win32::System::Shutdown::LockWorkStation() },
        System::Sleep => {
            settle();
            suspend(false)
        }
        System::Hibernate => {
            settle();
            suspend(true)
        }
        System::Restart => exit_windows(windows::Win32::System::Shutdown::EWX_REBOOT),
        System::ShutDown => exit_windows(windows::Win32::System::Shutdown::EWX_POWEROFF),
        System::SignOut => exit_windows(windows::Win32::System::Shutdown::EWX_LOGOFF),
        System::AudioOutput(id) => set_default_output(id),
        System::Wifi(on) => set_wifi(*on),
        System::Connect { interface, profile } => connect(*interface, profile),
    };
    if let Err(e) = result {
        log::error!("launcher: {cmd:?}: {e}");
    }
}

fn settle() {
    std::thread::sleep(Duration::from_millis(200));
}

// ---------------------------------------------------------------------------------------------
// Power

/// (sleep, hibernate) are available. Modern Standby PCs have no S3 but sleep all the same.
fn power_states() -> (bool, bool) {
    use windows::Win32::System::Power::{GetPwrCapabilities, SYSTEM_POWER_CAPABILITIES};
    let mut caps = SYSTEM_POWER_CAPABILITIES::default();
    if !unsafe { GetPwrCapabilities(&mut caps) } {
        return (false, false);
    }
    (caps.SystemS3 || caps.AoAc, caps.SystemS4 && caps.HiberFilePresent)
}

fn suspend(hibernate: bool) -> windows::core::Result<()> {
    use windows::Win32::System::Power::{GetPwrCapabilities, SYSTEM_POWER_CAPABILITIES, SetSuspendState};
    let mut caps = SYSTEM_POWER_CAPABILITIES::default();
    let standby = unsafe { GetPwrCapabilities(&mut caps) } && caps.AoAc && !caps.SystemS3;
    if !hibernate && standby {
        // SetSuspendState can't enter Modern Standby; turning the display off, as the power
        // button does, can.
        use windows::Win32::UI::WindowsAndMessaging::{HWND_BROADCAST, PostMessageW, SC_MONITORPOWER, WM_SYSCOMMAND};
        return unsafe { PostMessageW(Some(HWND_BROADCAST), WM_SYSCOMMAND, WPARAM(SC_MONITORPOWER as usize), LPARAM(2)) };
    }
    enable_shutdown_privilege()?;
    if unsafe { SetSuspendState(hibernate, false, false) } {
        Ok(())
    } else {
        Err(windows::core::Error::from_thread())
    }
}

/// Restart, power off or sign out, letting apps ask to save their work first (no force).
fn exit_windows(how: windows::Win32::System::Shutdown::EXIT_WINDOWS_FLAGS) -> windows::core::Result<()> {
    use windows::Win32::System::Shutdown::{ExitWindowsEx, SHTDN_REASON_FLAG_PLANNED, SHTDN_REASON_MAJOR_OTHER};
    enable_shutdown_privilege()?;
    unsafe { ExitWindowsEx(how, SHTDN_REASON_MAJOR_OTHER | SHTDN_REASON_FLAG_PLANNED) }
}

/// Our token holds SeShutdownPrivilege disabled; shutting down and suspending need it enabled.
fn enable_shutdown_privilege() -> windows::core::Result<()> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token)?;
        let mut luid = LUID::default();
        let result = LookupPrivilegeValueW(None, SE_SHUTDOWN_NAME, &mut luid).and_then(|_| {
            let tp = TOKEN_PRIVILEGES {
                PrivilegeCount: 1,
                Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }],
            };
            AdjustTokenPrivileges(token, false, Some(&tp), 0, None, None)
        });
        let _ = windows::Win32::Foundation::CloseHandle(token);
        result
    }
}

// ---------------------------------------------------------------------------------------------
// Audio

fn audio_outputs() -> windows::core::Result<Vec<AudioOutput>> {
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::Media::Audio::{DEVICE_STATE_ACTIVE, IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender};
    use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree, STGM_READ};
    unsafe {
        let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let id_of = |d: &windows::Win32::Media::Audio::IMMDevice| -> Option<String> {
            let p = d.GetId().ok()?;
            let s = p.to_string().ok();
            CoTaskMemFree(Some(p.0 as *const c_void));
            s
        };
        let default = devices.GetDefaultAudioEndpoint(eRender, eConsole).ok().and_then(|d| id_of(&d));
        let all = devices.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
        let mut out = Vec::new();
        for i in 0..all.GetCount()? {
            let Ok(d) = all.Item(i) else { continue };
            let Some(id) = id_of(&d) else { continue };
            let name = d
                .OpenPropertyStore(STGM_READ)
                .and_then(|p| p.GetValue(&PKEY_Device_FriendlyName))
                .map(|v| v.to_string())
                .unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            out.push(AudioOutput { default: default.as_deref() == Some(id.as_str()), id, name });
        }
        out.sort_by_key(|d| d.name.to_lowercase());
        Ok(out)
    }
}

/// `IPolicyConfig` (Windows 7 and later). Undocumented, so its one method we call is reached
/// through the vtable by slot: `SetDefaultEndpoint`, after IUnknown's three and ten others.
const CLSID_POLICY_CONFIG: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);
const IID_POLICY_CONFIG: GUID = GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);
const SET_DEFAULT_ENDPOINT: usize = 13;

fn set_default_output(id: &str) -> windows::core::Result<()> {
    use windows::Win32::Media::Audio::{eCommunications, eConsole, eMultimedia};
    use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
    use windows::core::IUnknown;
    type SetDefault = unsafe extern "system" fn(*mut c_void, PCWSTR, i32) -> HRESULT;
    let wide = windows::core::HSTRING::from(id);
    unsafe {
        let unknown: IUnknown = CoCreateInstance(&CLSID_POLICY_CONFIG, None, CLSCTX_ALL)?;
        let mut raw = std::ptr::null_mut();
        unknown.query(&IID_POLICY_CONFIG, &mut raw).ok()?;
        let vtable = *(raw as *const *const usize);
        let set: SetDefault = std::mem::transmute(*vtable.add(SET_DEFAULT_ENDPOINT));
        // All three roles, as picking an output in Windows' sound settings does.
        let mut result = Ok(());
        for role in [eConsole, eMultimedia, eCommunications] {
            result = result.and(set(raw, PCWSTR(wide.as_ptr()), role.0).ok());
        }
        // Balance the reference `query` added.
        drop(IUnknown::from_raw(raw));
        result
    }
}

// ---------------------------------------------------------------------------------------------
// Wi-Fi

/// The Wi-Fi radio and saved networks, or None without a Wi-Fi radio.
fn wifi() -> Option<Wifi> {
    let on = on_mta(|| wifi_radio().and_then(|r| r.State()))
        .inspect_err(|e| log::debug!("launcher: no Wi-Fi radio: {e}"))
        .ok()?
        == windows::Devices::Radios::RadioState::On;
    Some(Wifi { on, networks: if on { networks() } else { Vec::new() } })
}

fn set_wifi(on: bool) -> windows::core::Result<()> {
    use windows::Devices::Radios::{Radio, RadioAccessStatus, RadioState};
    on_mta(move || {
        let access = Radio::RequestAccessAsync()?.join()?;
        if access != RadioAccessStatus::Allowed {
            return Err(windows::core::Error::new(windows::Win32::Foundation::E_ACCESSDENIED, "radio access denied"));
        }
        let status = wifi_radio()?.SetStateAsync(if on { RadioState::On } else { RadioState::Off })?.join()?;
        if status != RadioAccessStatus::Allowed {
            return Err(windows::core::Error::new(windows::Win32::Foundation::E_ACCESSDENIED, "radio access denied"));
        }
        Ok(())
    })
}

fn wifi_radio() -> windows::core::Result<windows::Devices::Radios::Radio> {
    use windows::Devices::Radios::{Radio, RadioKind};
    for radio in Radio::GetRadiosAsync()?.join()? {
        if radio.Kind()? == RadioKind::WiFi {
            return Ok(radio);
        }
    }
    Err(windows::core::Error::new(windows::Win32::Foundation::E_FAIL, "no Wi-Fi radio"))
}

/// Runs `f` on a short-lived multithreaded-apartment thread: blocking on a WinRT async call
/// from the worker's single-threaded apartment could wait on its own message loop.
fn on_mta<T: Send + 'static>(f: impl FnOnce() -> windows::core::Result<T> + Send + 'static) -> windows::core::Result<T> {
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
    std::thread::spawn(move || unsafe {
        let init = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let r = f();
        if init {
            CoUninitialize();
        }
        r
    })
    .join()
    .unwrap_or_else(|_| Err(windows::core::Error::new(windows::Win32::Foundation::E_FAIL, "Wi-Fi thread panicked")))
}

/// A WLAN API client handle, closed on drop.
struct Wlan(HANDLE);

impl Wlan {
    fn open() -> Option<Wlan> {
        use windows::Win32::NetworkManagement::WiFi::WlanOpenHandle;
        let (mut version, mut handle) = (0u32, HANDLE::default());
        (unsafe { WlanOpenHandle(2, None, &mut version, &mut handle) } == 0).then_some(Wlan(handle))
    }

    fn interfaces(&self) -> Vec<GUID> {
        use windows::Win32::NetworkManagement::WiFi::{WLAN_INTERFACE_INFO_LIST, WlanEnumInterfaces, WlanFreeMemory};
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        unsafe {
            if WlanEnumInterfaces(self.0, None, &mut list) != 0 || list.is_null() {
                return Vec::new();
            }
            let items = std::slice::from_raw_parts((*list).InterfaceInfo.as_ptr(), (*list).dwNumberOfItems as usize);
            let out = items.iter().map(|i| i.InterfaceGuid).collect();
            WlanFreeMemory(list as *const c_void);
            out
        }
    }
}

impl Drop for Wlan {
    fn drop(&mut self) {
        unsafe {
            windows::Win32::NetworkManagement::WiFi::WlanCloseHandle(self.0, None);
        }
    }
}

/// Saved networks in range, the connected one first. Windows 11 hides scan results from apps
/// without location access; then every saved network is listed instead.
fn networks() -> Vec<Network> {
    use windows::Win32::NetworkManagement::WiFi::{
        WLAN_AVAILABLE_NETWORK_CONNECTED, WLAN_AVAILABLE_NETWORK_HAS_PROFILE, WLAN_AVAILABLE_NETWORK_LIST,
        WLAN_PROFILE_INFO_LIST, WlanFreeMemory, WlanGetAvailableNetworkList, WlanGetProfileList,
    };
    let Some(wlan) = Wlan::open() else { return Vec::new() };
    let mut out: Vec<Network> = Vec::new();
    for iface in wlan.interfaces() {
        let interface = iface.to_u128();
        unsafe {
            let mut list: *mut WLAN_AVAILABLE_NETWORK_LIST = std::ptr::null_mut();
            if WlanGetAvailableNetworkList(wlan.0, &iface, 0, None, &mut list) == 0 && !list.is_null() {
                let items = std::slice::from_raw_parts((*list).Network.as_ptr(), (*list).dwNumberOfItems as usize);
                for n in items.iter().filter(|n| n.dwFlags & WLAN_AVAILABLE_NETWORK_HAS_PROFILE != 0) {
                    let profile = wide_str(&n.strProfileName);
                    let connected = n.dwFlags & WLAN_AVAILABLE_NETWORK_CONNECTED != 0;
                    // A network shows once per BSS type and profile; keep one row per profile.
                    match out.iter_mut().find(|o| o.interface == interface && o.profile == profile) {
                        Some(o) => o.connected |= connected,
                        None if !profile.is_empty() => out.push(Network { interface, profile, connected }),
                        None => {}
                    }
                }
                WlanFreeMemory(list as *const c_void);
                continue;
            }
            let mut profiles: *mut WLAN_PROFILE_INFO_LIST = std::ptr::null_mut();
            if WlanGetProfileList(wlan.0, &iface, None, &mut profiles) == 0 && !profiles.is_null() {
                let items = std::slice::from_raw_parts((*profiles).ProfileInfo.as_ptr(), (*profiles).dwNumberOfItems as usize);
                out.extend(items.iter().map(|p| Network { interface, profile: wide_str(&p.strProfileName), connected: false }));
                WlanFreeMemory(profiles as *const c_void);
            }
        }
    }
    out.sort_by_key(|n| !n.connected);
    out
}

/// Connects with a saved profile; Windows reports the outcome in its own network flyout.
fn connect(interface: u128, profile: &str) -> windows::core::Result<()> {
    use windows::Win32::NetworkManagement::WiFi::{
        WLAN_CONNECTION_PARAMETERS, WlanConnect, dot11_BSS_type_any, wlan_connection_mode_profile,
    };
    let fail = |code: u32| windows::core::Error::from_hresult(HRESULT::from_win32(code));
    let wlan = Wlan::open().ok_or_else(|| fail(windows::Win32::Foundation::ERROR_NOT_SUPPORTED.0))?;
    let name = windows::core::HSTRING::from(profile);
    let params = WLAN_CONNECTION_PARAMETERS {
        wlanConnectionMode: wlan_connection_mode_profile,
        strProfile: PCWSTR(name.as_ptr()),
        pDot11Ssid: std::ptr::null_mut(),
        pDesiredBssidList: std::ptr::null_mut(),
        dot11BssType: dot11_BSS_type_any,
        dwFlags: 0,
    };
    match unsafe { WlanConnect(wlan.0, &GUID::from_u128(interface), &params, None) } {
        0 => Ok(()),
        code => Err(fail(code)),
    }
}

fn wide_str(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_strings_stop_at_nul() {
        let mut buf = [0u16; 8];
        for (i, c) in "Home".encode_utf16().enumerate() {
            buf[i] = c;
        }
        assert_eq!(wide_str(&buf), "Home");
        assert_eq!(wide_str(&"abc".encode_utf16().collect::<Vec<_>>()), "abc");
    }

    #[test]
    fn policy_config_slot() {
        // IUnknown (3) + GetMixFormat .. SetPropertyValue (10).
        assert_eq!(SET_DEFAULT_ENDPOINT, 3 + 10);
    }
}
