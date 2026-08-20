//! The product roster a collector polls: parsing it out of the environment,
//! and the canonical ↔ venue spelling it hands each adapter.
//!
//! **One collector process serves a venue, not a pair.** It used to serve
//! exactly one product — `PRODUCT_ID` was singular and the compose file ran a
//! separate service per pair — which meant N pairs cost N processes, N pools,
//! and N images to schedule for what is often a single batched request. A
//! roster replaces that: one service per venue, every pair it covers listed
//! here.
//!
//! **The canonical id is the stored one, and the venue's spelling is derived
//! from it** — the same rule the FX collectors already follow (see [`crate::fx`]
//! for why: storing venue-native symbols would put one pair under several keys
//! and make the cross-source comparison these feeds exist for impossible).
//! Most venues' spellings *are* derivable (`AUD-USD` → OANDA `AUD_USD`, Twelve
//! Data `AUD/USD`, Kraken `AUDUSD`), so the roster carries only the canonical
//! id. The exception is a venue that names a pair in a way no rule reproduces —
//! Kraken keeps the legacy `X`/`Z` prefixes for some assets, so `USDT-USD` is
//! `USDTZUSD` there and nothing derives that — and for those an entry may pin
//! the venue's spelling explicitly with `CANONICAL=VENUE_SYMBOL`.
//!
//! Pinning is deliberately per-entry rather than a second environment variable:
//! a roster and its overrides drift apart the moment they live in two places.
//!
//! Where a venue's spelling rule lives: the three FX vendors' rules sit in
//! [`crate::fx`], beside the credential handling and backfill configuration
//! those collectors share. The rules here are for venues with neither — a
//! keyless ticker whose only venue-specific concern *is* its spelling.

use anyhow::{anyhow, Result};

/// One roster entry: the canonical product this collector stores, and the
/// venue's own spelling of it when that cannot be derived.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RosterEntry {
    /// The canonical `BASE-QUOTE` id, e.g. `AUD-USD` or `EURC-USDC`. This is
    /// what lands in `cex_prices.product_id` / `spot_ticks.product_id`.
    pub product_id: String,
    /// The venue's spelling, when the entry pinned one. `None` means the
    /// collector derives it — the normal case.
    pub venue_symbol: Option<String>,
}

impl RosterEntry {
    /// The venue's spelling: the pinned one if the entry carried it, else
    /// whatever `derive` makes of the canonical id.
    ///
    /// Taking a closure rather than a rule keeps this type ignorant of any
    /// particular venue: each collector passes its own mapping and the
    /// override precedence lives in one place instead of once per venue.
    pub fn venue_symbol_or<F>(&self, derive: F) -> Result<String>
    where
        F: FnOnce(&str) -> Result<String>,
    {
        match &self.venue_symbol {
            Some(pinned) => Ok(pinned.clone()),
            None => derive(&self.product_id),
        }
    }
}

/// Parse a comma-separated roster: `AUD-USD,EUR-USD` or, where a venue's
/// spelling has to be pinned, `USDT-USD=USDTZUSD`.
///
/// Blank entries are skipped rather than rejected, so a trailing comma or a
/// YAML-folded list with a stray separator is not a startup failure. A
/// **duplicate** canonical id *is* rejected: two entries for one pair would
/// have the collector poll it twice and race two cursors under one feed name,
/// and silently keeping the last would hide a compose-file mistake.
pub fn parse_roster(spec: &str) -> Result<Vec<RosterEntry>> {
    let mut out: Vec<RosterEntry> = Vec::new();
    for raw in spec.split(',') {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let (product_id, venue_symbol) = match raw.split_once('=') {
            Some((canonical, venue)) => {
                let canonical = canonical.trim();
                let venue = venue.trim();
                if venue.is_empty() {
                    return Err(anyhow!(
                        "roster entry {raw:?} pins an empty venue symbol; drop \
                         the `=` to derive it"
                    ));
                }
                (canonical, Some(venue.to_string()))
            }
            None => (raw, None),
        };
        if product_id.is_empty() {
            return Err(anyhow!("roster entry {raw:?} has no canonical product id"));
        }
        if out.iter().any(|e| e.product_id == product_id) {
            return Err(anyhow!(
                "roster names {product_id:?} more than once; one collector \
                 polls each pair exactly once"
            ));
        }
        out.push(RosterEntry {
            product_id: product_id.to_string(),
            venue_symbol,
        });
    }
    if out.is_empty() {
        return Err(anyhow!(
            "the roster is empty; set a comma-separated list of canonical \
             BASE-QUOTE product ids"
        ));
    }
    Ok(out)
}

/// Read a collector's roster from the environment.
///
/// Precedence is `PRODUCT_IDS` (the roster), then `PRODUCT_ID` (the single
/// product this predates), then `default`. The singular spelling is kept
/// working on purpose: it is what every deployed compose service and every
/// operator note still says, and a roster of one is exactly what it means. It
/// is **not** consulted as a supplement — a file that sets both gets the
/// roster, because merging them would make the effective set depend on parse
/// order rather than on what the operator wrote.
pub fn roster_from_env(default: &str) -> Result<Vec<RosterEntry>> {
    let spec = std::env::var("PRODUCT_IDS")
        .or_else(|_| std::env::var("PRODUCT_ID"))
        .unwrap_or_else(|_| default.to_string());
    parse_roster(&spec)
}

