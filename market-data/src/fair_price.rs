//! Publishing the fair-price estimator's output into `fair_price` (migration
//! 0011) — the seam between the estimator process and everything downstream of
//! it.
//!
//! This module issues **no DDL**, for the reason [`crate::store`] gives: the
//! table is defined in `dropset-db-schema` and created by `dropset-migrate`,
//! the single schema owner.
//!
//! **What lives here is the serialization of one composition, and nothing
//! else.** Assembling the candidate sets the engine composes *from* is
//! deliberately not in this crate — that is the store-read path, and it belongs
//! to whichever consumer owns the roster. This module takes a finished
//! [`FairValue`] and writes it down.
//!
//! The engine's enums reach the database as TEXT rather than as an integer
//! discriminant, matching every enum-ish column in 0003 and 0007. The reason is
//! the same: a variant added later must not turn a telemetry write into a
//! constraint violation that fails the very write reporting the problem. The
//! mappings below are therefore **stable wire names**, not `Debug` output —
//! renaming a Rust variant must not silently re-label a column that dashboards
//! and analytics already filter on.

use dropset_fair_value::{Anchor, Degrade, FairValue, Health, Regime};
use sqlx::PgPool;

/// The stable wire name for the anchor leg.
pub fn anchor_name(anchor: Anchor) -> &'static str {
    match anchor {
        Anchor::Fx => "fx",
        Anchor::CryptoReference => "crypto_reference",
        Anchor::Static => "static",
        Anchor::None => "none",
    }
}

/// The stable wire name for the composition regime.
///
/// [`Regime::Degraded`] carries a payload that this deliberately drops: the
/// column pair is `regime` plus `degrade`, so a reader can filter on the regime
/// without parsing it. [`degrade_name`] recovers the payload.
pub fn regime_name(regime: Regime) -> &'static str {
    match regime {
        Regime::Normal => "normal",
        Regime::CryptoOnly => "crypto_only",
        Regime::FxPinned => "fx_pinned",
        Regime::Uncorroborated => "uncorroborated",
        Regime::Degraded(_) => "degraded",
        Regime::Paused => "paused",
    }
}

/// The stable wire name for *which* degrade, or `None` in every other regime.
///
/// `None` here means "not degraded" — never "degraded for an unknown reason".
/// The column is NULL in exactly the regimes that are not [`Regime::Degraded`].
pub fn degrade_name(regime: Regime) -> Option<&'static str> {
    let Regime::Degraded(degrade) = regime else {
        return None;
    };
    Some(match degrade {
        Degrade::FxStale => "fx_stale",
        Degrade::LegDispersed => "leg_dispersed",
        Degrade::FxInvalid => "fx_invalid",
        Degrade::NoBasisLeg => "no_basis_leg",
        Degrade::BasisUnusable => "basis_unusable",
        Degrade::StaticPeg => "static_peg",
    })
}

/// The stable wire name for the kill-switch health gate.
pub fn health_name(health: Health) -> &'static str {
    match health {
        Health::Ok => "ok",
        Health::Unverified => "unverified",
        Health::Degraded => "degraded",
        Health::Pause => "pause",
    }
}

/// Publish one composition for one pair.
///
/// `ts` is the **estimator's** tick stamp in Unix seconds, not the write time:
/// a consumer ages the value from it, so taking it from the clock here would
/// report a value composed from stale feeds as freshly published. It is passed
/// in for that reason rather than read from the database or the system clock.
///
/// Returns whether the row was new. A `false` means the primary key already held
/// this pair at this stamp — see the query's note on why that is left visible
/// rather than overwritten.
pub async fn publish(
    pool: &PgPool,
    ts: i64,
    product_id: &str,
    fv: &FairValue,
) -> anyhow::Result<bool> {
    let res = sqlx::query(include_str!("../queries/fair_price_insert.sql"))
        .bind(ts)
        .bind(product_id)
        .bind(fv.fair)
        .bind(anchor_name(fv.anchor))
        .bind(regime_name(fv.regime))
        .bind(degrade_name(fv.regime))
        .bind(health_name(fv.health))
        .bind(fv.basis)
        // Seconds, to match the column's unit. Truncating is right rather than
        // lossy: the age is a staleness signal read against multi-second
        // freshness bounds, and sub-second precision would imply the estimator
        // ticks faster than it does.
        .bind(fv.basis_age.map(|d| d.as_secs() as i64))
        .bind(fv.basis_outlier)
        .bind(fv.uncertain)
        .bind(fv.basis_breach)
        .bind(fv.usdc_breach)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every degrade variant must map to a distinct, non-empty name. A
    /// collision would merge two operator-facing causes into one label, and the
    /// wildcard-free `match` above is what keeps a new variant from defaulting
    /// into an existing one.
    #[test]
    fn every_degrade_maps_to_a_distinct_name() {
        let all = [
            Degrade::FxStale,
            Degrade::LegDispersed,
            Degrade::FxInvalid,
            Degrade::NoBasisLeg,
            Degrade::BasisUnusable,
            Degrade::StaticPeg,
        ];
        let mut names: Vec<&str> = all
            .iter()
            .map(|d| degrade_name(Regime::Degraded(*d)).expect("a degraded regime names its cause"))
            .collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count, "two degrades share a wire name");
        assert!(!names.iter().any(|n| n.is_empty()));
    }

    /// The `degrade` column is NULL in exactly the non-degraded regimes. This is
    /// the invariant migration 0011 states about the column, asserted at the one
    /// place that can violate it.
    #[test]
    fn only_a_degraded_regime_names_a_degrade() {
        let not_degraded = [
            Regime::Normal,
            Regime::CryptoOnly,
            Regime::FxPinned,
            Regime::Uncorroborated,
            Regime::Paused,
        ];
        for regime in not_degraded {
            assert_eq!(
                degrade_name(regime),
                None,
                "{regime:?} is not a degrade and must write NULL"
            );
        }
        assert_eq!(
            degrade_name(Regime::Degraded(Degrade::FxStale)),
            Some("fx_stale")
        );
    }

    /// Health is a total function of the regime in the engine, and this asserts
    /// the two agree *through the wire names* — the mapping this module owns is
    /// the one that could drift, since the engine's own derivation cannot.
    #[test]
    fn health_names_follow_the_regime() {
        let cases = [
            (Regime::Normal, "ok"),
            (Regime::CryptoOnly, "ok"),
            (Regime::FxPinned, "unverified"),
            (Regime::Uncorroborated, "unverified"),
            (Regime::Degraded(Degrade::FxStale), "degraded"),
            (Regime::Paused, "pause"),
        ];
        for (regime, expected) in cases {
            assert_eq!(
                health_name(regime.health()),
                expected,
                "{regime:?} should gate as {expected}"
            );
        }
    }

    /// Wire names are a schema contract, so they are pinned here rather than
    /// left to `Debug`. A rename that reaches the column silently re-labels a
    /// series every dashboard and analytics query already filters on.
    #[test]
    fn the_wire_names_are_pinned() {
        assert_eq!(anchor_name(Anchor::CryptoReference), "crypto_reference");
        assert_eq!(anchor_name(Anchor::Fx), "fx");
        assert_eq!(anchor_name(Anchor::Static), "static");
        assert_eq!(anchor_name(Anchor::None), "none");
        assert_eq!(regime_name(Regime::CryptoOnly), "crypto_only");
        assert_eq!(
            regime_name(Regime::Degraded(Degrade::StaticPeg)),
            "degraded"
        );
        assert_eq!(health_name(Health::Unverified), "unverified");
    }
}
