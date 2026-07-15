//! Inventory / basis / TVL kill switches (§4).
//!
//! Maps a tick's composed reference and live inventory to the single most
//! severe action the bot should take. The spec's hard triggers (basis-band
//! breach, USDC/USD common-mode breach, TVL floor, >80% imbalance) name
//! `FreezeVault`, but that instruction is **admin-only and irreversible**
//! (`programs/dropset` → `freeze_vault`: no unfreeze) — the bot signs only as
//! the leader. So those map to a leader-authorized [`Action::Halt`]: stop
//! quoting (zero the profile, let existing levels expire) and alert for human
//! review, which is what the spec asks for on those rows. A real `FreezeVault`
//! stays a human (admin) decision so a transient feed glitch can't autonomously
//! brick the vault.
//!
//! The fair-value guards — the **basis-band breach** (a peg event) and the
//! portfolio-wide **USDC/USD common-mode breach** (§1 fm1) — are computed by
//! the [`dropset_fair_value`] engine and arrive here as flags on the
//! [`FairValue`]; this module only maps them to actions. The common-mode guard
//! is the most systemic (a USDC depeg hits every market's basis at once), so it
//! is checked first.
//!
//! When the composition is degraded — no live FX anchor outside the weekend
//! regime, or only the last basis / static peg left (see
//! [`dropset_fair_value::Regime`]) — the vault runs *degraded* (§4): the whole
//! switch set tightens by 50% so the bot pulls back sooner on thinner
//! information — the imbalance bounds halve here and the TVL floor halves its
//! permitted drawdown from launch here.
//!
//! The TVL floor is expressed as a *fraction of launch TVL* (the
//! `tvl_floor_frac` config knob) against the launch TVL the caller reads from
//! the vault at startup — not the spec's literal $80/$100. The demo seeds
//! ~$100 top-of-book across seven markets whose tokens span ~$1.14 down to
//! ~$0.00006, so an absolute-dollar floor would be wrong in six of them; a
//! drawdown fraction is correct in all of them at once.

use crate::config::KillSwitchConfig;
use crate::model::inventory::Inventory;
use crate::model::ladder::Side;
use dropset_fair_value::FairValue;

/// Why the bot halted — carried into the alert log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HaltReason {
    /// USDC/USD left its common-mode band — a correlated, portfolio-wide depeg
    /// that moves every market's basis at once (§1 fm1, §4). The most systemic
    /// halt, so it is evaluated before the per-market peg event.
    UsdcCommonMode,
    /// The smoothed basis left its per-market sane band — a peg event (§1, §4).
    BasisBreach,
    /// Vault TVL fell to the floor.
    TvlFloor,
    /// Per-side imbalance past the critical bound.
    ImbalanceCritical,
}

/// What the bot should do this tick, most-severe-first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Quote the full ladder normally.
    Quote,
    /// Imbalance past the reshape bound (§4 row 1): shrink the *accumulating*
    /// side (carried here) so the untouched heavy side dominates the book and
    /// leans into offloading the heavy leg, on top of the reference skew every
    /// tick already applies. A milder step than [`Action::FreezeSide`].
    Reshape(Side),
    /// Freeze the side that *accumulates* the heavy leg — only the rebuild
    /// side keeps quoting (imbalance past the freeze-side bound).
    FreezeSide(Side),
    /// Stop quoting and alert for human review — the hard triggers.
    Halt(HaltReason),
}

