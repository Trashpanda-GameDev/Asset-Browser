//! Numbers, sizes, lengths and dates as people read them.

use std::time::{SystemTime, UNIX_EPOCH};

/// 68945 → "68,945"
pub fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// 0.35 → "0.35 s", 4.2 → "4.2 s", 75 → "1:15"
pub fn format_time(seconds: f64) -> String {
    let seconds = seconds.max(0.0);
    if seconds < 1.0 {
        format!("{seconds:.2} s")
    } else if seconds < 60.0 {
        format!("{seconds:.1} s")
    } else {
        let whole = seconds as u64;
        format!("{}:{:02}", whole / 60, whole % 60)
    }
}

/// 1 → "1 file", 1200 → "1,200 files"
pub fn plural(count: usize, noun: &str) -> String {
    if count == 1 { format!("1 {noun}") } else { format!("{} {noun}s", grouped(count)) }
}

/// A picture's size in pixels: "1024 × 512".
pub fn dimensions([w, h]: [u32; 2]) -> String {
    format!("{w} × {h}")
}

/// 44100 → "44.1 kHz"
pub fn sample_rate(hz: u32) -> String {
    format!("{:.1} kHz", hz as f32 / 1000.0)
}

/// 1 → "mono", 2 → "stereo", 6 → "6 ch"
pub fn channels(count: u16) -> String {
    match count {
        1 => "mono".to_owned(),
        2 => "stereo".to_owned(),
        n => format!("{n} ch"),
    }
}

/// A modification time (ns since the Unix epoch) relative to now: "5 min ago", "3 days ago", and
/// the date ("12 May 2024") beyond a week. Dates are UTC: there's no time zone database here, and
/// a date that's off by a few hours doesn't matter for telling files apart.
pub fn format_age(mtime_ns: i64) -> String {
    if mtime_ns <= 0 {
        return "–".to_owned();
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let then = mtime_ns / 1_000_000_000;
    let ago = now - then;
    match ago {
        i64::MIN..60 => "just now".to_owned(),
        60..3600 => format!("{} min ago", ago / 60),
        3600..86_400 => format!("{} h ago", ago / 3600),
        86_400..172_800 => "yesterday".to_owned(),
        172_800..604_800 => format!("{} days ago", ago / 86_400),
        _ => format_date(then),
    }
}

/// Seconds since the Unix epoch → "12 May 2024" (UTC).
fn format_date(seconds: i64) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let (year, month, day) = civil_from_days(seconds.div_euclid(86_400));
    format!("{day} {} {year}", MONTHS[(month - 1) as usize])
}

/// Days since 1970-01-01 → (year, month 1-12, day 1-31). Howard Hinnant's `civil_from_days`.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(format_date(0), "1 Jan 1970");
        assert_eq!(format_date(951_782_400), "29 Feb 2000");
        assert_eq!(format_date(1_715_472_000), "12 May 2024");
        assert_eq!(format_date(-86_400), "31 Dec 1969");
    }

    #[test]
    fn numbers() {
        assert_eq!(grouped(68_945), "68,945");
        assert_eq!(grouped(100), "100");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_time(75.0), "1:15");
    }
}
