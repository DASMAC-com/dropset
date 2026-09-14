//! Publishing the fair-price estimator's output into `fair_price` (migration
//! 0011) — the seam between the estimator process and everything downstream of
//! it.
//!
//! This module issues **no DDL**, for the reason [`crate::store`] gives: the
//! table is defined in `dropset-db-schema` and created by `dropset-migrate`,
//! the single schema owner.
//!
//! **What lives here is the serialization of one composition, and nothing
//! else.** This module takes a finished [`FairValue`] and writes it down; it
//! neither reads the store nor composes anything.
//!
//! Assembling the candidate sets the engine composes *from* is
//! [`crate::fx_store`], and driving the two is the `market-data-estimator`
//! binary — both in this crate but not in this module. The boundary that
//! matters is the one
//! this module keeps, between serializing a composition and producing it; an
//! earlier version of this note drew it at the crate edge instead, which stopped
//! being true when the reader joined the writers it reads behind.
//!
//! The engine's enums reach the database as TEXT rather than as an integer
//! discriminant, matching every enum-ish column in 0003 and 0007. The reason is
//! the same: a variant added later must not turn a telemetry write into a
//! constraint violation that fails the very write reporting the problem. The
//! mappings below are therefore **stable wire names**, not `Debug` output —
//! renaming a Rust variant must not silently re-label a column that dashboards
//! and analytics already filter on.

use dropset_fair_value::{Anchor, Degrade, FairValue, Health, LegStaleness, Regime};

/// Why a publish failed, split by whether **retrying the same row could ever
/// succeed**.
///
/// The split exists because the estimator's fail-closed halt has to name a
/// cause, and the two classes call for opposite handling. A dropped connection
/// is worth retrying on the next tick; a row the database refuses on its own
/// terms is not, and retrying one forever is a stall wearing a transient's
/// clothes — the estimator would look busy, publish nothing, and report a
/// retry count rising instead of a defect.
///
/// The classification is about **where the row got to**, not about severity.
/// Both classes are failures a caller must act on; neither is benign.
#[derive(Debug)]
pub enum PublishError {
    /// A database judged the row and refused it: a CHECK or constraint
    /// violation, a type or column mismatch, a missing table. The same row
    /// will be refused again by the same schema, so a retry is a spin.
    ///
    /// This is a **defect**, in the composition or in the deploy — a product id
    /// that fails 0011's canonical-shape CHECK, an inverted staleness pair
    /// against 0014's ordering CHECK, or a binary running against a schema it
    /// was not built for.
    Permanent(anyhow::Error),
    /// The row never reached a database that could judge it, or reached one that
    /// was shutting down or overloaded: a socket error, a TLS failure, a pool
    /// timeout, an admin shutdown, a serialization failure.
    ///
    /// Says nothing about whether the row is acceptable — only that the question
    /// was not answered. Retrying is sound.
    Transient(anyhow::Error),
}

impl PublishError {
    /// The wire-stable word for the class, for a halt reason or a log field.
    /// Stable like the column vocabularies above: an operator alert and a
    /// dashboard filter key off it.
    pub fn class(&self) -> &'static str {
        match self {
            Self::Permanent(_) => "permanent",
            Self::Transient(_) => "transient",
        }
    }

    /// Whether retrying this exact row could succeed. The whole point of the
    /// type: a caller branches on this rather than on the message text.
    pub fn retryable(&self) -> bool {
        matches!(self, Self::Transient(_))
    }
}

impl std::fmt::Display for PublishError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Permanent(e) => write!(f, "permanent publish failure: {e}"),
            Self::Transient(e) => write!(f, "transient publish failure: {e}"),
        }
    }
}

impl std::error::Error for PublishError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Permanent(e) | Self::Transient(e) => Some(e.as_ref()),
        }
    }
}

