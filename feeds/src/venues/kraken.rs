// cspell:word altname
// cspell:word AUDDUSD
// cspell:word CADCUSD
// cspell:word delisted
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
//! from a quiet venue, this adapter reported nothing at all.
//!
//! **How silent that was, precisely**, since the adapter is not the only thing
//! watching: the market-data collector's `SilenceWatch` does eventually report
//! the pairs, but only after `SILENCE_THRESHOLD` consecutive silent polls, and
//! it reports them as *unpriced* rather than naming the batch refusal that
//! caused them. The maker's feed path has no such watch at all. So the adapter
//! itself said nothing, and what did speak was late and pointed elsewhere.
//!
//! So a refused batch is not passed through as "nothing quoted". It is
//! **isolated**: each pair is re-requested on its own, the ones the venue
//! actually refuses are named in a warning and remembered by the source itself
//! (its private `unlisted` map) so later polls stop carrying them, and every
//! pair that does price survives. The pass costs one request per pair, and
//! runs on the poll that discovers the problem — after which the batch is clean
//! until the eviction's `EVICTION_TTL` elapses and the pair is re-admitted to
//! be tried again. The memory is per-instance rather than durable.
//!
//! (A code span rather than an intra-doc link because the constant is private,
//! and these module docs are public — the rustdoc hook rejects the link, and
//! widening the constant to make it resolve would export an implementation
//! detail to fix a docs problem.)
//!
//! **What a non-`online` pair status does NOT do**, measured against the live
//! endpoint (2026-09-15), because the shape of that answer is what bounds the
//! re-admission path above: Kraken's pair `status` gates **order placement, not
//! market data**. Of 1448 listed pairs, 78 were `cancel_only` and 17
//! `post_only`, and every one probed priced normally — HTTP 200, an empty
//! `error`, a populated `result` — both alone and batched beside `online` pairs.
//! There is no `maintenance` status in `/0/public/AssetPairs` at all. So a
//! restricted-trading window cannot produce the unlisted-pair refusal and
//! cannot evict anything, and the only path to a *transient* refusal is a pair
//! leaving the venue's listings and returning.

use super::Quotes;
use crate::{Batch, HttpClient, Source};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

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

/// Kraken's error text for a pair it does not list.
///
/// Matched as a **substring** because the venue prefixes an error class
/// (`EQuery:Unknown asset pair`), and that prefix is not part of any documented
/// contract — so only the text itself is relied on. This says nothing about
/// whether the prefix has ever changed, which is not something this repo has
/// measured.
const UNKNOWN_PAIR_ERROR: &str = "Unknown asset pair";

/// How long a pair stays evicted before the source tries it again.
///
/// Without this the eviction memory only ever grows, so any refusal — however
/// temporary — drops the pair for the remaining life of the process and
/// recovery is a **restart**.
///
/// **One hour, and the reason it is hours rather than minutes is that eviction
/// is usually CORRECT.** Measured against the live endpoint (2026-09-15), four
/// of the Kraken spellings this repo derives are absent from `AssetPairs`
/// outright — AUDD, CADC, MXNe and ZARP are not listed at all — so for a roster
/// carrying them the steady state is a standing, accurate eviction that
/// re-probing will never clear. The TTL is therefore priced as recurring waste
/// against a rare win, and it wants to be long.
///
/// **What one expiry costs**, which is more than one request: a re-admitted pair
/// rejoins the *batch*, and if the venue still refuses it that batch is refused
/// whole, so the expiry spends one wasted batch request plus a full isolation
/// pass of one request per roster entry. No reading is lost — the isolation pass
/// recovers every pair that prices, which is the whole point of it — but at the
/// [`MIN_REQUEST_INTERVAL`] floor a 5-pair roster spends ~6 s of a 15 s poll
/// tick doing it. Hourly that is negligible; at a minute it would be most of
/// what the source does.
///
/// **Why a TTL rather than a separate re-probe loop**, which the alternative
/// design would have been: an expired pair rejoining the batch drives the
/// *existing* isolation machinery, which already re-requests each pair alone,
/// already decides correctly which ones the venue refuses, and already logs and
/// re-remembers them. A dedicated re-probe would duplicate all of that and add a
/// second scheduling path to reason about. The cost is the wasted batch request
/// named above; a re-probe loop would have avoided that one request and bought a
/// parallel implementation of everything else.
const EVICTION_TTL: Duration = Duration::from_secs(60 * 60);

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
    /// this way. It is keyed by the roster spelling rather than by whatever name
    /// Kraken answers under, because it is the roster entry that has to stop
    /// being sent.
    ///
    /// **Each entry carries the instant it was evicted, and expires after
    /// [`EVICTION_TTL`]** — so a refusal that was only ever *temporary* costs a
    /// bounded outage rather than the remaining life of the process. On expiry
    /// the pair simply rejoins the batch; if the venue still refuses it, the
    /// ordinary isolation pass evicts it again with a fresh timestamp.
    ///
    /// **The one transient this actually guards is a pair leaving the listings
    /// and returning**, which the module docs record as the only path left after
    /// the pair-`status` hypothesis was measured and ruled out. That case is not
    /// hypothetical for this repo: the FX stablecoins the roster targets are
    /// unlisted on Kraken **today**, so a new listing for one of them is an
    /// expected event, and before this TTL a long-lived collector could not see
    /// it happen.
    unlisted: Mutex<BTreeMap<String, Instant>>,
}

