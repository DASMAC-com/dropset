//! Inventory / peg / TVL kill switches (§4).
//!
//! Maps a tick's composed reference and live inventory to the single most
//! severe action the bot should take. The spec's hard triggers (peg breach,
//! TVL floor, >80% imbalance) name `FreezeVault`, but that instruction is
//! **admin-only and irreversible** (`programs/dropset` → `freeze_vault`: no
//! unfreeze) — the bot signs only as the leader. So those map to a
//! leader-authorized [`Action::Halt`]: stop quoting (zero the profile, let
//! existing levels expire) and alert for human review, which is what the spec
//! asks for on those rows. A real `FreezeVault` stays a human (admin) decision
//! so a transient feed glitch can't autonomously brick the vault.
//!
//! When any single feed is stale the vault runs *degraded* (§4 row 5): the
//! whole switch set tightens by 50% so the bot pulls back sooner on thinner
//! information — the imbalance bounds halve here, the TVL floor halves its
//! permitted drawdown from launch here, and the peg band halves its margin
//! upstream in [`super::fair_mid`] (where `peg_breach` is composed).

use crate::config::KillSwitchConfig;
use crate::model::fair_mid::FairMid;
use crate::model::inventory::Inventory;
use crate::model::ladder::Side;

/// Why the bot halted — carried into the alert log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HaltReason {
    /// CADC peg left `[peg_low, peg_high]` vs FX spot.
    PegBreach,
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

/// Evaluate the kill switches for this tick. `degraded` (any single stale
/// feed, per [`super::fair_mid::Health::Degraded`]) tightens the whole switch
/// set by 50% (§4 row 5): the imbalance thresholds halve, and the TVL floor
/// halves its permitted drawdown from launch (the peg band is tightened
/// upstream in `compose`, since `peg_breach` is composed there).
pub fn evaluate(
    fair: &FairMid,
    inv: &Inventory,
    kill: &KillSwitchConfig,
    degraded: bool,
) -> Action {
    let scale = if degraded { 0.5 } else { 1.0 };

    // Hard halts first — these want a human, not a self-healing reshape.
    if fair.peg_breach {
        return Action::Halt(HaltReason::PegBreach);
    }
    // Degraded halves the permitted drawdown from launch, so the floor rises
    // toward launch TVL (e.g. 100 → 80 floor becomes a 90 floor).
    let tvl_floor = kill.tvl_halt_usd + (kill.launch_tvl_usd - kill.tvl_halt_usd) * (1.0 - scale);
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
    use crate::model::fair_mid::Health;

    fn ok_fair() -> FairMid {
        FairMid {
            mid: Some(0.73),
            peg: Some(1.0),
            health: Health::Ok,
            peg_breach: false,
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

    #[test]
    fn balanced_vault_quotes() {
        assert_eq!(
            evaluate(&ok_fair(), &inv(50.0, 50.0), &kill(), false),
            Action::Quote
        );
    }

    #[test]
    fn moderate_imbalance_reshapes() {
        // 65/35 → 30% (not > 30), 66/34 → 32% → reshape. Base-heavy, so the
        // accumulating (bid) side is the one shrunk.
        assert_eq!(
            evaluate(&ok_fair(), &inv(66.0, 34.0), &kill(), false),
            Action::Reshape(Side::Bid)
        );
    }

    #[test]
    fn heavy_imbalance_freezes_the_accumulating_side() {
        // 78/22 → 56% imbalance, base-heavy → freeze the bid side.
        assert_eq!(
            evaluate(&ok_fair(), &inv(78.0, 22.0), &kill(), false),
            Action::FreezeSide(Side::Bid)
        );
    }

    #[test]
    fn critical_imbalance_halts() {
        // 95/5 → 90% imbalance → halt for review.
        assert_eq!(
            evaluate(&ok_fair(), &inv(95.0, 5.0), &kill(), false),
            Action::Halt(HaltReason::ImbalanceCritical)
        );
    }

    #[test]
    fn peg_breach_halts_over_everything() {
        let mut fair = ok_fair();
        fair.peg_breach = true;
        // Even a balanced vault halts on a peg breach.
        assert_eq!(
            evaluate(&fair, &inv(50.0, 50.0), &kill(), false),
            Action::Halt(HaltReason::PegBreach)
        );
    }

    #[test]
    fn tvl_floor_halts() {
        assert_eq!(
            evaluate(&ok_fair(), &inv(40.0, 39.0), &kill(), false),
            Action::Halt(HaltReason::TvlFloor)
        );
    }

    #[test]
    fn degraded_tightens_thresholds() {
        // 60/40 → 20% imbalance: Quote when healthy, Reshape when degraded
        // (reshape bound tightens 30% → 15%). Base-heavy → shrink the bid side.
        assert_eq!(
            evaluate(&ok_fair(), &inv(60.0, 40.0), &kill(), false),
            Action::Quote
        );
        assert_eq!(
            evaluate(&ok_fair(), &inv(60.0, 40.0), &kill(), true),
            Action::Reshape(Side::Bid)
        );
    }

    #[test]
    fn degraded_raises_the_tvl_floor() {
        // Default floor 80, launch 100 → degraded floor 90. A vault at 85 is
        // above the healthy floor but halts once degraded.
        assert_eq!(
            evaluate(&ok_fair(), &inv(43.0, 42.0), &kill(), false),
            Action::Quote
        );
        assert_eq!(
            evaluate(&ok_fair(), &inv(43.0, 42.0), &kill(), true),
            Action::Halt(HaltReason::TvlFloor)
        );
    }
}
