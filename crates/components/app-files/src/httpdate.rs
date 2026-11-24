//! Pure-std RFC 9110 IMF-fixdate formatter and HTTP-date parser.

use files_api::shell::civil_from_days;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct",
    "Nov", "Dec",
];

/// Format `time` as an RFC 9110 IMF-fixdate string (`Sun, 06 Nov 1994 08:49:37 GMT`).
pub fn format_http_date(time: SystemTime) -> String {
    let secs = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let total_days = (secs / 86_400) as i64;
    let day_secs = secs % 86_400;
    let dow = DAYS[total_days.rem_euclid(7) as usize];
    let (year, month, day) = civil_from_days(total_days);
    let hour = day_secs / 3_600;
    let min = (day_secs % 3_600) / 60;
    let sec = day_secs % 60;
    let mon = MONTHS[(month - 1) as usize];
    format!("{dow}, {day:02} {mon} {year:04} {hour:02}:{min:02}:{sec:02} GMT")
}

/// Parse an HTTP-date header (`IMF-fixdate`, obsolete `RFC 850`, or `asctime()`).
pub fn parse_http_date(raw: &str) -> Option<SystemTime> {
    let s = raw.trim();
    if let Some((_, rest)) = s.split_once(", ") {
        if rest.contains('-') {
            // RFC 850: "06-Nov-94 08:49:37 GMT"
            let mut parts = rest.split_whitespace();
            let date_part = parts.next()?;
            let time_part = parts.next()?;
            let tz = parts.next()?;
            if !tz.eq_ignore_ascii_case("GMT") || parts.next().is_some() {
                return None;
            }
            let mut dmy = date_part.split('-');
            let d: u8 = dmy.next()?.parse().ok()?;
            let m = parse_month(dmy.next()?)?;
            let yy: i64 = dmy.next()?.parse().ok()?;
            if dmy.next().is_some() {
                return None;
            }
            let year = if yy >= 70 { 1900 + yy } else { 2000 + yy };
            let (h, min, sec) = parse_hms(time_part)?;
            return compose_utc(year, m, d, h, min, sec);
        }

        // IMF-fixdate: "06 Nov 1994 08:49:37 GMT"
        let mut imf = rest.split_whitespace();
        let d: u8 = imf.next()?.parse().ok()?;
        let m = parse_month(imf.next()?)?;
        let y: i64 = imf.next()?.parse().ok()?;
        let t = imf.next()?;
        let gmt = imf.next()?;
        if !gmt.eq_ignore_ascii_case("GMT") || imf.next().is_some() {
            return None;
        }
        let (h, min, sec) = parse_hms(t)?;
        return compose_utc(y, m, d, h, min, sec);
    }

    // ANSI C asctime(): "Sun Nov  6 08:49:37 1994"
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() == 5 {
        let m = parse_month(parts[1])?;
        let d: u8 = parts[2].parse().ok()?;
        let (h, min, sec) = parse_hms(parts[3])?;
        let y: i64 = parts[4].parse().ok()?;
        return compose_utc(y, m, d, h, min, sec);
    }

    None
}

fn parse_month(token: &str) -> Option<u8> {
    MONTHS
        .iter()
        .position(|m| m.eq_ignore_ascii_case(token))
        .map(|i| (i + 1) as u8)
}

fn parse_hms(token: &str) -> Option<(u64, u64, u64)> {
    let mut parts = token.split(':');
    let h: u64 = parts.next()?.parse().ok()?;
    let m: u64 = parts.next()?.parse().ok()?;
    let s: u64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || h > 23 || m > 59 || s > 60 {
        return None;
    }
    Some((h, m, s))
}

fn compose_utc(
    year: i64,
    month: u8,
    day: u8,
    hour: u64,
    min: u64,
    sec: u64,
) -> Option<SystemTime> {
    if year < 1970 || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let days = days_from_civil(year, month, day);
    if days < 0 {
        return None;
    }
    let total_secs = (days as u64) * 86_400 + hour * 3_600 + min * 60 + sec;
    Some(UNIX_EPOCH + Duration::from_secs(total_secs))
}

const fn days_from_civil(y: i64, m: u8, d: u8) -> i64 {
    let y = y - if m <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let m = m as u64;
    let doy =
        (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + (d as u64) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + (doe as i64) - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_standard_formats() {
        let t = parse_http_date("Sun, 06 Nov 1994 08:49:37 GMT").unwrap();
        assert_eq!(format_http_date(t), "Sun, 06 Nov 1994 08:49:37 GMT");

        let t850 = parse_http_date("Sunday, 06-Nov-94 08:49:37 GMT").unwrap();
        assert_eq!(t, t850);

        let t_asc = parse_http_date("Sun Nov  6 08:49:37 1994").unwrap();
        assert_eq!(t, t_asc);
    }
}
