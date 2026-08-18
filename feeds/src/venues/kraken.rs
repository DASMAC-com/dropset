// cspell:word altname
// cspell:word EURCEUR
// cspell:word USDTZUSD
// cspell:word ZARPUSD
//! The Kraken public-ticker adapter (docs/data-feeds.md §9) — the batched
//! basis and **peg-truth** venue.
//!
//! One keyless `/0/public/Ticker` request prices every pair the source was
//! built with. It carries two legs the other adapters cannot:
//!
//! - **`USDC/USD` — a real market print of the USDC peg.** This is the
//!   portfolio-wide common-mode leg (market-making.md §1 fm1), and Kraken is
//!   the venue that actually quotes it: Coinbase Exchange lists no `USDC-USD`
//!   product, and Binance.US quotes an administered flat `1.00000000`. It
//!   replaces the CoinGecko `usd-coin` proxy the maker used before.
//! - **`EURC/EUR` — token against its own fiat**, the cross redemption
//!   arbitrage enforces directly, and the closest *live* stand-in for an
//!   issuer redemption rate (Circle publishes no keyless one —
//!   `/v1/exchange/rates` is credentialed). **This adapter can decode it, but
//!   no consumer subscribes to it yet**: the maker's roster asks only for
//!   `<token>/USD` plus the shared `USDC/USD`. It is listed here as the
//!   venue's capability, not as a wired leg.
//!
//! **Pairs are Kraken's own names, not ours.** Kraken keys its response by its
//! canonical pair name, which for legacy assets carries the `X`/`Z` prefixes
//! (`USDTZUSD` for USDT/USD). Pass the name Kraken itself uses — `/0/public/
//! AssetPairs` lists them as `altname` — and a pair that still doesn't match is
//! omitted like any unquoted symbol, never guessed at.

use super::Quotes;
use crate::{Batch, HttpClient, Source};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

/// A poll [`Source`] over Kraken's batched public ticker, keyed by the
/// Kraken pair names the source was built with.
pub struct KrakenSource {
    http: HttpClient,
    pairs: Vec<String>,
}

impl KrakenSource {
    /// Build the source over `base_url`, batching `pairs` in every poll.
    pub fn new(base_url: &str, pairs: Vec<String>) -> Result<Self> {
        Ok(Self {
            http: HttpClient::new(base_url)?,
            pairs,
        })
    }

    /// Fetch every pair this source was built with, in one request. Pairs
    /// Kraken does not quote are **omitted** rather than erroring, per the
    /// batched-poll convention in [`super`].
    pub async fn poll(&self) -> Result<Quotes<String>> {
        if self.pairs.is_empty() {
            return Ok(Quotes::new());
        }
        let csv = self.pairs.join(",");
        let body: Value = self
            .http
            .get_json("/0/public/Ticker", &[("pair", &csv)])
            .await?;
        let pairs: Vec<&str> = self.pairs.iter().map(String::as_str).collect();
        Ok(parse_kraken(&body, &pairs))
    }
}

#[async_trait]
impl Source for KrakenSource {
    type Record = Quotes<String>;
    fn name(&self) -> &str {
        "kraken"
    }
    async fn next(&mut self) -> Result<Batch<Self::Record>> {
        Ok(Batch::new(vec![self.poll().await?]))
    }
}

/// Decode Kraken's `{"error":[],"result":{"<pair>":{"a":[…],"b":[…],"c":[…]}}}`
/// ticker response into `pair → price`, keeping only positive finite readings.
///
/// The price is the **bid/ask mid** when both sides are quoted, falling back to
/// the last trade (`c[0]`). A peg reading wants the mid: the last trade sits on
/// whichever side happened to lift, which on a pair that trades within a
/// fraction of a basis point is most of the signal.
///
/// A populated `error` array is not fatal on its own — Kraken reports an
/// unknown pair there while still answering for the rest — so whatever
/// `result` holds is decoded and the unmatched pairs are simply omitted.
pub fn parse_kraken(body: &Value, pairs: &[&str]) -> Quotes<String> {
    let mut out = Quotes::new();
    let Some(result) = body.get("result") else {
        return out;
    };
    for &pair in pairs {
        // Kraken answers under its own canonical name; accept an exact match
        // first and a case-insensitive one after, but never guess past that.
        let entry = result.get(pair).or_else(|| {
            result
                .as_object()?
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(pair))
                .map(|(_, v)| v)
        });
        let Some(entry) = entry else { continue };
        if let Some(price) = mid_or_last(entry) {
            out.insert(pair.to_string(), price);
        }
    }
    out
}

