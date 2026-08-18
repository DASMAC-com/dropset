//! Wall-clock time for the quote datum and the book's expiry filter.
//!
//! Level expiry is wall-denominated: a leader stamps
//! `reference_price.quote_unix` when it quotes, each level dies
//! `expiry_offset_secs` later, and the engine gates on the
//! validator's `Clock.unix_timestamp`. Every off-chain consumer therefore
//! needs a "now" in the same units — the maker to stamp the datum, the
//! taker / TUI / router adapters to filter the book the way the engine
//! will.
//!
//! That "now" is read from the **host clock**, not from the chain, so
//! neither the maker's re-quote path nor a book poll pays an RPC for it.
//! The skew this accepts is immaterial at the timescales involved:
//! `Clock.unix_timestamp` is itself stake-weighted from validator vote
//! timestamps and accurate only to a few seconds, and the shortest
//! configured TIF is an order of magnitude longer. An NTP-synced host is
//! well inside that band.
//!
//! Two honest limits, per the spec's **SetReferencePrice**:
//!
//! - A host whose clock is *ahead* of the chain's stamps a datum in the
//!   chain's future, lengthening every level's effective life by the
//!   skew; one *behind* shortens it. Self-inflicted either way — the
//!   leader can only affect its own quotes.
//! - The **taker** side is not symmetric with that, and is worth stating
//!   plainly because the maker-side argument does not cover it. A quoting
//!   client filters the book by *its own* clock while the engine gates on
//!   the cluster's, so a client running behind sizes `min_out` against
//!   levels the engine will drop (the swap reverts on `min_out`, fees
//!   paid), and one running ahead reports "no liquidity" against a
//!   healthy book. Both failures are visible rather than silent, and both
//!   need a skew larger than the tier's whole wall TIF — but a consumer
//!   that cannot bound its own clock should quote against a chain-read
//!   time rather than this helper.
//! - Chain time can lag real time under congestion (the sysvar is clamped
//!   relative to slot pace), so a level's *real-world* life can exceed
//!   its nominal TIF. This is a staleness cap, not a hard deadline.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::clock::WallTime;

/// Host wall-clock time as unix seconds.
///
/// Returns `0` if the host clock is somehow before the unix epoch, which
/// is the fail-closed direction: a zero datum materializes every level
/// with an expiry in the distant past, so the vault simply stops
/// matching rather than quoting against an unbounded-life ladder.
pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// [`now_unix`] narrowed to the `u32` width the on-chain fields store and
/// tagged with its clock domain, for the book-filtering call sites
/// (`simulate_swap` / `resting_levels`). Saturates rather than wrapping
/// at both ends.
///
/// Returning a [`WallTime`] rather than a bare `u32` is the point: this
/// is the wall domain's entry into every off-chain consumer, and the
/// slot domain's `now` is a different type, so the two can no longer be
/// handed to each other's half of the dual gate.
pub fn now_unix_u32() -> WallTime {
    WallTime::new(now_unix().clamp(0, u32::MAX as i64) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sanity floor rather than an exact value: the point is that the
    /// helper returns real epoch seconds (not a slot, not a duration),
    /// so a unit mix-up at a call site is caught by the magnitude.
    #[test]
    fn now_is_a_plausible_epoch_second() {
        // 2020-01-01T00:00:00Z — any correct host clock is past this.
        assert!(now_unix() > 1_577_836_800);
        assert_eq!(now_unix_u32().get() as i64, now_unix());
    }
}
