//! CPU, memory, battery and clock readings on macOS: not written yet (host_statistics, IOKit,
//! localtime_r). The bar shows zeros, no battery, and the time in UTC.

use crate::sysinfo::LocalTime;

pub fn cpu_times() -> Option<(u64, u64)> {
    super::unsupported("CPU, memory and battery readings");
    None
}

pub fn memory_load() -> u32 {
    0
}

pub fn battery() -> Option<(u32, bool)> {
    None
}

/// UTC from the system clock: the time zone needs libc, which comes with the real back end.
pub fn local_time() -> LocalTime {
    let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    from_unix(d.as_secs(), d.subsec_millis() as u16)
}

/// Seconds since 1970-01-01 UTC as a date and time, UTC.
fn from_unix(secs: u64, millis: u16) -> LocalTime {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    LocalTime {
        year: year as u16,
        month: month as u16,
        // 1970-01-01 was a Thursday.
        day_of_week: (days + 4).rem_euclid(7) as u16,
        day: day as u16,
        hour: (rem / 3600) as u16,
        minute: (rem / 60 % 60) as u16,
        second: (rem % 60) as u16,
        millis,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_from_unix_seconds() {
        let t = |y, mo, dow, d, h, mi, s| LocalTime { year: y, month: mo, day_of_week: dow, day: d, hour: h, minute: mi, second: s, millis: 0 };
        assert_eq!(from_unix(0, 0), t(1970, 1, 4, 1, 0, 0, 0));
        assert_eq!(from_unix(1795529109, 0), t(2026, 11, 2, 24, 14, 5, 9));
        assert_eq!(from_unix(1709251199, 0), t(2024, 2, 4, 29, 23, 59, 59));
    }
}
