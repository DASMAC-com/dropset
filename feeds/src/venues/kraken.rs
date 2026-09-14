// cspell:word altname
// cspell:word CADCUSD
// cspell:word EURCEUR
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
//!   `/v1/exchange/rates` is credentialed). **The market-data tick collector
//!   records it; the maker does not read it**, its roster asking only for
//!   `<token>/USD` plus the shared `USDC/USD`. So it is a stored series
//!   rather than a wired model leg.
//!
//! **Pairs are Kraken's own names, not ours.** Kraken keys its response by its
//! canonical pair name, which for legacy assets carries the `X`/`Z` prefixes
//! (`USDTZUSD` for USDT/USD). Pass the name Kraken itself uses — `/0/public/
//! AssetPairs` lists them as `altname` — and a pair that still doesn't match is
//! omitted rather than guessed at.
//!
//! **This venue refuses a batch it cannot fully price, so honoring the
//! batched-poll contract in [`venues`](super) takes extra work here.** Measured
//! against the live endpoint (2026-09-14): `pair=USDCUSD,CADCUSD`, with only
//! the second unlisted, answers **HTTP 200** with
//! `{"error":["EQuery:Unknown asset pair"]}` and **no `result` key at all** —
//! not a partial result. One unlisted symbol therefore zeroes every pair in the
//! batch, and because the status is 200 and an empty decode is indistinguishable
//! from a quiet venue, the loss is completely silent.
//!
//! So a refused batch is not passed through as "nothing quoted". It is
//! **isolated**: each pair is re-requested on its own, the ones the venue
//! actually refuses are named in a warning and remembered by the source itself
//! (its private `unlisted` set) so later polls stop carrying them, and every
//! pair that does price survives. The pass costs one request per pair, and
//! only on the poll that discovers the problem — once the offenders are
//! remembered, the batch is clean again.

use super::Quotes;
use crate::{Batch, HttpClient, Source};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

/// The floor between two requests on this venue.
///
/// Kraken documents its public endpoints as safe at **1 call per second or
/// less**, backed by a decrementing call counter rather than a fixed window.
/// That is 4× stricter than the shared client's 250 ms default.
///
/// **1.2 s rather than the 1 s the guidance names**, for the same reason as
/// Pyth's and CoinGecko's floors: a floor equal to the documented rate sits
/// exactly on the cap, which leaves nothing for a retry or for a second process
/// on the same IP. Kraken is the mildest of the three — its counter decrements
/// continuously, and its own wording ("or less") reads as guidance rather than
/// a hard window — but there is no reason to spend the margin, and keeping all
/// three floors strictly inside their limits means the rule holds without an
/// exception to remember.
///
/// It does not bind today — this source batches every pair into one request
/// and never pages, and the maker polls it every 15 s. It is here so the
/// constraint lives at the transport, where it binds the moment anyone adds
/// paging or a retry loop.
const MIN_REQUEST_INTERVAL: Duration = Duration::from_millis(1_200);

/// Kraken's error text for a pair it does not list. Matched as a substring
/// because the venue prefixes an error class (`EQuery:Unknown asset pair`) and
/// the class has changed before while the text has not.
const UNKNOWN_PAIR_ERROR: &str = "Unknown asset pair";

/// A poll [`Source`] over Kraken's batched public ticker, keyed by the
/// Kraken pair names the source was built with.
pub struct KrakenSource {
    http: HttpClient,
    pairs: Vec<String>,
    /// Pairs the venue has answered [`UNKNOWN_PAIR_ERROR`] for, excluded from
    /// later batches so one bad roster entry cannot keep poisoning the rest.
    ///
    /// Behind a `Mutex` rather than taken as `&mut self`, so `poll` keeps the
    /// `&self` signature the batched-poll contract in [`venues`](super) gives
    /// every adapter — the same reason [`HttpClient`] holds its rate-limit gate
    /// this way. The set only ever grows, and is keyed by the roster spelling
    /// rather than by whatever name Kraken answers under, because it is the
    /// roster entry that has to stop being sent.
    unlisted: Mutex<BTreeSet<String>>,
}

