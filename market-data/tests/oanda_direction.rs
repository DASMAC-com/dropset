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
//! magnitude* for the canonical direction, and the raw one is not — so a wide
//! band around a plausible CAD/USD is both sufficient to catch a missing
//! inversion (which would read ~1.38, far outside it) and stable across
//! years of ordinary FX drift.
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

/// A generously wide band around a plausible CAD/USD. The point is to separate
/// ~0.73 from its reciprocal ~1.38, not to pin a rate.
const CAD_USD_BAND: (f64, f64) = (0.55, 0.95);

/// The practice host, matching the collector's default.
const BASE_URL: &str = "https://api-fxpractice.oanda.com";

/// Drain windows until one carries candles, so a request landing on a closed
/// weekend is not read as a failure. FX is shut from Friday evening to Sunday
/// evening and a window inside that legitimately returns nothing.
async fn first_non_empty(instrument: &str, invert: bool) -> Vec<dropset_feeds::venues::Candle> {
    let api_key = secret(oanda::SECRET_NAME).expect("OANDA credential must resolve");
    let http = OandaCandles::client(BASE_URL, &api_key).expect("client builds");
    // Four days back, so the first windows cover recent trading days.
    let start = now_secs() - 4 * 86_400;
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
    let records = first_non_empty("USD_CAD", false).await;
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
        "raw USD_CAD close {} should sit near the reciprocal of a CAD/USD, \
         outside {CAD_USD_BAND:?}",
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
    assert!(
        msg.contains("400") || msg.to_ascii_lowercase().contains("instrument"),
        "expected a 400 naming the instrument, got: {msg}"
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

    let records = first_non_empty(&resolved.symbol, resolved.inverted).await;
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
