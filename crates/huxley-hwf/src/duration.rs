//! Durations as written in HWF: one or more `<number><unit>` parts, with units
//! `ms`, `s`, `m`, `h`, `d`. Examples: `500ms`, `30s`, `1h30m`, `2d`.

use std::time::Duration;

/// Parses an HWF duration. Returns `None` for anything malformed, empty,
/// zero, or longer than a year.
pub fn parse(text: &str) -> Option<Duration> {
    const YEAR_MS: u128 = 366 * 24 * 3600 * 1000;
    let mut rest = text.trim();
    if rest.is_empty() {
        return None;
    }
    let mut total_ms: u128 = 0;
    let mut last_rank = u8::MAX;
    while !rest.is_empty() {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 || digits > 9 {
            return None;
        }
        let value: u128 = rest[..digits].parse().ok()?;
        rest = &rest[digits..];
        let unit_len = rest.bytes().take_while(u8::is_ascii_alphabetic).count();
        let (unit_ms, rank) = match &rest[..unit_len] {
            "d" => (86_400_000, 4),
            "h" => (3_600_000, 3),
            "m" => (60_000, 2),
            "s" => (1_000, 1),
            "ms" => (1, 0),
            _ => return None,
        };
        // Largest unit first, each at most once: `1h30m`, not `30m1h`.
        if rank >= last_rank {
            return None;
        }
        last_rank = rank;
        rest = &rest[unit_len..];
        total_ms += value * unit_ms;
    }
    if total_ms == 0 || total_ms > YEAR_MS {
        return None;
    }
    Some(Duration::from_millis(u64::try_from(total_ms).ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_compound_durations() {
        assert_eq!(parse("500ms"), Some(Duration::from_millis(500)));
        assert_eq!(parse("30s"), Some(Duration::from_secs(30)));
        assert_eq!(parse("1h30m"), Some(Duration::from_secs(5400)));
        assert_eq!(parse(" 2d "), Some(Duration::from_secs(172_800)));
        assert_eq!(parse("1m30s250ms"), Some(Duration::from_millis(90_250)));
    }

    #[test]
    fn rejects_malformed_durations() {
        for bad in [
            "",
            "5",
            "s",
            "1x",
            "1.5h",
            "-1s",
            "0s",
            "30m1h",
            "1h1h",
            "1 h",
            "400d",
            "9999999999s",
        ] {
            assert_eq!(parse(bad), None, "{bad:?}");
        }
    }
}