impl KrakenSource {
    /// Build the source over `base_url`, batching `pairs` in every poll.
    pub fn new(base_url: &str, pairs: Vec<String>) -> Result<Self> {
        Ok(Self {
            http: HttpClient::new(base_url)?.with_min_interval(MIN_REQUEST_INTERVAL),
            pairs,
            unlisted: Mutex::new(BTreeMap::new()),
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
            TickerBatch::Answered(quotes) => {
                // A refusal this adapter does NOT model — nothing priced, and
                // the venue named an error that is not the unlisted-pair one.
                // Isolating would not help (it would multiply a failing call by
                // the roster size), but staying quiet here would reproduce the
                // exact whole-venue silence this module exists to end, one error
                // class over.
                //
                // Unlike the unlisted-pair path there is no memory behind this,
                // so it fires on **every** poll while the condition persists —
                // once every 15 s for a 200-wrapped rate limit. That is the
                // intended trade while it is still unknown which error classes
                // can reach here: a repeated line is recoverable, and the
                // silence it replaces was not.
                if quotes.is_empty() {
                    if let Some(errors) = venue_errors(&body) {
                        tracing::warn!(
                            venue = FEED_NAME,
                            pairs = refs.join(","),
                            errors,
                            "kraken priced nothing and reported an error this \
                             adapter does not model; the whole batch is empty"
                        );
                    }
                }
                Ok(quotes)
            }
            TickerBatch::UnknownPair => self.isolate(&refs).await,
        }
    }

    /// The roster minus everything still known to be unlisted, re-admitting any
    /// eviction whose [`EVICTION_TTL`] has elapsed.
    fn batch_pairs(&self) -> Vec<String> {
        self.batch_pairs_at(Instant::now())
    }

    /// [`Self::batch_pairs`] against an explicit clock.
    ///
    /// The clock is a parameter rather than an `Instant::now()` call inside so
    /// the TTL is testable without sleeping: a test builds one base instant and
    /// asks what the batch looks like at `base + EVICTION_TTL`. Sleeping through
    /// a real hour is not an option, and making the TTL itself configurable to
    /// dodge that would mean the tested value is never the shipped one.
    fn batch_pairs_at(&self, now: Instant) -> Vec<String> {
        let mut unlisted = self.unlisted.lock().unwrap_or_else(PoisonError::into_inner);
        let readmitted = expire_evictions(&mut unlisted, now);
        // Say this at INFO rather than WARN: a re-admission is this design
        // working, not a fault. It is worth saying at all because the pair is
        // about to reappear in a batch and — if the venue still refuses it — to
        // trigger an isolation pass whose warnings would otherwise look like a
        // fresh problem rather than an hourly retry.
        if !readmitted.is_empty() {
            tracing::info!(
                venue = FEED_NAME,
                readmitted = readmitted.join(","),
                still_evicted = unlisted.len(),
                "kraken eviction TTL elapsed; re-admitting these pairs to the \
                 batch to see whether the venue lists them now"
            );
        }
        self.pairs
            .iter()
            .filter(|pair| !unlisted.contains_key(*pair))
            .cloned()
            .collect()
    }

    /// Whether every roster entry is now remembered as unlisted, so this source
    /// prices nothing at all until an [`EVICTION_TTL`] elapses and re-admits
    /// something.
    ///
    /// **The TTL bounds this state but does not make it benign.** It is still
    /// worth an `ERROR`: the recovery it offers is an hourly retry of a roster
    /// the venue has rejected in full, which for a wholly misspelled roster will
    /// keep failing forever. What changed is that the operator's fix no longer
    /// has to include a restart.
    ///
    /// Tested by **containment, not by comparing lengths**: `known` is keyed by
    /// pair and the roster is a `Vec`, so two roster entries sharing one Kraken
    /// spelling would leave `known.len()` short of `self.pairs.len()` and silence
    /// the alarm in precisely the state it exists to catch. (`resolve_venue`
    /// rejects that collision for the market-data collector, but nothing in this
    /// type's signature promises it, and the maker builds its roster by another
    /// path.)
    ///
    /// Split out from `isolate` so this predicate is reachable from a unit test
    /// — inline, its only caller was the transport half, which nothing tests.
    fn roster_exhausted(&self, known: &BTreeMap<String, Instant>) -> bool {
        self.pairs.iter().all(|pair| known.contains_key(pair))
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
    /// `WARN` and remembered, which keeps the pass off the steady-state path.
    ///
    /// **It does not run only once.** It runs once per offender *that the venue
    /// actually refuses when asked alone* — so it repeats on the next poll in
    /// two cases the code handles deliberately: a single-pair request that fails
    /// at the transport (nothing is remembered, because a transport error is not
    /// evidence a pair is unlisted), and a batch refusal that no single pair
    /// reproduces. It also repeats once per [`EVICTION_TTL`], by design, since a
    /// re-admitted pair that is still unlisted refuses the batch again — that
    /// recurring cost is priced in the TTL's own docs. In those cases the
    /// steady-state cost is one request per
    /// roster entry per poll instead of one. At the 1.2 s floor that is ~6 s for
    /// the 5-pair default roster, inside the 15 s poll interval — but it scales
    /// linearly, so a roster past roughly a dozen pairs would outrun its own
    /// tick and wants a per-poll cap.
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
            // Stamped with one instant taken outside the loop, so every pair a
            // single pass evicts expires together. That is deliberate and it is
            // the cheaper arrangement: pairs sharing a deadline are re-admitted
            // by one isolation pass rather than by one pass each.
            let evicted_at = Instant::now();
            let mut known = self.unlisted.lock().unwrap_or_else(PoisonError::into_inner);
            known.extend(unlisted.into_iter().map(|pair| (pair, evicted_at)));
            // Remembering the offenders is what stops the isolation pass running
            // every poll — but if it swallowed the entire roster, every later
            // poll returns empty with nothing left to warn about. Say so once,
            // here, rather than leaving the venue quietly dark.
            //
            if self.roster_exhausted(&known) {
                tracing::error!(
                    venue = FEED_NAME,
                    roster = self.pairs.join(","),
                    "kraken lists NONE of the configured pairs; this venue will \
                     now produce nothing at all until the roster is fixed, \
                     beyond an hourly retry of the whole roster as evictions \
                     expire"
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

/// The response's `error` strings joined, or `None` when it reported none.
///
/// `None` and `Some` are the distinction that matters to both callers: a venue
/// that reported no error at all is a different thing from one that reported an
/// error this adapter does not recognize.
///
/// **`None` is reserved for "the venue reported nothing wrong", and an
/// unreadable `error` is not that.** Both callers key off `None` — the
/// classifier stops looking for an unlisted pair, and `poll` stops warning about
/// an empty batch — so folding a shape this adapter cannot read into `None`
/// would put a refusal back into the silent path this module exists to end, with
/// one JSON shape change disabling the detection and the alarm together. So a
/// present-but-unreadable `error` returns its raw text instead: worse to read,
/// impossible to miss.
fn venue_errors(body: &Value) -> Option<String> {
    let error = body.get("error")?;
    match error.as_array() {
        // The documented shape, and the only one that can mean "no error".
        Some(errors) if errors.is_empty() => None,
        Some(errors) => {
            let strings: Vec<&str> = errors.iter().filter_map(Value::as_str).collect();
            match strings.is_empty() {
                // A non-empty array holding no strings — readable as JSON only.
                true => Some(error.to_string()),
                false => Some(strings.join("; ")),
            }
        }
        // Not an array at all. A bare string would in fact be the friendlier
        // shape; either way it is reported rather than swallowed.
        None => Some(error.to_string()),
    }
}

/// Whether the response's `error` array names an unlisted pair.
fn names_unknown_pair(body: &Value) -> bool {
    venue_errors(body).is_some_and(|errors| errors.contains(UNKNOWN_PAIR_ERROR))
}

/// Fold an isolation pass's per-pair responses into the quotes that survived
/// and the pairs the venue does not list.
///
/// Split out from the source's private `isolate` pass so the recovery logic is
/// unit testable against captured responses with no network, per the decode
/// convention in [`venues`](super) — the transport half of that pass is a plain
/// loop of single-pair requests.
///
/// Private: it exists to make `isolate` testable and its tests live in this
/// module, so nothing outside needs to destructure its untagged return.
fn fold_isolated(responses: &[(&str, Value)]) -> (Quotes<String>, Vec<String>) {
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

/// Drop every eviction whose [`EVICTION_TTL`] has elapsed as of `now`, and
/// report the pairs that were re-admitted.
///
/// Pure and free-standing for the same reason as [`fold_isolated`]: it is the
/// half of the re-admission path worth testing, and taking `now` as an argument
/// means testing it costs no sleeping and no fake clock.
///
/// **`duration_since` saturates rather than panicking** when `now` precedes the
/// stored instant, which is the behavior this wants: a clock that appears to run
/// backwards yields a zero elapsed time, so the pair stays evicted instead of
/// being re-admitted early. `Instant` is monotonic so that should not arise, but
/// the failure mode if it did is the conservative one either way.
fn expire_evictions(evicted: &mut BTreeMap<String, Instant>, now: Instant) -> Vec<String> {
    let expired: Vec<String> = evicted
        .iter()
        .filter(|(_, at)| now.duration_since(**at) >= EVICTION_TTL)
        .map(|(pair, _)| pair.clone())
        .collect();
    for pair in &expired {
        evicted.remove(pair);
    }
    expired
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
    use crate::testing::{json_response, request_line, serve_sequence_capturing};
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
    fn pricing_nothing_is_not_enough_to_condemn_a_pair() {
        // An isolation response that prices nothing but names no unlisted pair
        // must not condemn the pair — otherwise a transient venue hiccup would
        // permanently evict a healthy roster entry, and eviction has no
        // re-admission path.
        let responses = vec![("USDCUSD", json!({ "error": [], "result": {} }))];
        let (quotes, unlisted) = fold_isolated(&responses);
        assert!(quotes.is_empty());
        assert!(unlisted.is_empty());
    }

    /// A source over a base URL nothing listens on. `HttpClient::new` builds a
    /// client
    /// and stores the base URL without performing any I/O, so this reaches no
    /// network — which is what makes the pair-filtering half of the memory
    /// testable with no mock server and no new dependency.
    fn source(pairs: &[&str]) -> KrakenSource {
        source_at("http://127.0.0.1:1", pairs)
    }

    /// A source over `base_url`, for the seam test that needs a live stub port.
    fn source_at(base_url: &str, pairs: &[&str]) -> KrakenSource {
        let pairs = pairs.iter().map(|pair| (*pair).to_string()).collect();
        KrakenSource::new(base_url, pairs).expect("constructing performs no I/O")
    }

    /// Insert `pairs` into the source's remembered-unlisted map, evicted `at`.
    fn remember_at(source: &KrakenSource, pairs: &[&str], at: Instant) {
        let mut unlisted = source
            .unlisted
            .lock()
            .expect("this test holds the lock alone");
        unlisted.extend(pairs.iter().map(|pair| ((*pair).to_string(), at)));
    }

    /// [`remember_at`] with the eviction stamped now, for the cases that are
    /// about the filtering rather than about the TTL.
    fn remember(source: &KrakenSource, pairs: &[&str]) {
        remember_at(source, pairs, Instant::now());
    }

    /// An eviction map for the `roster_exhausted` cases, which care only about
    /// which keys are present.
    fn evictions(pairs: &[&str]) -> BTreeMap<String, Instant> {
        let now = Instant::now();
        pairs
            .iter()
            .map(|pair| ((*pair).to_string(), now))
            .collect()
    }

    #[test]
    fn a_remembered_pair_leaves_the_batch_and_the_others_stay() {
        // The module's central cost claim: once an offender is remembered, later
        // polls carry a clean batch rather than re-isolating.
        let source = source(&["USDCUSD", "CADCUSD", "EURCEUR"]);
        remember(&source, &["CADCUSD"]);
        assert_eq!(
            source.batch_pairs(),
            vec!["USDCUSD".to_string(), "EURCEUR".to_string()]
        );
    }

    #[test]
    fn a_fully_remembered_roster_batches_nothing() {
        let source = source(&["USDCUSD", "CADCUSD"]);
        remember(&source, &["USDCUSD", "CADCUSD"]);
        assert!(source.batch_pairs().is_empty());
    }

    #[test]
    fn an_eviction_inside_its_ttl_stays_out_of_the_batch() {
        // The cost claim the TTL must not break: within the window the batch is
        // still clean, so the isolation pass stays off the steady-state path.
        let base = Instant::now();
        let source = source(&["USDCUSD", "CADCUSD", "EURCEUR"]);
        remember_at(&source, &["CADCUSD"], base);
        let almost = base + EVICTION_TTL - Duration::from_secs(1);
        assert_eq!(
            source.batch_pairs_at(almost),
            vec!["USDCUSD".to_string(), "EURCEUR".to_string()]
        );
    }

    #[test]
    fn an_eviction_past_its_ttl_rejoins_the_batch() {
        // The whole point of Part 1: a pair the venue refused an hour ago gets
        // another chance without a process restart. Asserted on the full batch
        // rather than on membership, so the roster's own order is pinned too.
        let base = Instant::now();
        let source = source(&["USDCUSD", "CADCUSD", "EURCEUR"]);
        remember_at(&source, &["CADCUSD"], base);
        assert_eq!(
            source.batch_pairs_at(base + EVICTION_TTL),
            vec![
                "USDCUSD".to_string(),
                "CADCUSD".to_string(),
                "EURCEUR".to_string()
            ]
        );
    }

    #[test]
    fn only_the_pairs_whose_ttl_elapsed_are_re_admitted() {
        // Two evictions from different passes. Re-admission is per-entry, so a
        // pair evicted more recently must stay out even though a sibling left.
        let base = Instant::now();
        let source = source(&["USDCUSD", "CADCUSD", "AUDDUSD"]);
        remember_at(&source, &["CADCUSD"], base);
        remember_at(&source, &["AUDDUSD"], base + Duration::from_secs(600));
        assert_eq!(
            source.batch_pairs_at(base + EVICTION_TTL),
            vec!["USDCUSD".to_string(), "CADCUSD".to_string()]
        );
    }

    #[test]
    fn a_wholly_evicted_roster_recovers_once_the_ttl_elapses() {
        // The state `roster_exhausted` alarms on used to be terminal for the
        // life of the process. It is now an outage with an end, which is the
        // behavioral claim that made the alarm's wording change.
        let base = Instant::now();
        let source = source(&["USDCUSD", "CADCUSD"]);
        remember_at(&source, &["USDCUSD", "CADCUSD"], base);
        assert!(source.batch_pairs_at(base).is_empty());
        assert_eq!(
            source.batch_pairs_at(base + EVICTION_TTL),
            vec!["USDCUSD".to_string(), "CADCUSD".to_string()]
        );
    }

    #[test]
    fn an_expiry_is_reported_once_rather_than_every_poll() {
        // Re-admission drops the entry, so the INFO line fires on the poll that
        // re-admits and not on the ones after it. Were the entry left in place
        // with only the filter changed, every subsequent poll would re-announce
        // the same re-admission.
        let base = Instant::now();
        let mut evicted = BTreeMap::from([("CADCUSD".to_string(), base)]);
        let now = base + EVICTION_TTL;
        assert_eq!(expire_evictions(&mut evicted, now), vec!["CADCUSD"]);
        assert!(evicted.is_empty());
        assert!(expire_evictions(&mut evicted, now).is_empty());
    }

    #[test]
    fn a_clock_running_backwards_does_not_re_admit_early() {
        // `duration_since` saturates at zero, so the conservative outcome. Pinned
        // because the arithmetic would otherwise be a subtraction that panics in
        // debug and wraps in release.
        let base = Instant::now();
        let mut evicted = BTreeMap::from([("CADCUSD".to_string(), base)]);
        let earlier = base - Duration::from_secs(30);
        assert!(expire_evictions(&mut evicted, earlier).is_empty());
        assert_eq!(evicted.len(), 1);
    }

    /// One `USDCUSD` reading, for the single-pair legs of the seam test.
    fn priced_alone() -> Value {
        json!({
            "error": [],
            "result": {
                "USDCUSD": {
                    "a": ["0.99980000", "1", "1.0"],
                    "b": ["0.99970000", "1", "1.0"],
                    "c": ["0.99970000", "100.0"]
                }
            }
        })
    }

    #[tokio::test]
    async fn the_transport_seam_isolates_a_refusal_then_batches_clean() {
        // The half of this module nothing reached before: `fold_isolated` and
        // `classify_ticker` are unit-tested against captured bodies, but the
        // loop that turns a refused batch into per-pair requests, remembers the
        // offender and issues a NARROWER batch next poll is transport code, and
        // its bug modes are all in the relationship between consecutive
        // requests — which no single-response stub can express.
        //
        // Four requests, in order: the refused batch, the two isolation probes,
        // then the next poll's batch. Two pairs rather than three deliberately —
        // this test really spends 3 × MIN_REQUEST_INTERVAL in the rate-limit
        // gate, so every extra roster entry costs 1.2 s of wall clock.
        //
        // **Not `start_paused = true`**, which is the obvious way to skip that
        // wait and would make this flaky: the gate sleeps on tokio time, but so
        // does `reqwest`'s 10 s request timeout, and auto-advancing the clock
        // while a real loopback request is in flight can trip that timeout
        // instead of the sleep. Paused time and real I/O do not mix here; 3.6 s
        // of honest waiting is the cheaper trade.
        let responses = vec![
            json_response(&live_refusal().to_string()),
            json_response(&priced_alone().to_string()),
            json_response(&live_refusal().to_string()),
            json_response(&priced_alone().to_string()),
        ];
        let (port, heads) = serve_sequence_capturing(responses).await;
        let source = source_at(&format!("http://127.0.0.1:{port}"), &["USDCUSD", "CADCUSD"]);

        // Poll one: the batch is refused, the isolation pass runs, and the pair
        // that prices survives the pair that does not. This is the whole claim
        // of the module, asserted end-to-end over a socket for the first time.
        let first = source.poll().await.expect("the stub answers every request");
        assert_eq!(first.len(), 1, "the healthy pair should survive: {first:?}");
        assert!(first.contains_key("USDCUSD"));

        // Poll two: CADCUSD is remembered, so it never reaches the wire again.
        let second = source.poll().await.expect("the stub answers every request");
        assert_eq!(second.len(), 1, "{second:?}");
        assert!(second.contains_key("USDCUSD"));

        let heads = heads.lock().expect("the server task is done writing");
        let lines: Vec<&str> = heads.iter().map(|head| request_line(head)).collect();
        assert_eq!(lines.len(), 4, "expected four requests, got {lines:?}");

        // The refused batch carried both pairs. Asserted on both names rather
        // than on the exact query string, which is URL-encoded — the comma
        // separator may arrive as `%2C`, and pinning that would be testing
        // reqwest's escaping rather than this adapter.
        assert!(lines[0].contains("USDCUSD"), "{}", lines[0]);
        assert!(lines[0].contains("CADCUSD"), "{}", lines[0]);

        // The isolation pass asked one pair at a time, in roster order — the
        // property that makes a refusal attributable to a specific pair. Each
        // probe must carry its own pair and NOT its sibling, since a pass that
        // re-sent the batch would also have produced the surviving reading
        // above and passed a presence-only assertion.
        assert!(
            lines[1].contains("USDCUSD") && !lines[1].contains("CADCUSD"),
            "{}",
            lines[1]
        );
        assert!(
            lines[2].contains("CADCUSD") && !lines[2].contains("USDCUSD"),
            "{}",
            lines[2]
        );

        // The payoff: the second poll's batch is narrower than the first. This
        // is the memory observed at the transport rather than through the
        // private field, so it would catch a `batch_pairs` that filtered
        // correctly while `poll` went on sending the unfiltered roster.
        assert!(lines[3].contains("USDCUSD"), "{}", lines[3]);
        assert!(
            !lines[3].contains("CADCUSD"),
            "the remembered pair reached the wire again: {}",
            lines[3]
        );
    }

    #[test]
    fn the_eviction_ttl_is_hours_rather_than_minutes() {
        // A short TTL is not merely wasteful, it is self-defeating: each expiry
        // spends a refused batch plus one request per roster entry, so at a
        // minute the source would spend most of its polls re-probing pairs that
        // are correctly evicted. Pinned against a well-meaning "make it more
        // responsive" edit.
        assert!(
            EVICTION_TTL >= Duration::from_secs(15 * 60),
            "an eviction TTL under 15 minutes cannot pay for its own isolation \
             pass: {EVICTION_TTL:?}"
        );
    }

    #[test]
    fn a_duplicated_roster_entry_still_counts_as_exhausted() {
        // Two roster entries sharing one Kraken spelling. The set holds two
        // names against three entries, so a LENGTH comparison reads 2 < 3 and
        // calls this exhausted roster healthy — skipping the alarm in exactly
        // the state it exists to catch. The first assertion pins that the
        // lengths really do disagree, so this test fails if the predicate is
        // ever rewritten as a length check.
        let source = source(&["USDCUSD", "USDCUSD", "CADCUSD"]);
        let known = evictions(&["USDCUSD", "CADCUSD"]);
        assert!(known.len() < source.pairs.len());
        assert!(source.roster_exhausted(&known));
    }

    #[test]
    fn a_roster_with_one_live_pair_is_not_exhausted() {
        let source = source(&["USDCUSD", "CADCUSD"]);
        let known = evictions(&["CADCUSD"]);
        assert!(!source.roster_exhausted(&known));
    }

    #[test]
    fn venue_errors_distinguishes_no_error_from_an_unrecognized_one() {
        // Not a log assertion (nothing here captures tracing) — this pins the
        // input that drives the warning: an empty answer with a populated error
        // array is distinguishable from a venue that reported no error at all.
        let unrecognized = json!({ "error": ["EGeneral:Invalid arguments"] });
        assert_eq!(
            venue_errors(&unrecognized).as_deref(),
            Some("EGeneral:Invalid arguments")
        );
        // Only these two are "the venue reported nothing wrong".
        assert_eq!(venue_errors(&json!({ "error": [] })), None);
        assert_eq!(venue_errors(&json!({ "result": {} })), None);
    }

    #[test]
    fn an_unreadable_error_shape_is_reported_rather_than_swallowed() {
        // Both callers key off `None`, so folding a shape this adapter cannot
        // read into `None` would disable the classifier and the empty-batch
        // warning together — one JSON change putting a refusal back into the
        // silent path. Every unreadable shape must therefore be `Some`.
        for unreadable in [
            json!({ "error": "EQuery:Unknown asset pair" }),
            json!({ "error": { "code": "EQuery" } }),
            json!({ "error": [{ "code": "EQuery" }] }),
        ] {
            assert!(
                venue_errors(&unreadable).is_some(),
                "an unreadable error shape must not read as no-error: {unreadable}"
            );
        }
        // And a bare-string refusal still routes to isolation rather than being
        // classified as "the venue quoted nothing".
        let as_string = json!({ "error": "EQuery:Unknown asset pair" });
        assert_eq!(
            classify_ticker(&as_string, &["USDCUSD"]),
            TickerBatch::UnknownPair
        );
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