/// Sort one `sqlx` failure into the two classes.
///
/// **Unclassified failures land in `Permanent`**, which is the fail-closed
/// direction and is deliberate: the cost of misfiling a genuine transient is
/// one halt an operator can see and clear, while the cost of misfiling a
/// permanent one is the silent infinite retry this whole split exists to
/// prevent. So the transient set is an **allowlist** of failures known not to
/// be about the row, and everything else — including a variant `sqlx` adds
/// later — is permanent until someone establishes otherwise.
fn classify(err: sqlx::Error) -> PublishError {
    // SQLSTATE classes that describe the server or the link rather than the
    // row. `08` connection exception, `40` transaction rollback (serialization
    // failure, deadlock — the retry is the documented remedy), `53`
    // insufficient resources, `57` operator intervention (`57P01` admin
    // shutdown is what a restarting Postgres answers), `58` external system
    // error. Everything else a database reports is about the statement or the
    // row: `22` data exception, `23` integrity constraint violation — 0011's
    // and 0014's CHECKs — `42` syntax or access rule violation, `3D`/`3F`
    // invalid catalog or schema name.
    const TRANSIENT_SQLSTATE_CLASSES: [&str; 5] = ["08", "40", "53", "57", "58"];

    if let sqlx::Error::Database(ref db) = err {
        // A driver that reports no SQLSTATE cannot be placed by class, so it
        // takes the fail-closed default with everything else unclassified.
        let transient = db
            .code()
            .is_some_and(|code| TRANSIENT_SQLSTATE_CLASSES.contains(&&code[..2]));
        return if transient {
            PublishError::Transient(err.into())
        } else {
            PublishError::Permanent(err.into())
        };
    }

    match err {
        // The row did not reach a database, or the link died mid-answer.
        sqlx::Error::Io(_)
        | sqlx::Error::Tls(_)
        | sqlx::Error::Protocol(_)
        | sqlx::Error::PoolTimedOut
        | sqlx::Error::PoolClosed
        | sqlx::Error::WorkerCrashed => PublishError::Transient(err.into()),
        // Everything else: a misconfiguration, a decode fault, a schema
        // mismatch, or a variant that did not exist when this was written.
        _ => PublishError::Permanent(err.into()),
    }
}

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
///
/// The non-degraded regimes are spelled out rather than caught by a `_` arm, or
/// by the `let ... else` this used to use, so that a regime added later is a
/// compile error here instead of silently acquiring a NULL `degrade`. That
/// property is what the rest of this module's mappings get from being
/// wildcard-free, and it is worth the extra arm to keep it uniform.
pub fn degrade_name(regime: Regime) -> Option<&'static str> {
    match regime {
        Regime::Degraded(degrade) => Some(match degrade {
            Degrade::FxStale => "fx_stale",
            Degrade::LegDispersed => "leg_dispersed",
            Degrade::FxInvalid => "fx_invalid",
            Degrade::NoBasisLeg => "no_basis_leg",
            Degrade::BasisUnusable => "basis_unusable",
            Degrade::StaticPeg => "static_peg",
        }),
        Regime::Normal
        | Regime::CryptoOnly
        | Regime::FxPinned
        | Regime::Uncorroborated
        | Regime::Paused => None,
    }
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
/// Takes any Postgres executor rather than a pool specifically, so a caller
/// publishing a whole tick can pass one transaction and have the tick land
/// atomically. With a bare pool each pair is its own auto-commit round trip, so
/// an interrupted tick leaves some pairs written at `ts` and others absent —
/// indistinguishable, to a reader, from pairs the estimator skipped. Widening
/// this later would be a breaking change to a public signature, and it costs
/// nothing now.
///
/// Returns whether the row was new. **A `false` is an alarm, not a benign
/// no-op**: the primary key already held this pair at this stamp, which means
/// something else published for it. A caller must log or count it — see the
/// query's note on why the row is left alone rather than overwritten.
///
/// `stale` is the staleness bound pair the composition was **actually resolved
/// at**, and it is a parameter rather than something read from a config here on
/// purpose: 0014 records it per row, and a bound this function looked up itself
/// could disagree with the one the engine used. Take it from
/// [`FairValueEngine::leg_bounds`], whose own documentation makes the same
/// point — every engine owns its config, so a second copy read anywhere else is
/// a diagnostic that may disagree with the thing it is diagnosing. Passing it
/// makes per-leg staleness an explicit requirement of this path rather than an
/// implicit property of `compose`.
///
/// [`FairValueEngine::leg_bounds`]: dropset_fair_value::FairValueEngine::leg_bounds
///
/// # Errors
///
/// Returns [`PublishError`], which splits a row the database refused from a row
/// that never reached one. **Branch on [`PublishError::retryable`] before
/// retrying** — a permanent failure retried on every tick is a silent stall.
pub async fn publish(
    executor: impl sqlx::PgExecutor<'_>,
    ts: i64,
    product_id: &str,
    fv: &FairValue,
    stale: LegStaleness,
) -> Result<bool, PublishError> {
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
        // The bounds this composition was judged at (0014). Seconds, matching
        // the columns and `basis_age_secs` above.
        .bind(stale.tape.as_secs() as i64)
        .bind(stale.reference.as_secs() as i64)
        .execute(executor)
        .await
        .map_err(classify)?;
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
    ///
    /// **Every** name this module owns is pinned, not a sample. A partial list
    /// is the failure this test exists to prevent, wearing the shape of the
    /// test that prevents it: whatever it leaves out can still be renamed with
    /// the suite green, which is precisely the silent re-labelling above.
    #[test]
    fn the_wire_names_are_pinned() {
        assert_eq!(anchor_name(Anchor::Fx), "fx");
        assert_eq!(anchor_name(Anchor::CryptoReference), "crypto_reference");
        assert_eq!(anchor_name(Anchor::Static), "static");
        assert_eq!(anchor_name(Anchor::None), "none");

        assert_eq!(regime_name(Regime::Normal), "normal");
        assert_eq!(regime_name(Regime::CryptoOnly), "crypto_only");
        assert_eq!(regime_name(Regime::FxPinned), "fx_pinned");
        assert_eq!(regime_name(Regime::Uncorroborated), "uncorroborated");
        assert_eq!(regime_name(Regime::Paused), "paused");
        assert_eq!(
            regime_name(Regime::Degraded(Degrade::StaticPeg)),
            "degraded"
        );

        for (degrade, expected) in [
            (Degrade::FxStale, "fx_stale"),
            (Degrade::LegDispersed, "leg_dispersed"),
            (Degrade::FxInvalid, "fx_invalid"),
            (Degrade::NoBasisLeg, "no_basis_leg"),
            (Degrade::BasisUnusable, "basis_unusable"),
            (Degrade::StaticPeg, "static_peg"),
        ] {
            assert_eq!(degrade_name(Regime::Degraded(degrade)), Some(expected));
        }

        assert_eq!(health_name(Health::Ok), "ok");
        assert_eq!(health_name(Health::Unverified), "unverified");
        assert_eq!(health_name(Health::Degraded), "degraded");
        assert_eq!(health_name(Health::Pause), "pause");
    }
}