/// Evaluate the kill switches for this tick. `launch_tvl` is the vault's TVL at
/// startup (the caller reads it from the first snapshot), against which the
/// drawdown floor is measured. `degraded` (from [`FairValue::degraded`] — no
/// live FX anchor outside the weekend regime, or a peg-rate fallback tier)
/// tightens the whole switch set by 50% (§4): the imbalance thresholds halve,
/// and the TVL floor halves its permitted drawdown from launch. The basis and
/// common-mode bands are not scaled here — they are absolute peg events the
/// engine already evaluated against its (survey-set) bands.
pub fn evaluate(
    fair: &FairValue,
    inv: &Inventory,
    kill: &KillSwitchConfig,
    degraded: bool,
    launch_tvl: f64,
) -> Action {
    let scale = if degraded { 0.5 } else { 1.0 };

    // Hard halts first — these want a human, not a self-healing reshape. The
    // portfolio-wide common-mode guard precedes the per-market basis event: a
    // USDC depeg is the more systemic signal (§1 fm1).
    if fair.usdc_breach {
        return Action::Halt(HaltReason::UsdcCommonMode);
    }
    if fair.basis_breach {
        return Action::Halt(HaltReason::BasisBreach);
    }
    // The floor is launch TVL less the permitted drawdown. Degraded halves the
    // permitted drawdown, so the floor rises toward launch TVL (e.g. a 0.8
    // floor — 20% drawdown — tightens to a 10% drawdown, 0.9 of launch).
    let permitted_drawdown = (1.0 - kill.tvl_floor_frac) * scale;
    let tvl_floor = launch_tvl * (1.0 - permitted_drawdown);
    if inv.total_usd() <= tvl_floor {
        return Action::Halt(HaltReason::TvlFloor);
    }
    let imbalance = inv.imbalance_pct();
    if imbalance > kill.imbalance_halt_pct * scale {
        return Action::Halt(HaltReason::ImbalanceCritical);
    }

    // The order side that *adds* to the heavy leg is the one to pull: when
    // base-heavy we must stop buying base, so freeze the bids and let the
    // asks rebuild the quote leg.
    let accumulating = if inv.base_heavy() {
        Side::Bid
    } else {
        Side::Ask
    };
    if imbalance > kill.imbalance_freeze_side_pct * scale {
        return Action::FreezeSide(accumulating);
    }
    if imbalance > kill.imbalance_reshape_pct * scale {
        return Action::Reshape(accumulating);
    }
    Action::Quote
}

#[cfg(test)]
mod tests {
    use super::*;
    use dropset_fair_value::{Anchor, Health, Regime};

    fn ok_fair() -> FairValue {
        FairValue {
            fair: Some(1.14),
            anchor: Anchor::Fx,
            regime: Regime::Normal,
            basis: Some(1.0),
            health: Health::Ok,
            uncertain: false,
            basis_breach: false,
            usdc_breach: false,
        }
    }

    fn inv(base: f64, quote: f64) -> Inventory {
        Inventory {
            base_value_usd: base,
            quote_value_usd: quote,
        }
    }

    fn kill() -> KillSwitchConfig {
        KillSwitchConfig::default()
    }

    /// The reference-scale launch TVL the imbalance/quoting tests run against
    /// (their inventories sum to ~$100, matching the spec's reference vault).
    const LAUNCH: f64 = 100.0;

    #[test]
    fn balanced_vault_quotes() {
        assert_eq!(
            evaluate(&ok_fair(), &inv(50.0, 50.0), &kill(), false, LAUNCH),
            Action::Quote
        );
    }

    #[test]
    fn moderate_imbalance_reshapes() {
        // 65/35 → 30% (not > 30), 66/34 → 32% → reshape. Base-heavy, so the
        // accumulating (bid) side is the one shrunk.
        assert_eq!(
            evaluate(&ok_fair(), &inv(66.0, 34.0), &kill(), false, LAUNCH),
            Action::Reshape(Side::Bid)
        );
    }

    #[test]
    fn quote_heavy_reshape_shrinks_the_ask_side() {
        // 34/66 → 32% imbalance, quote-heavy → the accumulating (ask) side is
        // the one shrunk, mirroring the base-heavy bid case.
        assert_eq!(
            evaluate(&ok_fair(), &inv(34.0, 66.0), &kill(), false, LAUNCH),
            Action::Reshape(Side::Ask)
        );
    }

    #[test]
    fn heavy_imbalance_freezes_the_accumulating_side() {
        // 78/22 → 56% imbalance, base-heavy → freeze the bid side.
        assert_eq!(
            evaluate(&ok_fair(), &inv(78.0, 22.0), &kill(), false, LAUNCH),
            Action::FreezeSide(Side::Bid)
        );
    }

