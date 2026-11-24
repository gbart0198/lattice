//! CPU, memory, battery and clock readings for the bar.

use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::SystemInformation::{GetLocalTime, GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::GetSystemTimes;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SysInfo {
    pub cpu: u32,
    pub mem: u32,
    /// (percent, charging); None on machines without a battery.
    pub battery: Option<(u32, bool)>,
    pub clock: String,
}

pub struct Sampler {
    last: Option<(u64, u64)>, // (idle, total)
}

fn ft(f: FILETIME) -> u64 {
    ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
}

impl Sampler {
    pub fn new() -> Self {
        let mut s = Sampler { last: None };
        s.cpu();
        s
    }

    fn cpu(&mut self) -> u32 {
        let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
        if unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.is_err() {
            return 0;
        }
        let idle = ft(idle);
        let total = ft(kernel) + ft(user); // kernel time includes idle time
        let pct = match self.last {
            Some((pi, pt)) if total > pt => {
                let dt = (total - pt) as f64;
                let di = idle.saturating_sub(pi) as f64;
                ((1.0 - di / dt) * 100.0).round().clamp(0.0, 100.0) as u32
            }
            _ => 0,
        };
        self.last = Some((idle, total));
        pct
    }

    pub fn sample(&mut self, clock_format: &str) -> SysInfo {
        let mut ms = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
        let mem = if unsafe { GlobalMemoryStatusEx(&mut ms) }.is_ok() { ms.dwMemoryLoad } else { 0 };
        let mut ps = SYSTEM_POWER_STATUS::default();
        let battery = if unsafe { GetSystemPowerStatus(&mut ps) }.is_ok() && ps.BatteryFlag & 128 == 0 && ps.BatteryLifePercent <= 100 {
            Some((ps.BatteryLifePercent as u32, ps.ACLineStatus == 1))
        } else {
            None
        };
        SysInfo { cpu: self.cpu(), mem, battery, clock: format_time(&unsafe { GetLocalTime() }, clock_format) }
    }
}

const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December",
];

/// A small strftime subset: %H %M %S %I %p %a %A %b %B %d %e %m %Y %y %%.
pub fn format_time(t: &SYSTEMTIME, fmt: &str) -> String {
    let mut out = String::new();
    let mut chars = fmt.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let day = DAYS[(t.wDayOfWeek as usize) % 7];
        let month = MONTHS[(t.wMonth as usize).clamp(1, 12) - 1];
        let h12 = match t.wHour % 12 {
            0 => 12,
            h => h,
        };
        match chars.next() {
            Some('H') => out.push_str(&format!("{:02}", t.wHour)),
            Some('M') => out.push_str(&format!("{:02}", t.wMinute)),
            Some('S') => out.push_str(&format!("{:02}", t.wSecond)),
            Some('I') => out.push_str(&format!("{h12:02}")),
            Some('l') => out.push_str(&h12.to_string()),
            Some('p') => out.push_str(if t.wHour < 12 { "AM" } else { "PM" }),
            Some('a') => out.push_str(&day[..3]),
            Some('A') => out.push_str(day),
            Some('b') => out.push_str(&month[..3]),
            Some('B') => out.push_str(month),
            Some('d') => out.push_str(&format!("{:02}", t.wDay)),
            Some('e') => out.push_str(&t.wDay.to_string()),
            Some('m') => out.push_str(&format!("{:02}", t.wMonth)),
            Some('Y') => out.push_str(&t.wYear.to_string()),
            Some('y') => out.push_str(&format!("{:02}", t.wYear % 100)),
            Some('%') => out.push('%'),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        let t = SYSTEMTIME { wYear: 2026, wMonth: 11, wDayOfWeek: 2, wDay: 24, wHour: 14, wMinute: 5, wSecond: 9, wMilliseconds: 0 };
        assert_eq!(format_time(&t, "%a %b %d  %H:%M"), "Tue Nov 24  14:05");
        assert_eq!(format_time(&t, "%I:%M %p"), "02:05 PM");
    }
}
