//! Live checks that OANDA's quote *direction* is what the roster believes.
//!
//! The inversion machinery rests on a fact about the venue rather than on
//! anything in this repository: OANDA's v20 instrument list is direction-fixed,
//! listing exactly one instrument per pair in market convention and rejecting
//! the other outright. `OANDA_REVERSED_PAIRS` in `dropset_market_data::fx` is
//! a hand-maintained table of the pairs it lists backwards, so the one failure
//! no unit test can see is that table drifting away from the venue.
//!
//! These tests are the check for exactly that, and they are the reason the
//! table is safe to extend by measurement.
//!
//! **Why they assert a band and not a rate.** A hardcoded 0.7255 would fail on
//! any day the market moved, which trains the reader to ignore it. The claim
//! worth pinning is structural — the inverted series is on the *right order of
//! magnitude* for the canonical direction, and the raw one is not — so a band
//! around a plausible CAD/USD catches a missing inversion, which would read
//! ~1.38.
//!
//! **The band's honest bound.** It is *not* stable across all FX history, and
//! it cannot be: CAD traded at and above parity with USD for stretches of
//! 2007–2013 (USD/CAD bottomed near 0.906 in Nov 2007, i.e. CAD/USD ≈ 1.10),
//! while the all-time low is ≈ 0.618 in Jan 2002. The bounds below cover that
//! range, but note the structural limit — any band wide enough to contain
//! parity necessarily overlaps its own reciprocal image, so *at* parity these
//! tests cannot distinguish a direction at all. That is a property of the
//! measurement, not a defect to widen away. If CAD approaches parity again,
//! the discriminating assertion is the reciprocal check in
//! `the_stored_series_is_canonical_end_to_end`, not the band.
//!
//! Needs a network and a credential, so `#[ignore]`d like the fence tests:
//!
//! ```sh
//! op run --env-file=infra/localnet/secrets.local.env -- \
//!   cargo test -p dropset-market-data --test oanda_direction -- --ignored
//! ```

use dropset_feeds::{
    now_secs,
    venues::{oanda, OandaCandles},
    Source,
};
use dropset_market_data::fx::{oanda_instrument, secret};

/// A band around a plausible CAD/USD, wide enough to cover the pair's
/// historical range (≈0.618 in 2002 to ≈1.10 in 2007) while still separating
/// it from its reciprocal ≈1.38. The point is to catch a missing inversion,
/// not to pin a rate — see the module doc for why it cannot be made
/// unconditionally safe.
const CAD_USD_BAND: (f64, f64) = (0.50, 1.20);

/// The practice host, matching the collector's default.
const BASE_URL: &str = "https://api-fxpractice.oanda.com";

/// Four days back, so the first windows cover recent trading days. Call this
/// **once** per test and share the result across fetches — see
/// [`first_non_empty`].
fn recent_start() -> i64 {
    now_secs() - 4 * 86_400
}

/// Drain windows until one carries candles, so a request landing on a closed
/// weekend is not read as a failure. FX is shut from Friday evening to Sunday
/// evening and a window inside that legitimately returns nothing.
/// `start` is passed in rather than computed here, so two calls can be made to
/// cover the **same** window. Recomputing `now_secs()` per call makes the
/// second window start later by however long the first call's round trips
/// took, and `assemble` filters on `bucket_start >= next_start` — so with
/// minute candles the oldest surviving bucket drops out whenever a minute
/// boundary falls inside that delta. Any test comparing the two fetches
/// bucket-for-bucket would then fail a few percent of runs, and would read as
/// a quote-direction failure rather than as a race.
async fn first_non_empty(
    instrument: &str,
    invert: bool,
    start: i64,
) -> Vec<dropset_feeds::venues::Candle> {
    let api_key = secret(oanda::SECRET_NAME).expect("OANDA credential must resolve");
    let http = OandaCandles::client(BASE_URL, &api_key).expect("client builds");
    let mut source = OandaCandles::resume(
        http,
        format!("test:oanda:{instrument}"),
        instrument,
        60,
        5_000,
        None,
        start,
        invert,
    )
    .expect("source builds");

    for _ in 0..5 {
        let batch = source.next().await.expect("a window must not error");
        if !batch.records.is_empty() {
            return batch.records;
        }
        if batch.caught_up {
            break;
        }
    }
    panic!("no window returned candles for {instrument}");
}

#[tokio::test]
#[ignore = "requires a network and the OANDA credential"]
async fn the_reversed_instrument_is_the_one_that_exists() {
    // Half the premise: `USD_CAD` is real. If OANDA ever listed `CAD_USD`, the
    // table entry would become wrong in the harmless direction, but this is
    // where that shows up.
    let records = first_non_empty("USD_CAD", false, recent_start()).await;
    let bar = &records[0];
    assert!(
        bar.low <= bar.high,
        "raw bar must be ordered: low {} high {}",
        bar.low,
        bar.high
    );
    let (lo, hi) = CAD_USD_BAND;
    assert!(
        bar.close > 1.0 / hi && bar.close < 1.0 / lo,
        "raw USD_CAD close {} should sit on the reciprocal side of \
         {CAD_USD_BAND:?} — note the two ranges legitimately overlap once the \
         band is wide enough to contain parity, so this is a sanity check and \
         not a separation proof (see the module doc)",
        bar.close
    );
}

