//! Wall-clock helpers shared by the feed windows.

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
    let day_of_era =
        year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year; // [0, 146096]
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
        assert_eq!(
            civil_to_epoch_secs(2026, 8, 14, 0, 51, 0),
            1_786_668_660
        );
        // The Saturday the weekend probes used, derived rather than hardcoded —
        // an earlier hand-written constant for this date was four days wrong.
        assert_eq!(civil_to_epoch_secs(2026, 8, 8, 0, 0, 0), 1_786_147_200);
    }

    #[test]
    fn handles_leap_days_and_century_rules() {
        // 2000 was a leap year (divisible by 400); 1900 was not.
        assert_eq!(
            civil_to_epoch_secs(2000, 3, 1, 0, 0, 0)
                - civil_to_epoch_secs(2000, 2, 28, 0, 0, 0),
            2 * 86_400
        );
        assert_eq!(
            civil_to_epoch_secs(1900, 3, 1, 0, 0, 0)
                - civil_to_epoch_secs(1900, 2, 28, 0, 0, 0),
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
}
