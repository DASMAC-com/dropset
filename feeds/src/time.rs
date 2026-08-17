// cspell:word Hinnant
//! Wall-clock helpers shared by the feed windows.
//!
//! The civil-date arithmetic here exists so an adapter whose venue publishes a
//! *civil* timestamp (`2026-08-14 00:26:00`) rather than an epoch second can
//! still produce a [`crate::venues::Candle`] without this crate taking a
//! `chrono` / `time` dependency — the same reason `cex_prices.bucket_start` is
//! an epoch-second `BIGINT`. Two venues need it, so it lives here rather than
//! in whichever adapter reached for it first.

use anyhow::{anyhow, Context, Result};
use std::time::{SystemTime, UNIX_EPOCH};

/// Current wall-clock epoch second — the unit every poll window, bucket
/// boundary, and cursor position in this crate is expressed in. A clock before
/// the epoch reads as `0` rather than panicking: a feed that mis-reads the time
/// re-polls a window it already has (the sinks are idempotent), which is
/// cheaper than taking the process down.
pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Days since the Unix epoch for a proleptic-Gregorian civil date.
///
/// Hinnant's `days_from_civil`, which is exact for every date the calendar
/// defines and needs no tables. It exists so an adapter whose venue publishes a
/// *civil* timestamp (`2026-08-14 00:26:00`) rather than an epoch second can
/// still reach [`crate::venues::Candle::bucket_start`] without this crate
/// taking a `chrono` / `time` dependency — the same reason `cex_prices` stores
/// `bucket_start` as a `BIGINT` in the first place.
///
/// `m` is 1-based. Out-of-range inputs are the caller's to reject; this is
/// arithmetic, not validation.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    // Shift the year to start in March, so the leap day lands at the end and
    // the month-length pattern becomes a single linear expression.
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let year_of_era = y - era * 400; // [0, 399]
    let shifted_month = if m > 2 { m - 3 } else { m + 9 }; // March = 0
    let day_of_year = (153 * shifted_month + 2) / 5 + d - 1; // [0, 365]
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year; // [0, 146096]
                                                                                            // 719468 is the day count from 0000-03-01 to 1970-01-01.
    era * 146_097 + day_of_era - 719_468
}

/// Epoch second for a civil UTC date and time.
///
/// UTC only, deliberately: every venue this crate reads is asked for UTC
/// explicitly (Twelve Data defaults to *exchange-local* time and must be sent
/// `timezone=UTC`), so accepting an offset here would invite a caller to
/// normalize a timestamp this function cannot check.
pub fn civil_to_epoch_secs(
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
) -> i64 {
    days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second
}

/// The proleptic-Gregorian civil date `days` after the Unix epoch — Hinnant's
/// `civil_from_days`, the exact inverse of [`days_from_civil`]. Returns
/// `(year, month, day)` with a 1-based month.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097; // [0, 146096]
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153; // March = 0
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Render an epoch second as `YYYY-MM-DD HH:MM:SS` in UTC — the civil format
/// the venues that speak civil time also *accept* as a query bound.
pub fn format_civil_utc(epoch: i64) -> String {
    let (days, secs) = (epoch.div_euclid(86_400), epoch.rem_euclid(86_400));
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (secs / 3_600, (secs % 3_600) / 60, secs % 60);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
}

