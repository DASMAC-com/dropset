//! The estimator's roster and the maker bot's must agree about **which markets
//! have an observed basis** — and about the currency and static peg they
//! compose from.
//!
//! # Why two rosters exist at all
//!
//! They are not a duplicate to be deleted. The dependency runs one way — the
//! maker bot depends on `dropset-market-data` for [`fx_store`], so this crate
//! cannot depend on the maker — and the two rosters answer different questions:
//! the maker's `MARKETS` carries mint keypairs, decimals and quoting parameters,
//! while `MVP_MARKETS` carries the store series each leg reads.
//!
//! [`fx_store`]: dropset_market_data::fx_store
//!
//! # What must nonetheless agree, and why a divergence is silent
//!
//! Three fields overlap, and each has a failure that no other check catches:
//!
//! * **The basis pin.** The engine drops a pinned market's crypto candidate set
//!   unconditionally. So if the estimator pinned a market the maker composes an
//!   observed basis for, the two would publish *different fair values for the
//!   same pair* — the estimator's off the FX anchor alone, the maker's off the
//!   anchor corrected by a real basis — with both processes healthy, both logging
//!   normally, and nothing comparing them. That is the operator's re-scope
//!   ruling, recorded in the maker's roster comment, so the maker is the
//!   authority and this test reads it as one.
//! * **The tracked currency.** It decides the FX series each side reads. A
//!   divergence would price one pair off another country's cross, which is a
//!   plausible-looking number rather than an error.
//! * **The static peg.** The last-resort mid when every live leg is down. Two
//!   values means the deepest degraded case quotes one price and publishes
//!   another.
//!
//! # Why the maker's roster is PARSED rather than imported
//!
//! Because it cannot be imported: adding `dropset-maker-bot` as a dev-dependency
//! here would make the dependency graph circular. The same technique
//! `roster_compose_agreement.rs` uses for the compose file, for the same reason —
//! a constant in another tree that has to agree with one here, reachable by no
//! type system.
//!
//! The parse is therefore load-bearing and could go vacuous, so
//! `the_parse_found_the_makers_roster` asserts it found what it expected before
//! any comparison is trusted.

use std::collections::BTreeMap;

use dropset_market_data::estimator::MVP_MARKETS;

/// The maker bot's roster source. Not a compiled dependency — see the module
/// docs for why this is `include_str!` rather than a `use`.
const MAKER_CONFIG: &str = include_str!("../../bots/maker-bot/src/config.rs");

/// One market as the maker declares it, for the three fields that must agree.
#[derive(Debug, PartialEq)]
struct MakerMarket {
    currency: String,
    pinned_basis: Option<f64>,
    static_usd: f64,
}

/// The value of a `<field>: <value>,` line inside one `MarketConfig { … }` block.
///
/// Takes the field name with its colon so `static_usd` cannot match inside
/// another field's name, and stops at the line end rather than at the next
/// comma — a value is one token here, and anchoring on the newline means a
/// trailing comment could not be mistaken for part of it.
fn field<'a>(block: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("{name}: ");
    let start = block.find(&needle)? + needle.len();
    let rest = &block[start..];
    let end = rest.find('\n')?;
    Some(rest[..end].trim().trim_end_matches(','))
}

/// A `"EUR"` literal's contents.
fn string_field(block: &str, name: &str) -> Option<String> {
    let raw = field(block, name)?;
    Some(raw.trim_matches('"').to_string())
}

/// A `None` / `Some(1.0)` literal as an `Option<f64>`.
///
/// Returns `Err` on anything else rather than silently reading it as `None`,
/// which would make a mis-parsed pin look like a market with an observed basis —
/// the exact direction this file exists to catch.
fn option_f64(block: &str, name: &str) -> Result<Option<f64>, String> {
    let raw = field(block, name).ok_or_else(|| format!("no `{name}` in the block"))?;
    if raw == "None" {
        return Ok(None);
    }
    let inner = raw
        .strip_prefix("Some(")
        .and_then(|r| r.strip_suffix(')'))
        .ok_or_else(|| format!("`{name}` is neither None nor Some(_): {raw}"))?;
    inner
        .parse::<f64>()
        .map(Some)
        .map_err(|e| format!("`{name}` value {inner} is not a float: {e}"))
}

