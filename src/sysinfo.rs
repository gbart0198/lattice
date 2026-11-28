//! CPU, memory, battery and clock readings for the bar. The OS reads them
//! (`platform::sysinfo`); this turns them into what the bar shows.

use crate::platform::sysinfo as os;

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

/// The local wall-clock time, as the clock and the log show it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LocalTime {
    pub year: u16,
    /// 1-12.
    pub month: u16,
    /// 0 is Sunday.
    pub day_of_week: u16,
    /// 1-31.
    pub day: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
    pub millis: u16,
}

impl Sampler {
    pub fn new() -> Self {
        let mut s = Sampler { last: None };
        s.cpu();
        s
    }

    fn cpu(&mut self) -> u32 {
        let Some((idle, total)) = os::cpu_times() else { return 0 };
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
        let (mem, battery) = (os::memory_load(), os::battery());
        SysInfo { cpu: self.cpu(), mem, battery, clock: format_time(&os::local_time(), clock_format) }
    }
}

const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December",
];

/// A small strftime subset: %H %M %S %I %p %a %A %b %B %d %e %m %Y %y %%.
pub fn format_time(t: &LocalTime, fmt: &str) -> String {
    let mut out = String::new();
    let mut chars = fmt.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let day = DAYS[(t.day_of_week as usize) % 7];
        let month = MONTHS[(t.month as usize).clamp(1, 12) - 1];
        let h12 = match t.hour % 12 {
            0 => 12,
            h => h,
        };
        match chars.next() {
            Some('H') => out.push_str(&format!("{:02}", t.hour)),
            Some('M') => out.push_str(&format!("{:02}", t.minute)),
            Some('S') => out.push_str(&format!("{:02}", t.second)),
            Some('I') => out.push_str(&format!("{h12:02}")),
            Some('l') => out.push_str(&h12.to_string()),
            Some('p') => out.push_str(if t.hour < 12 { "AM" } else { "PM" }),
            Some('a') => out.push_str(&day[..3]),
            Some('A') => out.push_str(day),
            Some('b') => out.push_str(&month[..3]),
            Some('B') => out.push_str(month),
            Some('d') => out.push_str(&format!("{:02}", t.day)),
            Some('e') => out.push_str(&t.day.to_string()),
            Some('m') => out.push_str(&format!("{:02}", t.month)),
            Some('Y') => out.push_str(&t.year.to_string()),
            Some('y') => out.push_str(&format!("{:02}", t.year % 100)),
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
        let t = LocalTime { year: 2026, month: 11, day_of_week: 2, day: 24, hour: 14, minute: 5, second: 9, millis: 0 };
        assert_eq!(format_time(&t, "%a %b %d  %H:%M"), "Tue Nov 24  14:05");
        assert_eq!(format_time(&t, "%I:%M %p"), "02:05 PM");
    }
}
