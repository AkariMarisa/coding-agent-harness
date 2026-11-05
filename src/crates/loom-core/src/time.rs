//! Small UTC formatting helpers. We intentionally avoid a date-time crate here;
//! only two stable formats are needed for run ids and `tasks.json`.

/// Convert days since 1970-01-01 to `(year, month, day)`.
///
/// Howard Hinnant's civil-from-days algorithm. It is valid for a very wide
/// range and keeps the crate dependency-free.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    if month <= 2 {
        year += 1;
    }
    (year, month, day)
}

fn split_ts(ts_ms: u64) -> (i64, u32, u32, u32, u32, u32, u32) {
    let seconds = (ts_ms / 1000) as i64;
    let millis = (ts_ms % 1000) as u32;
    let days = seconds.div_euclid(86_400);
    let day_seconds = seconds.rem_euclid(86_400) as u32;
    let (year, month, day) = civil_from_days(days);
    let hour = day_seconds / 3_600;
    let minute = (day_seconds % 3_600) / 60;
    let second = day_seconds % 60;
    (year, month, day, hour, minute, second, millis)
}

/// RFC 3339 with millisecond precision, always in UTC.
pub fn rfc3339_ms(ts_ms: u64) -> String {
    let (year, month, day, hour, minute, second, millis) = split_ts(ts_ms);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// Compact UTC stamp used in generated run ids: `YYYYMMDD-HHMMSS`.
pub fn compact_utc(ts_ms: u64) -> String {
    let (year, month, day, hour, minute, second, _) = split_ts(ts_ms);
    format!("{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_epoch_and_known_instant() {
        assert_eq!(rfc3339_ms(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(rfc3339_ms(1_793_784_062_000), "2026-11-04T09:21:02.000Z");
        assert_eq!(compact_utc(1_793_784_062_000), "20261104-092102");
    }
}
