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
use crate::model::killswitch::Action;
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
    /// Passive only: this tick's fair mid, before skew.
    pub mid: f64,
    /// Passive only: the fair mid (before skew) the last stamp was taken at. The walk
    /// trigger compares mids rather than skewed references, so an inventory
    /// skew shift — which the book's own flip fills cause — never re-centers
    /// a calm market by itself.
    pub last_mid: Option<f64>,
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
    match strat.mode {
        StrategyMode::Active => {
            let drift_bps = (t.candidate - last).abs() / last * 10_000.0;
            drift_bps > strat.ref_drift_bps
                || t.since_last_set >= strat.ref_heartbeat
                || (t.skew_bps - t.last_skew_bps).abs() > strat.ref_skew_change_bps
        }
        StrategyMode::Passive => {
            // 1 bp = 100 ppm. An empty ladder covers nothing, and an unknown
            // last mid gives nothing to measure from, so either reads as walked.
            let walked = match (strat.recenter_offset_ppm(), t.last_mid) {
                (Some(bound), Some(last_mid)) => {
                    let walk_bps = (t.mid - last_mid).abs() / last_mid * 10_000.0;
                    walk_bps * 100.0 > f64::from(bound)
                }
                _ => true,
            };
            walked || t.imbalance_entered || t.operator_recenter
        }
    }
}

/// The cold-path action and imbalance edge for one tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColdDecision {
    /// The action the cold path acts on.
    pub action: Action,
    /// The kill switch put the imbalance in the reshape band this tick.
    pub in_band: bool,
    /// Passive only: the imbalance entered the band since the last landed
    /// stamp — a re-center trigger.
    pub entered: bool,
}

/// Map the kill-switch action onto what the cold path does under `mode`.
///
/// Passive mode answers an imbalance in the reshape band (`Reshape` or
/// `FreezeSide`) with one skewed re-center as it enters the band, not a
/// reshape: a profile write would itself re-center through the flush it arms,
/// on every tick the shape differed. So passive quotes the standard ladder
/// through it. Halts keep their action in both modes. `was_in_band` says the
/// band has already been answered — set by any landed stamp (a re-center for
/// any reason re-skews the book against the imbalance), cleared once the
/// imbalance leaves the band — so a vault that stays imbalanced re-centers
/// once rather than every tick.
pub fn cold_decision(action: Action, mode: StrategyMode, was_in_band: bool) -> ColdDecision {
    let in_band = matches!(action, Action::Reshape(_) | Action::FreezeSide(_));
    let passive = mode == StrategyMode::Passive;
    ColdDecision {
        action: if passive && in_band {
            Action::Quote
        } else {
            action
        },
        in_band,
        entered: passive && in_band && !was_in_band,
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
            mid: 0.73,
            last_mid: Some(0.73),
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
        t.mid = 0.73 * 1.0009;
        t.candidate = t.mid;
        assert!(!should_set_reference(&t, &passive()));
    }

    #[test]
    fn passive_re_centers_once_price_walks_past_the_inner_levels() {
        // Four inner rungs put the bound at 20 bps, clear of the active 10 bps
        // drift trigger, so this can only pass on the passive bound.
        let strat = StrategyConfig {
            recenter_inner_levels: 4,
            ..passive()
        };
        let mut t = base();
        t.mid = 0.73 * 1.0011;
        assert!(!should_set_reference(&t, &strat), "11 bps is inside 20");
        t.mid = 0.73 * 1.0021;
        assert!(should_set_reference(&t, &strat));
        t.mid = 0.73 * (1.0 - 0.0021);
        assert!(should_set_reference(&t, &strat));
    }

    #[test]
    fn a_skew_shift_alone_never_re_centers_a_passive_book() {
        // The book's own flip fills move inventory and so the skew: the
        // skewed candidate moves 20 bps while the market does not.
        let mut t = base();
        t.skew_bps = 20.0;
        t.candidate = 0.73 * 1.002;
        assert!(!should_set_reference(&t, &passive()));
    }

    #[test]
    fn an_unknown_last_mid_reads_as_walked() {
        let mut t = base();
        t.last_mid = None;
        assert!(should_set_reference(&t, &passive()));
    }

    #[test]
    fn passive_answers_an_imbalance_once_with_a_re_center_not_a_reshape() {
        use crate::model::killswitch::HaltReason;
        use crate::model::ladder::Side;
        let passive = StrategyMode::Passive;
        for action in [Action::Reshape(Side::Bid), Action::FreezeSide(Side::Ask)] {
            // Entering the band: quote through it, and trigger once.
            let d = cold_decision(action, passive, false);
            assert_eq!(d.action, Action::Quote);
            assert!(d.in_band && d.entered);
            // Still in the band, already answered: no second trigger.
            assert!(!cold_decision(action, passive, true).entered);
            // Active reshapes as before and never triggers a re-center.
            let a = cold_decision(action, StrategyMode::Active, false);
            assert_eq!(a.action, action);
            assert!(!a.entered);
        }
        // Halts pass through in both modes, and are out of band.
        let halt = Action::Halt(HaltReason::TvlFloor);
        let d = cold_decision(halt, passive, true);
        assert_eq!(d.action, halt);
        assert!(!d.in_band && !d.entered);
        assert_eq!(
            cold_decision(Action::Quote, passive, false).action,
            Action::Quote
        );
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
        t.last_mid = None;
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