/// Parse a venue's civil timestamp into an epoch second.
///
/// Accepts `YYYY-MM-DD HH:MM:SS` and a bare `YYYY-MM-DD` (which a daily series
/// returns), with the seconds field optional.
///
/// **UTC is assumed, not parsed** — the string carries no zone. That is correct
/// only because every caller pins the venue to UTC on the request side (Twelve
/// Data defaults to *exchange-local* time and must be sent `timezone=UTC`;
/// Alpha Vantage documents its FX series as UTC). A venue that cannot be pinned
/// must not use this.
pub fn parse_civil_utc(datetime: &str) -> Result<i64> {
    let (date, time) = datetime.split_once(' ').unwrap_or((datetime, "00:00:00"));
    let mut date_parts = date.split('-');
    let mut time_parts = time.split(':');
    let field = |part: Option<&str>, what: &str| -> Result<i64> {
        part.filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("civil timestamp {datetime:?} has no {what}"))?
            .parse::<i64>()
            .with_context(|| format!("civil timestamp {datetime:?} has a bad {what}"))
    };
    let year = field(date_parts.next(), "year")?;
    let month = field(date_parts.next(), "month")?;
    let day = field(date_parts.next(), "day")?;
    let hour = field(time_parts.next(), "hour")?;
    let minute = field(time_parts.next(), "minute")?;
    let second = match time_parts.next() {
        Some(s) => s
            .parse::<i64>()
            .with_context(|| format!("civil timestamp {datetime:?} has a bad second"))?,
        None => 0,
    };
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return Err(anyhow!("civil timestamp {datetime:?} is not a real date"));
    }
    Ok(civil_to_epoch_secs(year, month, day, hour, minute, second))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_epoch_itself_is_zero() {
        assert_eq!(civil_to_epoch_secs(1970, 1, 1, 0, 0, 0), 0);
    }

    #[test]
    fn matches_timestamps_captured_from_the_venues() {
        // Ground truth: OANDA returned this candle in both RFC3339 and UNIX
        // form, which pins the pair exactly.
        assert_eq!(civil_to_epoch_secs(2026, 8, 14, 0, 51, 0), 1_786_668_660);
        // The Saturday the weekend probes used, derived rather than hardcoded —
        // an earlier hand-written constant for this date was four days wrong.
        assert_eq!(civil_to_epoch_secs(2026, 8, 8, 0, 0, 0), 1_786_147_200);
    }

    #[test]
    fn handles_leap_days_and_century_rules() {
        // 2000 was a leap year (divisible by 400); 1900 was not.
        assert_eq!(
            civil_to_epoch_secs(2000, 3, 1, 0, 0, 0) - civil_to_epoch_secs(2000, 2, 28, 0, 0, 0),
            2 * 86_400
        );
        assert_eq!(
            civil_to_epoch_secs(1900, 3, 1, 0, 0, 0) - civil_to_epoch_secs(1900, 2, 28, 0, 0, 0),
            86_400
        );
    }

    #[test]
    fn is_monotonic_across_a_year_boundary() {
        let dec31 = civil_to_epoch_secs(2025, 12, 31, 23, 59, 0);
        let jan1 = civil_to_epoch_secs(2026, 1, 1, 0, 0, 0);
        assert_eq!(jan1 - dec31, 60);
    }

    #[test]
    fn round_trips_against_now() {
        // A sanity check that the arithmetic lands in the right era at all.
        let now = now_secs();
        assert!(now > civil_to_epoch_secs(2020, 1, 1, 0, 0, 0));
        assert!(now < civil_to_epoch_secs(2100, 1, 1, 0, 0, 0));
    }

    #[test]
    fn parses_the_formats_the_venues_actually_send() {
        // Twelve Data's minute bar.
        assert_eq!(
            parse_civil_utc("2026-08-14 00:26:00").unwrap(),
            civil_to_epoch_secs(2026, 8, 14, 0, 26, 0)
        );
        // Alpha Vantage's daily series key — a bare date.
        assert_eq!(
            parse_civil_utc("2026-08-13").unwrap(),
            civil_to_epoch_secs(2026, 8, 13, 0, 0, 0)
        );
    }

    #[test]
    fn rejects_a_string_that_is_not_a_civil_timestamp() {
        // An RFC3339 stamp is the realistic mistake: it would otherwise parse
        // its way to a plausible-looking wrong bucket.
        assert!(parse_civil_utc("2026-08-14T00:51:00Z").is_err());
        assert!(parse_civil_utc("not-a-time").is_err());
        assert!(parse_civil_utc("").is_err());
        assert!(parse_civil_utc("2026-13-01").is_err());
    }

    #[test]
    fn renders_the_civil_query_bound_format() {
        assert_eq!(
            format_civil_utc(civil_to_epoch_secs(2026, 8, 14, 0, 26, 0)),
            "2026-08-14 00:26:00"
        );
        assert_eq!(
            format_civil_utc(civil_to_epoch_secs(2026, 1, 1, 0, 0, 0)),
            "2026-01-01 00:00:00"
        );
    }

    #[test]
    fn rendering_and_parsing_are_exact_inverses() {
        // Two separate implementations of the calendar; pin them together
        // rather than trusting either alone.
        for epoch in [
            civil_to_epoch_secs(2026, 8, 8, 0, 0, 0),
            civil_to_epoch_secs(2024, 2, 29, 23, 59, 59),
            civil_to_epoch_secs(1999, 12, 31, 12, 0, 0),
            civil_to_epoch_secs(2007, 6, 14, 0, 0, 0),
        ] {
            assert_eq!(parse_civil_utc(&format_civil_utc(epoch)).unwrap(), epoch);
        }
    }
}
