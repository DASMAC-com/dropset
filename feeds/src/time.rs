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
