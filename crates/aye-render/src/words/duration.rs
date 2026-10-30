//! Durations in words ([`spaced`], [`seconds`]) and as numbers ([`secs`],
//! [`millis`]).

use std::time::Duration;

/// `2 ms` below a second, then as [`seconds`].
pub(crate) fn spaced(duration: Duration) -> String {
    if duration < Duration::from_secs(1) {
        format!("{} ms", duration.as_millis())
    } else {
        seconds(duration)
    }
}

/// `0.12 s` below a minute, then `1 min 5 s`.
pub(crate) fn seconds(duration: Duration) -> String {
    if duration < Duration::from_secs(60) {
        format!("{:.2} s", duration.as_secs_f64())
    } else {
        let whole = duration.as_secs();
        format!("{} min {} s", whole / 60, whole % 60)
    }
}

/// Seconds with two decimals, as a number.
pub(crate) fn secs(duration: Duration) -> f64 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "durations are far below 2^52 ms"
    )]
    let ms = millis(duration) as f64;
    (ms / 10.0).round() / 100.0
}

/// Whole milliseconds.
pub(crate) fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(spaced(Duration::from_micros(12_400)), "12 ms");
        assert_eq!(spaced(Duration::from_millis(840)), "840 ms");
        assert_eq!(spaced(Duration::from_millis(1234)), "1.23 s");
        assert_eq!(spaced(Duration::from_secs(65)), "1 min 5 s");
        assert_eq!(seconds(Duration::from_millis(120)), "0.12 s");
        assert!((secs(Duration::from_millis(1234)) - 1.23).abs() < f64::EPSILON);
        assert!((secs(Duration::from_millis(5)) - 0.01).abs() < f64::EPSILON);
    }
}
