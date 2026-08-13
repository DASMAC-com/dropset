//! Stale-quote invalidation — when a resting book must be killed rather than
//! left to expire on its own.
//!
//! A quote the bot stamped stays matchable until its level's `expiry_offset`
//! passes — up to 2_880 s (~48 min) on the deepest tier (§3 expiry table).
//! If the bot goes down, or the chain halts, or the feeds go dark, nobody
//! refreshes the reference in the meantime and takers can fill against a
//! price the bot no longer believes.
//!
//! Level expiry now *does* bound that exposure in wall-clock terms — it is
//! measured from the quote's `quote_unix` datum rather than its slot, so a
//! halt no longer freezes the countdown the way slot expiry did. But it is
//! a cap, not a policy: 48 minutes of unattended drift on the deepest tier
//! is far longer than the bot is willing to rest a book it is no longer
//! refreshing, so this mitigation stays required regardless.
//!
//! `FreezeVault` is admin-only (§4 — the bot's killswitch halts quoting, it
//! doesn't freeze), so the bot can't literally shut the vault. But matching
//! skips any vault whose reference price is invalid — the program's per-vault
//! gate is `has_valid_reference_price()`, which rejects zero — so stamping
//! `price = 0` through the ordinary leader-authorized hot path kills the whole
//! vault's book in one cheap instruction, leaving the `LiquidityProfile`
//! intact. The next valid reference re-arms it.
//!
//! This module is the decision half: given how stale the last live quote is and
//! whether the book is even matchable, should the bot spend an instruction on
//! that kill stamp? The send lives in `chain`, the sequencing in `tasks`, and
//! the wall-clock bookkeeping the age comes from in `quote_state`.

use std::time::Duration;

/// Why the bot killed a resting book — carried into the log line so an operator
/// can tell a restart from a mid-run feed outage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvalidateReason {
    /// Found resting at startup, older than the threshold (or of unknown age).
    /// The bot was down — or the chain was — while these levels stayed live.
    Startup,
    /// The bot is running but has stopped refreshing the reference for longer
    /// than the threshold — no usable feed. Quoting stopped; the prior quotes
    /// did not.
    QuotesStale,
    /// A kill-switch halt (§4). Zeroing the profile stops *new* levels from
    /// being materialized but leaves the resting ones matchable, and a halt is a
    /// decision that the bot no longer stands behind the price it stamped — so
    /// this one doesn't wait out the threshold. The exemption isn't encoded here:
    /// the caller expresses it by passing `age: None`, which
    /// [`should_invalidate`] already treats as stale.
    Halted,
}

/// Whether the bot must stamp the kill price before it is safe to leave this
/// vault's book resting.
///
/// - `reference_valid` — whether the on-chain reference price currently passes
///   the program's matching gate. A vault that is already dark (price zero, or
///   never stamped) has nothing to invalidate, so this is the first check: it
///   keeps a restart against an idle market from sending a pointless
///   transaction, and keeps the running path from re-sending the kill stamp
///   every cycle once it has landed.
/// - `age` — how long ago the bot last stamped a *live* reference on this
///   market, or `None` when there is no freshness evidence to appeal to.
///   **`None` reads as stale** — the resting book could be arbitrarily old, and
///   the cost of being wrong is one cheap stamp plus one cycle of darkness. That
///   covers both the case where the age is genuinely unknown (no persisted
///   record: a fresh checkout, a cleared state directory, a clock that moved
///   backwards) and the case where it is known but irrelevant, because the bot
///   has *decided* to stop standing behind the price
///   ([`InvalidateReason::Halted`]).
/// - `threshold` — the staleness bound (`InvalidateConfig::stale_after`).
pub fn should_invalidate(
    reference_valid: bool,
    age: Option<Duration>,
    threshold: Duration,
) -> bool {
    if !reference_valid {
        return false;
    }
    match age {
        None => true,
        Some(age) => age > threshold,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const THRESHOLD: Duration = Duration::from_secs(60);

    #[test]
    fn an_already_dark_book_needs_no_stamp() {
        // Nothing is matchable, so neither an unknown age nor a wildly stale
        // one justifies spending an instruction.
        assert!(!should_invalidate(false, None, THRESHOLD));
        assert!(!should_invalidate(
            false,
            Some(Duration::from_secs(3600)),
            THRESHOLD
        ));
    }

    #[test]
    fn unknown_age_reads_as_stale() {
        assert!(should_invalidate(true, None, THRESHOLD));
    }

    #[test]
    fn a_fresh_quote_is_left_alone() {
        // A restart inside the reference heartbeat: the resting book is still
        // priced off a reference the bot itself stamped moments ago.
        assert!(!should_invalidate(
            true,
            Some(Duration::from_secs(5)),
            THRESHOLD
        ));
    }

    #[test]
    fn the_threshold_is_exclusive() {
        // Exactly at the bound is still fresh; a hair past it is not.
        assert!(!should_invalidate(true, Some(THRESHOLD), THRESHOLD));
        assert!(should_invalidate(
            true,
            Some(THRESHOLD + Duration::from_millis(1)),
            THRESHOLD
        ));
    }

    #[test]
    fn a_long_outage_invalidates() {
        // The deepest ladder tier lives ~50 min of live chain; an outage that
        // long is exactly the pick-off window this exists to close.
        assert!(should_invalidate(
            true,
            Some(Duration::from_secs(50 * 60)),
            THRESHOLD
        ));
    }
}