impl KrakenSource {
    /// Build the source over `base_url`, batching `pairs` in every poll.
    pub fn new(base_url: &str, pairs: Vec<String>) -> Result<Self> {
        Ok(Self {
            http: HttpClient::new(base_url)?.with_min_interval(MIN_REQUEST_INTERVAL),
            pairs,
            unlisted: Mutex::new(BTreeSet::new()),
        })
    }

    /// Fetch every pair this source was built with, in one request. Pairs
    /// Kraken does not quote are **omitted** rather than erroring, per the
    /// batched-poll convention in [`venues`](super).
    ///
    /// Kraken does not omit them for us — it refuses the whole batch (see the
    /// module docs) — so a refusal falls through to the private `isolate` pass,
    /// which recovers every pair that prices and remembers the ones that do not.
    pub async fn poll(&self) -> Result<Quotes<String>> {
        let pairs = self.batch_pairs();
        if pairs.is_empty() {
            return Ok(Quotes::new());
        }
        let refs: Vec<&str> = pairs.iter().map(String::as_str).collect();
        let body = self.fetch(&refs).await?;
        match classify_ticker(&body, &refs) {
            TickerBatch::Answered(quotes) => Ok(quotes),
            TickerBatch::UnknownPair => self.isolate(&refs).await,
        }
    }

    /// The roster minus everything already known to be unlisted.
    fn batch_pairs(&self) -> Vec<String> {
        let unlisted = self.unlisted.lock().unwrap_or_else(PoisonError::into_inner);
        self.pairs
            .iter()
            .filter(|pair| !unlisted.contains(*pair))
            .cloned()
            .collect()
    }

    /// One batched ticker request over `pairs`.
    async fn fetch(&self, pairs: &[&str]) -> Result<Value> {
        let csv = pairs.join(",");
        self.http
            .get_json("/0/public/Ticker", &[("pair", &csv)])
            .await
    }

    /// Re-request each pair alone to find which ones the venue actually
    /// refuses, keeping every pair that prices.
    ///
    /// Returns quotes rather than an error: the point of the pass is that a
    /// healthy pair should survive a sick one. The offenders are logged at
    /// `WARN` and remembered, so this runs **once** per bad roster entry rather
    /// than on every poll.
    async fn isolate(&self, pairs: &[&str]) -> Result<Quotes<String>> {
        tracing::warn!(
            venue = FEED_NAME,
            pairs = pairs.join(","),
            "kraken refused the whole batch over an unlisted pair; \
             re-requesting each pair alone to find it"
        );
        let mut responses = Vec::with_capacity(pairs.len());
        for &pair in pairs {
            match self.fetch(&[pair]).await {
                Ok(body) => responses.push((pair, body)),
                // A transport failure is not evidence the pair is unlisted, so
                // it is skipped for this poll rather than remembered — the next
                // poll will batch it again.
                Err(err) => tracing::warn!(
                    venue = FEED_NAME,
                    pair,
                    error = %err,
                    "kraken isolation request failed; leaving the pair in the roster"
                ),
            }
        }
        let (quotes, unlisted) = fold_isolated(&responses);
        if !unlisted.is_empty() {
            tracing::warn!(
                venue = FEED_NAME,
                unlisted = unlisted.join(","),
                surviving = quotes.len(),
                "kraken does not list these pairs; dropping them from later \
                 batches. Fix the roster: the spelling may need Kraken's \
                 legacy X/Z prefix, or the token may not be listed at all"
            );
            let mut known = self.unlisted.lock().unwrap_or_else(PoisonError::into_inner);
            known.extend(unlisted);
            // Remembering the offenders is what stops the isolation pass running
            // every poll — but if it swallowed the entire roster, every later
            // poll returns empty with nothing left to warn about. Say so once,
            // here, rather than leaving the venue quietly dark.
            if known.len() >= self.pairs.len() {
                tracing::error!(
                    venue = FEED_NAME,
                    roster = self.pairs.join(","),
                    "kraken lists NONE of the configured pairs; this venue will \
                     now produce nothing at all until the roster is fixed"
                );
            }
        }
        Ok(quotes)
    }
}