/// The bid/ask mid when both are positive and finite, else the last trade.
fn mid_or_last(entry: &Value) -> Option<f64> {
    let level = |k: &str| {
        entry
            .get(k)
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v > 0.0)
    };
    match (level("b"), level("a")) {
        // Both sides are finite and positive, but their mid still has to be
        // re-checked: two ~1e308 quotes sum to infinity. Fall through to the
        // last trade rather than emitting a non-finite "price", so the
        // function delivers the invariant its doc comment claims.
        (Some(bid), Some(ask)) => match (bid + ask) / 2.0 {
            mid if mid.is_finite() && mid > 0.0 => Some(mid),
            _ => level("c"),
        },
        _ => level("c"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A captured Kraken ticker response, trimmed to the fields decoded here.
    fn body() -> Value {
        json!({
            "error": [],
            "result": {
                "USDCUSD": {
                    "a": ["0.99980000", "1", "1.0"],
                    "b": ["0.99970000", "1", "1.0"],
                    "c": ["0.99970000", "100.0"]
                },
                "EURCEUR": {
                    "a": ["0.99980000", "1", "1.0"],
                    "b": ["0.99960000", "1", "1.0"],
                    "c": ["0.99990000", "50.0"]
                }
            }
        })
    }

    #[test]
    fn takes_the_bid_ask_mid() {
        let out = parse_kraken(&body(), &["USDCUSD", "EURCEUR"]);
        assert!((out["USDCUSD"] - 0.999_75).abs() < 1e-12);
        // The mid (0.9997) differs from the last trade (0.9999) — the point of
        // preferring it on a pair this tight.
        assert!((out["EURCEUR"] - 0.999_70).abs() < 1e-12);
    }

    #[test]
    fn falls_back_to_the_last_trade_without_a_two_sided_quote() {
        let body = json!({
            "result": { "USDCUSD": { "b": ["0"], "c": ["0.99950000", "1.0"] } }
        });
        let out = parse_kraken(&body, &["USDCUSD"]);
        assert!((out["USDCUSD"] - 0.999_5).abs() < 1e-12);
    }

    #[test]
    fn omits_an_unquoted_pair_and_still_prices_the_rest() {
        // Kraken reports the unknown pair in `error` and answers for the other.
        let body = json!({
            "error": ["EQuery:Unknown asset pair"],
            "result": { "USDCUSD": { "a": ["1.0"], "b": ["1.0"], "c": ["1.0"] } }
        });
        let out = parse_kraken(&body, &["USDCUSD", "ZARPUSD"]);
        assert_eq!(out.len(), 1);
        assert!(out.contains_key("USDCUSD"));
    }

    #[test]
    fn matches_a_pair_case_insensitively() {
        let out = parse_kraken(&body(), &["usdcusd"]);
        assert!(out.contains_key("usdcusd"));
    }

    #[test]
    fn drops_a_non_positive_or_malformed_price() {
        let body = json!({
            "result": {
                "AAA": { "c": ["0"] },
                "BBB": { "c": ["not-a-number"] },
                "CCC": {}
            }
        });
        assert!(parse_kraken(&body, &["AAA", "BBB", "CCC"]).is_empty());
    }

    #[test]
    fn missing_result_yields_nothing() {
        let body = json!({ "error": ["EGeneral:Invalid arguments"] });
        assert!(parse_kraken(&body, &["USDCUSD"]).is_empty());
    }
}
