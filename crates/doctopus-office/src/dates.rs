//! Date conversions for the serial and binary timestamps of Office files.

/// An Excel serial date (1900 system) as `YYYY-MM-DD`.
pub(crate) fn excel_date(serial: f64) -> Option<String> {
    if !(1.0..2_958_466.0).contains(&serial) {
        return None;
    }
    // Excel treats 1900 as a leap year: serial 60 is the nonexistent
    // 29 February, and later serials are one day off.
    if serial.floor() == 60.0 {
        return Some("1900-02-29".into());
    }
    let days = serial.floor() as i64 - if serial > 60.0 { 2 } else { 1 };
    let (y, m, d) = civil_from_days(days - 25_567);
    Some(format!("{y:04}-{m:02}-{d:02}"))
}

/// A Windows FILETIME (100 ns intervals since 1601-01-01 UTC) as
/// `YYYY-MM-DDTHH:MM:SSZ`; `None` for zero and dates before 1980, which
/// generators write for "unknown".
pub(crate) fn filetime(value: u64) -> Option<String> {
    const UNIX_EPOCH: i64 = 11_644_473_600;
    let secs = (value / 10_000_000) as i64 - UNIX_EPOCH;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    if !(1980..=9999).contains(&y) {
        return None;
    }
    Some(format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    ))
}

/// Days since 1970-01-01 to a calendar date (Howard Hinnant's algorithm).
pub(crate) fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_excel_dates() {
        assert_eq!(excel_date(45_292.0).as_deref(), Some("2024-01-01"));
        assert_eq!(excel_date(1.0).as_deref(), Some("1900-01-01"));
        assert_eq!(excel_date(59.0).as_deref(), Some("1900-02-28"));
        assert_eq!(excel_date(60.0).as_deref(), Some("1900-02-29"));
        assert_eq!(excel_date(61.0).as_deref(), Some("1900-03-01"));
    }

    #[test]
    fn converts_filetimes() {
        // 2008-01-04T11:51:36Z
        assert_eq!(
            filetime(128_439_210_960_000_000).as_deref(),
            Some("2008-01-04T11:51:36Z")
        );
        assert_eq!(filetime(0), None);
    }
}