/// This source's [`Source::name`] — see [`crate::venues::pyth::FEED_NAME`] for
/// why the name is a constant rather than a literal at each use.
pub const FEED_NAME: &str = "kraken";

#[async_trait]
impl Source for KrakenSource {
    type Record = Quotes<String>;
    fn name(&self) -> &str {
        FEED_NAME
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
/// A populated `error` array is not fatal on its own, so whatever `result`
/// holds is decoded and the unmatched pairs are simply omitted.
///
/// This is a **pure decode and nothing more**: it cannot distinguish a venue
/// that quoted none of these pairs from one that refused the request, because
/// both arrive as an absent or unmatched `result`. That distinction is
/// [`classify_ticker`]'s job, and it is the one the caller needs — see the
/// module docs for why an empty decode here used to mean silent total loss.
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

/// What one batched ticker response says about the batch **as a whole**.
///
/// The distinction [`parse_kraken`] cannot draw on its own: an empty decode is
/// either a venue with nothing to say or a venue that threw the request out.
#[derive(Debug, PartialEq)]
pub enum TickerBatch {
    /// The venue answered. Carries whatever it priced, which may be empty
    /// because the pairs genuinely have no quote.
    Answered(Quotes<String>),
    /// The venue rejected the request over a pair it does not list, and so
    /// priced **nothing** — including the pairs it does list.
    UnknownPair,
}

/// Decide whether `body` is an answer or an unlisted-pair refusal.
///
/// Anything that priced at all counts as an answer: a response carrying both
/// quotes and an unknown-pair error has already omitted the offender, so there
/// is nothing to isolate and nothing was lost. Only a decode that came back
/// **empty** while the venue named an unlisted pair is a refusal.
///
/// An empty decode with some *other* error (`EGeneral:Invalid arguments`, a rate
/// limit) stays [`TickerBatch::Answered`] deliberately: re-requesting each pair
/// alone would not fix it and would multiply the failing call by the roster
/// size. Those already surface through the transport or the collector's silence
/// watch.
pub fn classify_ticker(body: &Value, pairs: &[&str]) -> TickerBatch {
    let quotes = parse_kraken(body, pairs);
    if quotes.is_empty() && names_unknown_pair(body) {
        return TickerBatch::UnknownPair;
    }
    TickerBatch::Answered(quotes)
}

/// Whether the response's `error` array names an unlisted pair.
fn names_unknown_pair(body: &Value) -> bool {
    body.get("error")
        .and_then(Value::as_array)
        .is_some_and(|errors| {
            errors
                .iter()
                .filter_map(Value::as_str)
                .any(|error| error.contains(UNKNOWN_PAIR_ERROR))
        })
}

/// Fold an isolation pass's per-pair responses into the quotes that survived
/// and the pairs the venue does not list.
///
/// Split out from the source's private `isolate` pass so the recovery logic is
/// unit testable against captured responses with no network, per the decode
/// convention in [`venues`](super) — the transport half of that pass is a plain
/// loop of single-pair requests.
pub fn fold_isolated(responses: &[(&str, Value)]) -> (Quotes<String>, Vec<String>) {
    let mut quotes = Quotes::new();
    let mut unlisted = Vec::new();
    for (pair, body) in responses {
        match classify_ticker(body, &[pair]) {
            TickerBatch::Answered(priced) => quotes.extend(priced),
            TickerBatch::UnknownPair => unlisted.push((*pair).to_string()),
        }
    }
    (quotes, unlisted)
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
    use crate::venues::requests_per_window;
    use serde_json::json;

    #[test]
    fn the_floor_stays_inside_krakens_documented_one_call_per_second() {
        let per_second = requests_per_window(MIN_REQUEST_INTERVAL, Duration::from_secs(1));
        assert!(
            per_second < 1.0,
            "{per_second} requests/second does not sit strictly inside Kraken's \
             documented ~1/s for public endpoints"
        );
    }

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
        // A body carrying both an error and a partial result. **Kraken does not
        // actually answer this way** — it refuses the batch outright, which is
        // what `the_live_refusal_shape_is_a_bare_error_with_no_result` pins.
        // This covers the decoder's own tolerance, which is what lets a
        // recovered isolation response decode cleanly.
        let body = json!({
            "error": ["EQuery:Unknown asset pair"],
            "result": { "USDCUSD": { "a": ["1.0"], "b": ["1.0"], "c": ["1.0"] } }
        });
        let out = parse_kraken(&body, &["USDCUSD", "ZARPUSD"]);
        assert_eq!(out.len(), 1);
        assert!(out.contains_key("USDCUSD"));
    }

    /// The response the live endpoint really returns for a batch holding one
    /// unlisted pair, captured 2026-09-14 from
    /// `/0/public/Ticker?pair=USDCUSD,CADCUSD` — **HTTP 200**, no `result` key.
    fn live_refusal() -> Value {
        json!({ "error": ["EQuery:Unknown asset pair"] })
    }

    #[test]
    fn the_live_refusal_shape_is_a_bare_error_with_no_result() {
        // The regression this file exists for: decoded alone this is empty, and
        // an empty decode used to be returned as "the venue quoted nothing",
        // silently zeroing every healthy pair in the batch.
        assert!(parse_kraken(&live_refusal(), &["USDCUSD", "CADCUSD"]).is_empty());
        assert_eq!(
            classify_ticker(&live_refusal(), &["USDCUSD", "CADCUSD"]),
            TickerBatch::UnknownPair
        );
    }

    #[test]
    fn an_isolation_pass_keeps_the_good_pairs_and_names_the_bad_one() {
        // One unlisted pair (CADCUSD, which Kraken genuinely does not list)
        // beside two healthy ones. The healthy pairs must survive.
        let responses = vec![
            ("USDCUSD", body()),
            ("CADCUSD", live_refusal()),
            ("EURCEUR", body()),
        ];
        let (quotes, unlisted) = fold_isolated(&responses);
        assert_eq!(unlisted, vec!["CADCUSD".to_string()]);
        assert!(quotes.contains_key("USDCUSD"));
        assert!(quotes.contains_key("EURCEUR"));
    }

    #[test]
    fn a_partly_priced_batch_is_an_answer_rather_than_a_refusal() {
        // Nothing was lost, so there is nothing to isolate and no reason to
        // spend a request per pair.
        let body = json!({
            "error": ["EQuery:Unknown asset pair"],
            "result": { "USDCUSD": { "a": ["1.0"], "b": ["1.0"], "c": ["1.0"] } }
        });
        assert!(matches!(
            classify_ticker(&body, &["USDCUSD", "ZARPUSD"]),
            TickerBatch::Answered(_)
        ));
    }

    #[test]
    fn an_unrelated_error_is_not_read_as_an_unlisted_pair() {
        // Isolating would not fix this and would multiply the failing call by
        // the roster size.
        let body = json!({ "error": ["EGeneral:Invalid arguments"] });
        assert_eq!(
            classify_ticker(&body, &["USDCUSD"]),
            TickerBatch::Answered(Quotes::new())
        );
    }

    #[test]
    fn a_pair_is_only_dropped_once_it_is_asked_for_alone() {
        // An isolation response that prices nothing but names no unlisted pair
        // must not condemn the pair — otherwise a transient venue hiccup would
        // permanently evict a healthy roster entry.
        let responses = vec![("USDCUSD", json!({ "error": [], "result": {} }))];
        let (quotes, unlisted) = fold_isolated(&responses);
        assert!(quotes.is_empty());
        assert!(unlisted.is_empty());
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
