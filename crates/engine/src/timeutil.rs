//! Calendar helpers without a date-time dependency.

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`). The inverse of `civil_from_days` in `transaction.rs`.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let mp = if m > 2 { m - 3 } else { m + 9 } as u64;
    let doy = (153 * mp + 2) / 5 + d as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

/// Parse a CIM/DMTF datetime, `yyyymmddHHMMSS.mmmmmm+UUU`, to Unix milliseconds
/// (UTC). `UUU` is the offset from UTC in minutes: local time = UTC + UUU.
pub fn cim_datetime_to_unix_ms(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() != 25 || b[14] != b'.' || (b[21] != b'+' && b[21] != b'-') {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<i64> {
        let part = s.get(from..to)?;
        if !part.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        part.parse().ok()
    };
    let (year, month, day) = (num(0, 4)?, num(4, 6)?, num(6, 8)?);
    let (hour, min, sec) = (num(8, 10)?, num(10, 12)?, num(12, 14)?);
    let micros = num(15, 21)?;
    let offset_min = num(22, 25)? * if b[21] == b'-' { -1 } else { 1 };

    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || min > 59 || sec > 60 {
        return None;
    }
    let days = days_from_civil(year, month as u32, day as u32);
    let local_secs = days * 86_400 + hour * 3600 + min * 60 + sec;
    let utc_secs = local_secs - offset_min * 60;
    let ms = utc_secs.checked_mul(1000)? + micros / 1000;
    u64::try_from(ms).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_from_civil_known_dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2024, 1, 1), 19_723);
        assert_eq!(days_from_civil(2025, 2, 28), 20_147);
        assert_eq!(days_from_civil(2000, 2, 29), 11_016);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
    }

    #[test]
    fn cim_datetimes_convert_to_utc() {
        // 2024-01-01 00:00:00 UTC
        assert_eq!(
            cim_datetime_to_unix_ms("20240101000000.000000+000"),
            Some(19_723 * 86_400_000)
        );
        // 19:00 local at UTC-5 (-300) is 00:00 UTC the next day.
        assert_eq!(
            cim_datetime_to_unix_ms("20231231190000.000000-300"),
            Some(19_723 * 86_400_000)
        );
        // 02:00 local at UTC+2 (+120) is 00:00 UTC.
        assert_eq!(
            cim_datetime_to_unix_ms("20240101020000.000000+120"),
            Some(19_723 * 86_400_000)
        );
        // Fractional seconds are kept to the millisecond.
        assert_eq!(
            cim_datetime_to_unix_ms("20240101000000.123456+000"),
            Some(19_723 * 86_400_000 + 123)
        );
    }

    #[test]
    fn malformed_datetimes_are_rejected_not_guessed() {
        for bad in [
            "",
            "20240101000000",
            "2024010100000.0000000+000",
            "20241301000000.000000+000", // month 13
            "20240101250000.000000+000", // hour 25
            "2024010100000a.000000+000",
            "20240101000000.000000*000",
        ] {
            assert_eq!(cim_datetime_to_unix_ms(bad), None, "{bad}");
        }
    }
}
