//! Times in words: a date and time in UTC ([`utc`], [`rfc3339`]), and how
//! long ago a time was ([`ago`]).

/// How long before `now_ms` the time `then_ms` was (both in milliseconds
/// since the Unix epoch): `just now` (under a minute, or later than now),
/// `2 min ago`, `3 h ago`, `1 day ago`, `2 days ago`.
pub fn ago(now_ms: u64, then_ms: u64) -> String {
    let secs = now_ms.saturating_sub(then_ms) / 1000;
    match secs {
        ..60 => "just now".to_owned(),
        60..3_600 => format!("{} min ago", secs / 60),
        3_600..86_400 => format!("{} h ago", secs / 3_600),
        86_400..172_800 => "1 day ago".to_owned(),
        _ => format!("{} days ago", secs / 86_400),
    }
}

/// `2026-10-15 12:34:56 UTC`.
pub fn utc(ms: u64) -> String {
    let (date, time) = civil(ms);
    format!("{date} {time} UTC")
}

/// `2026-10-15T12:34:56Z`.
pub fn rfc3339(ms: u64) -> String {
    let (date, time) = civil(ms);
    format!("{date}T{time}Z")
}

/// The date and time of `ms` milliseconds after the Unix epoch, in UTC:
/// `2026-10-15` and `12:34:56` (Howard Hinnant's `civil_from_days`).
fn civil(ms: u64) -> (String, String) {
    let secs = ms / 1000;
    let days = i64::try_from(secs / 86_400).unwrap_or(i64::MAX / 2);
    let of_day = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (
        format!("{year:04}-{month:02}-{day:02}"),
        format!(
            "{:02}:{:02}:{:02}",
            of_day / 3600,
            of_day % 3600 / 60,
            of_day % 60
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_utc() {
        assert_eq!(utc(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(rfc3339(1_792_091_602_130), "2026-10-15T19:13:22Z");
        assert_eq!(rfc3339(951_782_400_000), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(4_107_542_399_000), "2100-02-28T23:59:59Z");
    }

    #[test]
    fn ago_says_how_long_since_the_run_finished() {
        let then = 1_792_091_602_130;
        let after = |secs: u64| ago(then + secs * 1000, then);
        assert_eq!(after(0), "just now");
        assert_eq!(after(59), "just now");
        assert_eq!(after(60), "1 min ago");
        assert_eq!(after(150), "2 min ago");
        assert_eq!(after(3_599), "59 min ago");
        assert_eq!(after(3_600), "1 h ago");
        assert_eq!(after(3 * 3_600 + 59), "3 h ago");
        assert_eq!(after(86_400), "1 day ago");
        assert_eq!(after(2 * 86_400 + 1), "2 days ago");
        // A clock that went back: just now.
        assert_eq!(ago(then, then + 5_000), "just now");
    }
}