/// The maker's roster, keyed by symbol.
fn maker_markets() -> BTreeMap<String, MakerMarket> {
    let markets = MAKER_CONFIG
        .split_once("pub const MARKETS:")
        .expect("the maker declares `pub const MARKETS`")
        .1;
    let mut out = BTreeMap::new();
    // Skip the segment before the first `MarketConfig {`, which is the type
    // annotation rather than an entry.
    for block in markets.split("MarketConfig {").skip(1) {
        let symbol = string_field(block, "symbol").expect("every entry names a symbol");
        let currency = string_field(block, "currency").expect("every entry names a currency");
        let pinned_basis =
            option_f64(block, "pinned_basis").unwrap_or_else(|e| panic!("{symbol}: {e}"));
        let static_usd = field(block, "static_usd")
            .expect("every entry names a static peg")
            .parse::<f64>()
            .unwrap_or_else(|e| panic!("{symbol}: static_usd is not a float: {e}"));
        out.insert(
            symbol,
            MakerMarket {
                currency,
                pinned_basis,
                static_usd,
            },
        );
    }
    out
}

/// The base symbol of a canonical `BASE-QUOTE` product id.
fn base_symbol(product_id: &str) -> &str {
    product_id
        .split_once('-')
        .unwrap_or_else(|| panic!("{product_id} is not BASE-QUOTE"))
        .0
}

/// Guard against every comparison below passing vacuously.
///
/// If the maker's roster is reshaped — renamed fields, a different literal
/// style, a move to another file — the parse returns nothing and every
/// comparison holds over an empty set. This is what fails instead.
#[test]
fn the_parse_found_the_makers_roster() {
    let makers = maker_markets();
    assert!(
        makers.len() >= 9,
        "parsed only {} maker markets, so the extractor has probably stopped \
         matching the file's shape: {:?}",
        makers.len(),
        makers.keys().collect::<Vec<_>>(),
    );
    // A floor plus a spot check on both pin directions, so a parse that found
    // the blocks but misread `pinned_basis` cannot pass either.
    assert_eq!(
        makers.get("EURC").expect("EURC is rostered").pinned_basis,
        None,
        "EURC composes an observed basis in the maker's roster"
    );
    assert_eq!(
        makers.get("AUDD").expect("AUDD is rostered").pinned_basis,
        Some(1.0),
        "AUDD is pinned in the maker's roster"
    );
}

/// Every estimator market appears in the maker's roster and agrees on all three
/// overlapping fields.
///
/// The estimator's roster is deliberately a **subset** — it publishes the three
/// MVP pairs, while the maker quotes nine — so this iterates the estimator's and
/// looks each up, rather than comparing the two sets for equality. A market the
/// maker quotes and the estimator does not publish is the expected state today.
#[test]
fn every_estimator_market_agrees_with_the_maker() {
    let makers = maker_markets();
    for market in MVP_MARKETS {
        let symbol = base_symbol(market.product_id);
        let maker = makers.get(symbol).unwrap_or_else(|| {
            panic!(
                "{} publishes {symbol} but the maker bot does not roster it; one \
                 of the two rosters is wrong",
                market.product_id
            )
        });

        assert_eq!(
            market.pinned_basis, maker.pinned_basis,
            "{symbol}: the estimator and the maker disagree about whether the \
             basis is observed, so the two would publish different fair values \
             for one pair with both processes healthy"
        );
        assert_eq!(
            market.currency, maker.currency,
            "{symbol}: the two rosters read different FX crosses"
        );
        assert_eq!(
            market.static_usd, maker.static_usd,
            "{symbol}: the two rosters would quote different last-resort mids"
        );
    }
}

/// A market with a pinned basis must not also name a crypto series, and one with
/// an observed basis must.
///
/// Asserted here as well as in the estimator's own unit tests because this is
/// the field the cross-roster check above compares: if the local invariant broke,
/// the comparison would still hold while the leg mapping had gone incoherent.
#[test]
fn the_pin_and_the_crypto_series_stay_consistent() {
    for market in MVP_MARKETS {
        assert_eq!(
            market.pinned_basis.is_some(),
            market.crypto_product.is_none(),
            "{}: a pinned market must name no crypto series, and an observed one \
             must name exactly one",
            market.product_id
        );
    }
}
