//! The CoinGecko `/simple/price` adapter — batched token/USD quotes.
//!
//! One request prices every id the source was built with, which is what lets a
//! whole roster ride a keyless tier's IP budget (docs/data-feeds.md §10). For
//! the maker this is the **crypto basis leg**, and the anchor in the
//! crypto-only (weekend / localnet) regime; it is never the FX anchor — laggy
//! and reflexive against the venues it prices (market-making.md §1).

use super::Quotes;
use crate::{Batch, HttpClient, Source};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::time::Duration;

/// The floor between two requests on this venue — the strictest in the roster.
///
/// This adapter sends no key, so it draws on CoinGecko's **keyless public**
/// tier, documented as a *dynamic* **5–15 calls per minute** by IP rather than
/// a fixed rate. (The keyed Demo tier is 100/min against 10k calls/month; it is
/// a different budget and this adapter is not on it.)
///
/// The floor is sized to the **low end** of that band, since which end applies
/// is not ours to know. Against the shared client's 250 ms default — ~240 a
/// minute — that default would be 16–48× over the tier the first time anything
/// issued back-to-back requests here.
///
/// **15 s (4/min) rather than the 12 s that would yield exactly 5.** Sitting on
/// the cap is the wrong place for the same reasons as Pyth's floor, and one
/// more specific to this venue: the band is *dynamic*, so the low end is not a
/// floor the venue promises to hold — it is the lowest value observed. A margin
/// below it is the only thing that survives the band moving.
///
/// It does not bind today: one request prices the whole roster and the maker
/// polls every 60 s. It encodes the constraint at the transport, which is where
/// a future pager would otherwise inherit the wrong number.
const MIN_REQUEST_INTERVAL: Duration = Duration::from_secs(15);

/// A poll [`Source`] over CoinGecko's batched simple-price endpoint, keyed by
/// CoinGecko's own id slugs (`euro-coin`, `usd-coin`, …).
pub struct CoinGeckoSource {
    http: HttpClient,
    ids: Vec<String>,
}

impl CoinGeckoSource {
    /// Build the source over `base_url`, batching `ids` in every poll.
    pub fn new(base_url: &str, ids: Vec<String>) -> Result<Self> {
        Ok(Self {
            http: HttpClient::new(base_url)?.with_min_interval(MIN_REQUEST_INTERVAL),
            ids,
        })
    }

    /// Fetch every id this source was built with, in one request. Ids CoinGecko
    /// does not list are **omitted** rather than erroring, per the batched-poll
    /// convention in [`venues`](super).
    pub async fn poll(&self) -> Result<Quotes<String>> {
        let csv = self.ids.join(",");
        let body: Value = self
            .http
            .get_json("/simple/price", &[("ids", &csv), ("vs_currencies", "usd")])
            .await?;
        let ids: Vec<&str> = self.ids.iter().map(String::as_str).collect();
        Ok(parse_coingecko(&body, &ids))
    }
}

/// This source's [`Source::name`] — see [`crate::venues::pyth::FEED_NAME`] for
/// why the name is a constant rather than a literal at each use.
pub const FEED_NAME: &str = "coingecko";

#[async_trait]
impl Source for CoinGeckoSource {
    type Record = Quotes<String>;
    fn name(&self) -> &str {
        FEED_NAME
    }
    async fn next(&mut self) -> Result<Batch<Self::Record>> {
        Ok(Batch::new(vec![self.poll().await?]))
    }
}

/// Decode CoinGecko's `{"<id>":{"usd":<n>}}` batched simple-price response into
/// `id → usd`, keeping only positive finite readings.
pub fn parse_coingecko(body: &Value, ids: &[&str]) -> Quotes<String> {
    let mut out = Quotes::new();
    for &id in ids {
        if let Some(v) = body
            .get(id)
            .and_then(|v| v.get("usd"))
            .and_then(Value::as_f64)
        {
            if v.is_finite() && v > 0.0 {
                out.insert(id.to_string(), v);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::venues::requests_per_window;
    use serde_json::json;

    #[test]
    fn the_floor_stays_inside_the_low_end_of_the_keyless_band() {
        // The keyless tier is a dynamic 5–15 calls/minute, and which end applies
        // is not observable — so the floor must satisfy the *low* end, and
        // strictly, since a dynamic band's low end can itself move.
        let per_minute = requests_per_window(MIN_REQUEST_INTERVAL, Duration::from_secs(60));
        assert!(
            per_minute < 5.0,
            "{per_minute} requests/minute does not sit strictly inside the \
             5/minute low end of CoinGecko's keyless band"
        );
    }

    #[test]
    fn parses_coingecko_batch() {
        let body = json!({
            "euro-coin": { "usd": 1.141 },
            "idrx": { "usd": 0.000056 },
            "real-mxn": { "usd": 0.0573 }
        });
        let out = parse_coingecko(&body, &["euro-coin", "idrx", "real-mxn"]);
        assert_eq!(out["euro-coin"], 1.141);
        assert_eq!(out["idrx"], 0.000056);
        assert_eq!(out["real-mxn"], 0.0573);
    }

    #[test]
    fn coingecko_omits_missing_and_non_positive() {
        let body = json!({ "euro-coin": { "usd": 1.14 }, "xsgd": { "usd": 0.0 } });
        let out = parse_coingecko(&body, &["euro-coin", "xsgd", "tokenised-gbp"]);
        assert_eq!(out.len(), 1);
        assert!(out.contains_key("euro-coin"));
        // Zero price and an absent id are both dropped.
        assert!(!out.contains_key("xsgd"));
        assert!(!out.contains_key("tokenised-gbp"));
    }
}