    #[test]
    fn critical_imbalance_halts() {
        // 95/5 → 90% imbalance → halt for review.
        assert_eq!(
            evaluate(&ok_fair(), &inv(95.0, 5.0), &kill(), false, LAUNCH),
            Action::Halt(HaltReason::ImbalanceCritical)
        );
    }

    #[test]
    fn basis_breach_halts_over_everything() {
        let mut fair = ok_fair();
        fair.basis_breach = true;
        // Even a balanced vault halts on a basis-band breach.
        assert_eq!(
            evaluate(&fair, &inv(50.0, 50.0), &kill(), false, LAUNCH),
            Action::Halt(HaltReason::BasisBreach)
        );
    }

    #[test]
    fn usdc_common_mode_halts_before_the_basis_event() {
        // A USDC depeg is the more systemic signal, so it wins even when the
        // per-market basis has also breached.
        let mut fair = ok_fair();
        fair.usdc_breach = true;
        fair.basis_breach = true;
        assert_eq!(
            evaluate(&fair, &inv(50.0, 50.0), &kill(), false, LAUNCH),
            Action::Halt(HaltReason::UsdcCommonMode)
        );
    }

    #[test]
    fn tvl_floor_halts() {
        // $79 against a $100 launch is below the 0.8 (20% drawdown) floor.
        assert_eq!(
            evaluate(&ok_fair(), &inv(40.0, 39.0), &kill(), false, LAUNCH),
            Action::Halt(HaltReason::TvlFloor)
        );
    }

    #[test]
    fn tvl_floor_scales_with_launch_tvl() {
        // The floor is a drawdown fraction, so a $1M vault halts at $800k and
        // quotes just above it — the same 20% rule the $100 vault sees, with no
        // per-market dollar tuning.
        let above = inv(405_000.0, 405_000.0); // $810k
        let below = inv(399_000.0, 399_000.0); // $798k
        assert_eq!(
            evaluate(&ok_fair(), &above, &kill(), false, 1_000_000.0),
            Action::Quote
        );
        assert_eq!(
            evaluate(&ok_fair(), &below, &kill(), false, 1_000_000.0),
            Action::Halt(HaltReason::TvlFloor)
        );
    }

    #[test]
    fn tvl_floor_takes_precedence_over_critical_imbalance() {
        // A drained vault is the more urgent post-mortem signal, so the floor
        // is checked before the imbalance ladder: a vault both below floor and
        // critically imbalanced halts on TvlFloor, not ImbalanceCritical.
        let drained = inv(70.0, 5.0); // $75 (≤ $80 floor), ~87% imbalance
        assert_eq!(
            evaluate(&ok_fair(), &drained, &kill(), false, LAUNCH),
            Action::Halt(HaltReason::TvlFloor)
        );
    }

    #[test]
    fn degraded_tightens_thresholds() {
        // 60/40 → 20% imbalance: Quote when healthy, Reshape when degraded
        // (reshape bound tightens 30% → 15%). Base-heavy → shrink the bid side.
        assert_eq!(
            evaluate(&ok_fair(), &inv(60.0, 40.0), &kill(), false, LAUNCH),
            Action::Quote
        );
        assert_eq!(
            evaluate(&ok_fair(), &inv(60.0, 40.0), &kill(), true, LAUNCH),
            Action::Reshape(Side::Bid)
        );
    }

    #[test]
    fn degraded_raises_the_tvl_floor() {
        // 0.8 floor on a $100 launch is $80 healthy → $90 degraded (the 20%
        // permitted drawdown halves to 10%). A vault at $85 is above the
        // healthy floor but halts once degraded.
        assert_eq!(
            evaluate(&ok_fair(), &inv(43.0, 42.0), &kill(), false, LAUNCH),
            Action::Quote
        );
        assert_eq!(
            evaluate(&ok_fair(), &inv(43.0, 42.0), &kill(), true, LAUNCH),
            Action::Halt(HaltReason::TvlFloor)
        );
    }
}
