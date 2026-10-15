//! Update cadence — when to fire the hot and cold paths (§3).
//!
//! `SetReferencePrice` is the cheap hot path (two `u64` stores), fired on
//! price drift, a heartbeat, or an inventory-skew shift — expected 2–6×/min.
//! `SetLiquidityProfile` is the cold path that rewrites the whole ladder,
//! fired on a large imbalance, a vol-regime change, or a daily heartbeat —
//! expected 1–3×/day. The vol-regime trigger needs a realized-σ estimator the
//! bot doesn't keep yet, so it is deferred; the imbalance trigger is driven by
//! the kill-switch action, leaving the daily heartbeat here.
//!
//! **Passive mode has no heartbeat on either path.** Every `SetReferencePrice`
//! *and* every `SetLiquidityProfile` arms a flush, and a flush re-centers every
//! flipped level — so in passive mode either write is a re-center, and each
//! one fires only on a trigger: price walked past the inner levels, an
//! imbalance entering the reshape band, or an operator `--recenter`. A calm
//! market gets zero stamps.

use crate::config::{StrategyConfig, StrategyMode};
use std::time::Duration;

/// Inputs to the `SetReferencePrice` decision for one tick.
#[derive(Clone, Copy, Debug)]
pub struct RefTrigger {
    /// The skewed reference price this tick would stamp.
    pub candidate: f64,
    /// The last reference price actually stamped on-chain, if any.
    pub last_set: Option<f64>,
    /// Wall-clock since the last `SetReferencePrice`.
    pub since_last_set: Duration,
    /// The inventory skew (bps) this tick computes.
    pub skew_bps: f64,
    /// The inventory skew (bps) applied at the last stamp.
    pub last_skew_bps: f64,
    /// Passive only: the inventory imbalance has just entered the reshape
    /// band — the rising edge, so a vault that stays imbalanced re-centers
    /// once rather than every tick.
    pub imbalance_entered: bool,
    /// Passive only: the operator asked for a re-center (`--recenter`).
    pub operator_recenter: bool,
}

/// Whether to fire `SetReferencePrice` this tick (§3 hot-path triggers).
pub fn should_set_reference(t: &RefTrigger, strat: &StrategyConfig) -> bool {
    let Some(last) = t.last_set else {
        // Nothing stamped yet — establish the reference.
        return true;
    };
    let drift_bps = (t.candidate - last).abs() / last * 10_000.0;
    match strat.mode {
        StrategyMode::Active => {
            drift_bps > strat.ref_drift_bps
                || t.since_last_set >= strat.ref_heartbeat
                || (t.skew_bps - t.last_skew_bps).abs() > strat.ref_skew_change_bps
        }
        StrategyMode::Passive => {
            // 1 bp = 100 ppm. An empty ladder covers nothing, so any drift is
            // outside it.
            let walked = strat
                .recenter_offset_ppm()
                .is_none_or(|bound| drift_bps * 100.0 > f64::from(bound));
            walked || t.imbalance_entered || t.operator_recenter
        }
    }
}

/// Whether to fire the `SetLiquidityProfile` daily heartbeat (§3 cold-path
/// trigger 3). The imbalance trigger (1) is handled by the kill-switch action;
/// the vol-regime trigger (2) is deferred until the bot tracks realized σ.
/// Never in passive mode, where a profile write re-centers the book.
pub fn should_set_profile_heartbeat(
    since_last_profile: Duration,
    daily: Duration,
    mode: StrategyMode,
) -> bool {
    mode == StrategyMode::Active && since_last_profile >= daily
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strat() -> StrategyConfig {
        StrategyConfig::default()
    }

    fn base() -> RefTrigger {
        RefTrigger {
            candidate: 0.73,
            last_set: Some(0.73),
            since_last_set: Duration::from_secs(1),
            skew_bps: 0.0,
            last_skew_bps: 0.0,
            imbalance_entered: false,
            operator_recenter: false,
        }
    }

    fn passive() -> StrategyConfig {
        StrategyConfig::default().into_passive()
    }

    #[test]
    fn passive_ignores_the_heartbeat_skew_and_small_drift() {
        let mut t = base();
        t.since_last_set = Duration::from_secs(30 * 24 * 3600);
        t.skew_bps = 15.0;
        // +9 bps: inside the passive bound (two inner rungs, 10 bps out).
        t.candidate = 0.73 * 1.0009;
        assert!(!should_set_reference(&t, &passive()));
    }

    #[test]
    fn passive_re_centers_once_price_walks_past_the_inner_levels() {
        let mut t = base();
        // The passive bound is the second rung, 10 bps out.
        t.candidate = 0.73 * 1.0011;
        assert!(should_set_reference(&t, &passive()));
        t.candidate = 0.73 * (1.0 - 0.0011);
        assert!(should_set_reference(&t, &passive()));
    }

    #[test]
    fn passive_re_centers_on_an_imbalance_or_an_operator_command() {
        let mut t = base();
        t.imbalance_entered = true;
        assert!(should_set_reference(&t, &passive()));
        let mut t = base();
        t.operator_recenter = true;
        assert!(should_set_reference(&t, &passive()));
    }

    #[test]
    fn passive_establishes_a_reference_when_none_is_live() {
        let mut t = base();
        t.last_set = None;
        assert!(should_set_reference(&t, &passive()));
    }

    #[test]
    fn active_ignores_the_passive_only_triggers() {
        let mut t = base();
        t.imbalance_entered = true;
        t.operator_recenter = true;
        assert!(!should_set_reference(&t, &strat()));
    }

    #[test]
    fn establishes_reference_when_never_set() {
        let mut t = base();
        t.last_set = None;
        assert!(should_set_reference(&t, &strat()));
    }

    #[test]
    fn quiet_tick_does_not_refresh() {
        assert!(!should_set_reference(&base(), &strat()));
    }

    #[test]
    fn price_drift_past_10bps_refreshes() {
        let mut t = base();
        t.candidate = 0.73 * 1.0011; // +11 bps
        assert!(should_set_reference(&t, &strat()));
    }

    #[test]
    fn heartbeat_refreshes() {
        let mut t = base();
        t.since_last_set = Duration::from_secs(31);
        assert!(should_set_reference(&t, &strat()));
    }

    #[test]
    fn skew_shift_past_2bps_refreshes() {
        let mut t = base();
        t.skew_bps = 3.0; // 3 bps shift from last 0
        assert!(should_set_reference(&t, &strat()));
    }

    #[test]
    fn profile_heartbeat_fires_after_a_day() {
        let day = Duration::from_secs(24 * 3600);
        let active = StrategyMode::Active;
        assert!(!should_set_profile_heartbeat(
            Duration::from_secs(3600),
            day,
            active
        ));
        assert!(should_set_profile_heartbeat(day, day, active));
        assert!(!should_set_profile_heartbeat(
            day * 30,
            day,
            StrategyMode::Passive
        ));
    }
}