#[tokio::test]
#[ignore = "requires a network and the OANDA credential"]
async fn the_canonical_direction_is_rejected_by_the_venue() {
    // The other half, and the sharper one: this is what makes a wrong table
    // entry a loud failure rather than a plausible wrong number. If OANDA ever
    // starts serving `CAD_USD`, this test fails and the table should lose its
    // entry — the inversion would then be storing the reciprocal.
    let api_key = secret(oanda::SECRET_NAME).expect("OANDA credential must resolve");
    let http = OandaCandles::client(BASE_URL, &api_key).expect("client builds");
    let mut source = OandaCandles::resume(
        http,
        "test:oanda:CAD_USD",
        "CAD_USD",
        60,
        500,
        None,
        now_secs() - 4 * 86_400,
        false,
    )
    .expect("source builds");

    // `Batch` is not `Debug`, so match rather than `expect_err`.
    let msg = match source.next().await {
        Ok(batch) => panic!(
            "v20 must reject the non-market-convention direction, but CAD_USD \
             returned {} candle(s) — if the venue now serves this pair, drop \
             its OANDA_REVERSED_PAIRS entry, because the inversion is then \
             storing the reciprocal",
            batch.records.len()
        ),
        Err(err) => err.to_string(),
    };
    // **Assert on the status token the transport writes, and rule the auth
    // failures out explicitly.** A substring test for `"400"` or
    // `"instrument"` reads as precise and is very nearly vacuous: the request
    // path is `/v3/instruments/{instrument}/candles`, so ANY error carrying
    // the URL contains "instrument" — including a 401 from an expired
    // credential — and the query string carries `from=<epoch>`, where an
    // epoch like 1786668400 contains "400". Either would let a stale key
    // masquerade as the measurement succeeding, in the one test whose job is
    // to catch this table drifting away from the venue.
    //
    // `HttpClient::check_status` interpolates the status as `returned {status}`
    // (pinned by its own test `a_failed_status_is_named_in_the_error_itself`),
    // so `returned 400` is an unambiguous anchor.
    for auth_status in ["returned 401", "returned 403"] {
        assert!(
            !msg.contains(auth_status),
            "the credential looks stale ({auth_status}) — this test cannot \
             say anything about quote direction until it is fixed: {msg}"
        );
    }
    assert!(
        msg.contains("returned 400"),
        "expected v20 to reject CAD_USD with a 400, got: {msg}"
    );
}

#[tokio::test]
#[ignore = "requires a network and the OANDA credential"]
async fn the_stored_series_is_canonical_end_to_end() {
    // The whole change in one assertion: the roster's canonical `CAD-USD`
    // resolves to a reversed instrument, and what the adapter yields for it is
    // already in the canonical direction — so a sink needs no knowledge of any
    // of this.
    let resolved = oanda_instrument("CAD-USD").expect("the venue rule resolves");
    assert_eq!(resolved.symbol, "USD_CAD");
    assert!(resolved.inverted, "CAD-USD must be marked reversed");

    // One start for both fetches below, so they cover the same window and the
    // bucket-for-bucket comparison cannot race a minute boundary.
    let start = recent_start();

    let records = first_non_empty(&resolved.symbol, resolved.inverted, start).await;
    let bar = &records[0];

    let (lo, hi) = CAD_USD_BAND;
    for (field, value) in [
        ("open", bar.open),
        ("high", bar.high),
        ("low", bar.low),
        ("close", bar.close),
    ] {
        assert!(
            value > lo && value < hi,
            "inverted {field} {value} is outside the plausible CAD/USD band \
             {CAD_USD_BAND:?} — a missing inversion would read near 1.38"
        );
    }

    // **The direction-proof half, which needs no magic numbers at all.** The
    // band above is a sanity check with a structural limit (see the module
    // doc); this is the assertion that still discriminates at parity. Fetch
    // the SAME instrument un-inverted and require the two to be exact
    // reciprocals of each other — true regardless of where the market is.
    let raw = first_non_empty(&resolved.symbol, false, start).await;
    let raw_bar = raw
        .iter()
        .find(|c| c.bucket_start == bar.bucket_start)
        .expect("the same window must return the same buckets");
    assert_eq!(bar.close, 1.0 / raw_bar.close, "close is the reciprocal");
    assert_eq!(bar.high, 1.0 / raw_bar.low, "high comes from the raw low");
    assert_eq!(bar.low, 1.0 / raw_bar.high, "low comes from the raw high");
    // Note this is sufficient on its own to prove the inversion HAPPENED, and
    // it stays sufficient at parity: if `invert` were false the two bars would
    // be identical, so `close == 1.0 / close` would force `close == 1.0`
    // exactly. No band, and no assumption about which side of parity the
    // market sits on.

    // The detail a field-wise reciprocal gets wrong, checked against the live
    // response shape rather than a captured one.
    assert!(
        bar.low <= bar.high,
        "inverted bar must stay ordered: low {} high {}",
        bar.low,
        bar.high
    );
    assert!(
        bar.open >= bar.low && bar.open <= bar.high,
        "open within range"
    );
    assert!(
        bar.close >= bar.low && bar.close <= bar.high,
        "close within range"
    );
}
