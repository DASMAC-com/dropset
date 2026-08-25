//! The Pyth FX roster, read from the store rather than compiled in.
//!
//! Every other venue coordinate in this system is *derived*: the canonical
//! `BASE-QUOTE` product id yields OANDA's `AUD_USD`, Twelve Data's `AUD/USD`,
//! Kraken's `AUDUSD`. A Pyth feed is addressed by a 32-byte hex id that no rule
//! produces, so those ids have to be configured somewhere — and where is a
//! deployment question, not a taste one. Post-gate these collectors run on ECS,
//! which supplies environment variables but offers no way to mount a
//! configuration file, so the candidates were a compiled constant (rebuild and
//! redeploy to add a currency), a base64 blob in a variable, or reference data
//! in the store the collector is already required to reach. This is the third:
//! adding a cross is an `INSERT` plus a restart.
//!
//! **Read once, at startup.** There is no live reload, deliberately: the roster
//! changes a few times a year, and a process whose effective roster is fixed at
//! start can state it in one log line — which is worth more than avoiding a
//! restart. `dropset-migrate` owns the table and seeds it
//! (`0005_pyth_fx_feeds.sql`, widened by `0006_pyth_fx_crosses.sql`); nothing
//! here writes to it.

use anyhow::{anyhow, Result};
use dropset_feeds::venues::pyth::PythFeed;
use sqlx::PgPool;

/// One roster row: a currency cross and the Hermes coordinates that price it.
#[derive(Clone, Debug, PartialEq)]
pub struct PythCross {
    /// ISO 4217 code of the **base** leg — the first half of `product_id`.
    ///
    /// Named `currency` because the roster began as one feed per currency
    /// against USD, where the non-USD leg was simply "the fiat leg". Since
    /// `0006_pyth_fx_crosses.sql` the roster carries crosses with no USD leg at
    /// all (`EUR-GBP`, `AUD-JPY`), so one currency names several rows and this
    /// field is the base of a pair rather than an identifier for the feed.
    pub currency: String,
    /// The canonical id readings are stored under, e.g. `EUR-USD`.
    pub product_id: String,
    /// Hermes' 32-byte feed id, lowercase hex.
    pub feed_id: String,
    /// Whether Hermes publishes the cross as `USD/<ccy>` and the reading has to
    /// be reciprocated.
    pub invert: bool,
}

/// Load the enabled roster.
///
/// An **empty** roster is an error rather than an empty run: a collector with
/// nothing to poll sits there looking perfectly healthy while writing nothing,
/// which is the most expensive failure this feed can have. The likely causes
/// are a database that was never migrated (so the seed never ran) or every row
/// disabled at once, and both deserve a startup failure that names them.
pub async fn load(pool: &PgPool) -> Result<Vec<PythCross>> {
    let rows: Vec<(String, String, String, bool)> =
        sqlx::query_as(include_str!("../queries/pyth_fx_feeds_select.sql"))
            .fetch_all(pool)
            .await?;
    if rows.is_empty() {
        return Err(anyhow!(
            "the Pyth FX roster (`pyth_fx_feeds`) has no enabled rows — either \
             the database predates the roster migration, or every cross is \
             disabled; this collector would poll nothing"
        ));
    }
    Ok(rows
        .into_iter()
        .map(|(currency, product_id, feed_id, invert)| PythCross {
            currency,
            product_id,
            feed_id,
            invert,
        })
        .collect())
}

/// Map the roster onto the adapter's feed list.
///
/// **Keyed by canonical product id, not by currency.** The adapter answers
/// under whatever key it was asked with, and the collector's next act is to
/// write those keys into `spot_ticks.product_id` — so asking with the canonical
/// id removes a translation step, and with it the chance of storing a reading
/// under a currency code by mistake. (The maker bot asks by currency because
/// its consumer is an in-memory cache keyed that way, not a table.)
pub fn to_feeds(roster: &[PythCross]) -> Vec<PythFeed> {
    roster
        .iter()
        .map(|cross| {
            if cross.invert {
                PythFeed::inverted(&cross.product_id, &cross.feed_id)
            } else {
                PythFeed::direct(&cross.product_id, &cross.feed_id)
            }
        })
        .collect()
}

/// The canonical product ids the roster covers, for the silence watch and the
/// startup log.
pub fn product_ids(roster: &[PythCross]) -> Vec<String> {
    roster.iter().map(|c| c.product_id.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roster() -> Vec<PythCross> {
        vec![
            PythCross {
                currency: "EUR".into(),
                product_id: "EUR-USD".into(),
                feed_id: "aa".into(),
                invert: false,
            },
            PythCross {
                currency: "ZAR".into(),
                product_id: "ZAR-USD".into(),
                feed_id: "bb".into(),
                invert: true,
            },
        ]
    }

    #[test]
    fn the_adapter_is_asked_under_the_key_the_rows_are_stored_under() {
        // The property that removes a translation step between the poll and the
        // INSERT — and with it the chance of filing a reading under a currency
        // code instead of a product id.
        let feeds = to_feeds(&roster());
        assert_eq!(feeds[0].key, "EUR-USD");
        assert_eq!(feeds[1].key, "ZAR-USD");
    }

    #[test]
    fn an_inverted_cross_is_marked_for_reciprocation() {
        let feeds = to_feeds(&roster());
        assert!(!feeds[0].invert, "EUR is published as EUR/USD");
        assert!(feeds[1].invert, "ZAR is published as USD/ZAR");
    }

    #[test]
    fn product_ids_are_the_roster_in_order() {
        assert_eq!(product_ids(&roster()), vec!["EUR-USD", "ZAR-USD"]);
    }
}
