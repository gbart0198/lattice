//! CPU, memory, battery and clock readings on Windows.

use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::SystemInformation::{GetLocalTime, GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::GetSystemTimes;

use crate::sysinfo::LocalTime;

fn ft(f: FILETIME) -> u64 {
    ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
}

/// CPU time since boot, all cores: (idle, total), in any unit as long as it's the same one.
pub fn cpu_times() -> Option<(u64, u64)> {
    let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.ok()?;
    // Kernel time includes idle time.
    Some((ft(idle), ft(kernel) + ft(user)))
}

/// Physical memory in use, percent (0 if it can't be read).
pub fn memory_load() -> u32 {
    let mut ms = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    if unsafe { GlobalMemoryStatusEx(&mut ms) }.is_ok() { ms.dwMemoryLoad } else { 0 }
}

/// (percent, charging); None on machines without a battery.
pub fn battery() -> Option<(u32, bool)> {
    let mut ps = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut ps) }.is_ok() && ps.BatteryFlag & 128 == 0 && ps.BatteryLifePercent <= 100 {
        Some((ps.BatteryLifePercent as u32, ps.ACLineStatus == 1))
    } else {
        None
    }
}

pub fn local_time() -> LocalTime {
    let t = unsafe { GetLocalTime() };
    LocalTime {
        year: t.wYear,
        month: t.wMonth,
        day_of_week: t.wDayOfWeek,
        day: t.wDay,
        hour: t.wHour,
        minute: t.wMinute,
        second: t.wSecond,
        millis: t.wMilliseconds,
    }
}