/// `EURC-USD` → `EURCUSD`, Kraken's usual pair spelling.
///
/// Kraken names most pairs as the two assets concatenated, which this
/// reproduces — but **not all of them**: legacy assets keep the `X`/`Z`
/// prefixes (`USDT/USD` is `USDTZUSD`), and nothing derives that. Such a pair
/// pins its spelling in the roster entry instead. Getting it wrong is quiet
/// rather than loud, because the adapter omits a pair it got no answer for, so
/// the collector's silence watch is what surfaces the mistake.
pub fn kraken_pair(product_id: &str) -> Result<String> {
    let (base, quote) = product_id
        .split_once('-')
        .ok_or_else(|| anyhow!("{product_id:?} is not a canonical BASE-QUOTE symbol"))?;
    if base.is_empty() || quote.is_empty() {
        return Err(anyhow!(
            "{product_id:?} is not a canonical BASE-QUOTE symbol: both legs must \
             be non-empty"
        ));
    }
    Ok(format!("{base}{quote}").to_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kraken_pairs_concatenate_the_two_legs() {
        assert_eq!(kraken_pair("EURC-USD").unwrap(), "EURCUSD");
        assert_eq!(kraken_pair("USDC-USD").unwrap(), "USDCUSD");
        // Case is normalized, since Kraken answers under an uppercase name.
        assert_eq!(kraken_pair("eurc-usd").unwrap(), "EURCUSD");
    }

    #[test]
    fn a_malformed_pair_is_rejected_rather_than_concatenated_into_nonsense() {
        assert!(kraken_pair("EURCUSD").is_err());
        assert!(kraken_pair("-USD").is_err());
        assert!(kraken_pair("EURC-").is_err());
    }

    #[test]
    fn parses_a_plain_comma_separated_roster() {
        let roster = parse_roster("AUD-USD,EUR-USD").unwrap();
        assert_eq!(roster.len(), 2);
        assert_eq!(roster[0].product_id, "AUD-USD");
        assert_eq!(roster[0].venue_symbol, None);
        assert_eq!(roster[1].product_id, "EUR-USD");
    }

    #[test]
    fn tolerates_whitespace_and_a_trailing_separator() {
        // The shape a YAML-folded or hand-edited list actually arrives in.
        let roster = parse_roster(" AUD-USD , EUR-USD ,").unwrap();
        assert_eq!(roster.len(), 2);
        assert_eq!(roster[1].product_id, "EUR-USD");
    }

    #[test]
    fn a_single_product_is_a_roster_of_one() {
        // The back-compat path: every deployed service still says PRODUCT_ID.
        let roster = parse_roster("EURC-USDC").unwrap();
        assert_eq!(roster.len(), 1);
        assert_eq!(roster[0].product_id, "EURC-USDC");
    }

    #[test]
    fn pins_a_venue_spelling_no_rule_would_derive() {
        // The motivating case: Kraken keeps the legacy Z prefix on this pair,
        // and hyphen-stripping produces USDTUSD, which the venue does not
        // quote.
        let roster = parse_roster("EURC-USD,USDT-USD=USDTZUSD").unwrap();
        assert_eq!(roster[0].venue_symbol, None);
        assert_eq!(roster[1].product_id, "USDT-USD");
        assert_eq!(roster[1].venue_symbol.as_deref(), Some("USDTZUSD"));
    }

    #[test]
    fn a_pinned_symbol_wins_over_the_derived_one() {
        let roster = parse_roster("USDT-USD=USDTZUSD").unwrap();
        let derived = roster[0].venue_symbol_or(|_| Ok("USDTUSD".to_string()));
        assert_eq!(derived.unwrap(), "USDTZUSD");
    }

    #[test]
    fn an_unpinned_entry_derives_its_symbol() {
        let roster = parse_roster("EURC-USD").unwrap();
        let derived = roster[0].venue_symbol_or(|p| Ok(p.replace('-', "")));
        assert_eq!(derived.unwrap(), "EURCUSD");
    }

    #[test]
    fn a_derive_failure_propagates_rather_than_being_swallowed() {
        // A malformed canonical id must fail startup, not be quietly omitted:
        // the venue adapters omit symbols they got no answer for, so a bad
        // derivation would masquerade as an unquoted pair.
        let roster = parse_roster("not-a-pair").unwrap();
        assert!(roster[0]
            .venue_symbol_or(|_| Err(anyhow!("nope")))
            .is_err());
    }

    #[test]
    fn rejects_a_duplicate_pair() {
        // Two entries for one pair would race two cursors under a single feed
        // name; keeping the last would hide the compose-file mistake.
        let err = parse_roster("AUD-USD,AUD-USD").unwrap_err().to_string();
        assert!(err.contains("more than once"), "{err}");
    }

    #[test]
    fn rejects_an_empty_roster() {
        assert!(parse_roster("").is_err());
        assert!(parse_roster(" , , ").is_err());
    }

    #[test]
    fn rejects_a_half_written_override() {
        // `PAIR=` reads as "pin the venue symbol" with nothing pinned, which
        // is a typo rather than a request to derive.
        assert!(parse_roster("USDT-USD=").is_err());
        assert!(parse_roster("=USDTZUSD").is_err());
    }
}
