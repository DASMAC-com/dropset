// cspell:word USDTUSD
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
    pub fn venue_symbol_or_else<F>(&self, derive: F) -> Result<String>
    where
        F: FnOnce(&str) -> Result<String>,
    {
        match &self.venue_symbol {
            Some(pinned) => Ok(pinned.clone()),
            None => derive(&self.product_id),
        }
    }
}

/// One roster entry resolved against a venue: what to ask the venue for, and
/// what to store the answer under.
///
/// **A named pair rather than a tuple, deliberately.** Both fields are
/// `String`, so a positional `(venue, canonical)` swaps silently at any call
/// site that takes it apart — and the consequence of a swap is storing a
/// reading under a venue-native symbol, which is the exact outcome the
/// canonical-id convention exists to prevent (see [`crate::fx`]). Names make
/// that mistake fail to compile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VenueProduct {
    /// The symbol to send to the venue, and the key its response comes back
    /// under.
    pub venue_symbol: String,
    /// The canonical id the reading is stored under.
    pub product_id: String,
}

/// Resolve a whole roster against one venue's spelling rule.
///
/// This is the inverse of the derivation above, and it is a collector concern
/// rather than a venue one: the adapters answer under the keys they were asked
/// with, and only the roster knows which canonical id each of those keys
/// stands for.
///
/// **Rejects two entries that resolve to the same venue symbol.**
/// [`parse_roster`] already rejects a duplicate *canonical* id, but that is not
/// the same check: a pinned spelling can collide with another entry's derived
/// one (`USDC-USD,USDT-USD=USDCUSD`), and a batched venue answers once per
/// symbol. Indexing that by venue symbol would keep one entry and silently
/// file the venue's reading under the surviving pair's canonical id — one
/// pair's price stored as another's, with the loser reported as a roster typo
/// by the silence watch. That is corrupt data rather than missing data, so it
/// fails startup.
pub fn resolve_venue<F>(products: &[RosterEntry], derive: F) -> Result<Vec<VenueProduct>>
where
    F: Fn(&str) -> Result<String>,
{
    let mut out: Vec<VenueProduct> = Vec::with_capacity(products.len());
    for entry in products {
        let venue_symbol = entry.venue_symbol_or_else(&derive)?;
        if let Some(clash) = out.iter().find(|p| p.venue_symbol == venue_symbol) {
            // Worded for both caller shapes. A collector that indexes a batched
            // response by venue symbol would file the venue's single answer
            // under whichever pair survived the collision — corrupt data. One
            // that builds a feed per pair instead would merely poll the same
            // symbol twice, which is not corruption but still wrong: it double-
            // counts against the request budget the quota floor computes from
            // roster size. Both deserve a refusal, so the message claims only
            // what holds for either.
            return Err(anyhow!(
                "{:?} and {:?} both resolve to the venue symbol {venue_symbol:?}; \
                 the venue answers once per symbol, so one reading would have to \
                 stand for two pairs",
                clash.product_id,
                entry.product_id
            ));
        }
        out.push(VenueProduct {
            venue_symbol,
            product_id: entry.product_id.clone(),
        });
    }
    Ok(out)
}

/// The canonical ids of a roster whose venue spelling is not derived at all —
/// a venue that names its products the canonical way already (Coinbase), or one
/// that takes the two legs separately (Alpha Vantage).
///
/// **Rejects a pinned spelling instead of ignoring it.** [`parse_roster`]
/// accepts `CANONICAL=VENUE` from any collector's roster, because the parser
/// does not know which venue will consume it. A collector that derives nothing
/// has nowhere to put that override, and silently dropping it would make the
/// documented pinning feature a promise this collector does not keep — an
/// operator's deliberate edit having no effect and no error, which is the
/// quiet-config failure the whole roster module is written to avoid. So say so.
pub fn canonical_only(products: &[RosterEntry]) -> Result<Vec<String>> {
    let mut out = Vec::with_capacity(products.len());
    for entry in products {
        if let Some(pinned) = &entry.venue_symbol {
            return Err(anyhow!(
                "roster entry {:?} pins the venue symbol {pinned:?}, but this \
                 venue is addressed by its canonical id and derives no \
                 spelling — drop the `={pinned}` rather than leaving an \
                 override that cannot take effect",
                entry.product_id
            ));
        }
        out.push(entry.product_id.clone());
    }
    Ok(out)
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
        // **Normalized and shape-checked, because the stored key is the join
        // key.** A canonical id goes straight into `cex_prices.product_id` /
        // `spot_ticks.product_id`, and every consumer joins on it. Accepting it
        // verbatim meant `eurc-usd` priced perfectly well — the venue side is
        // derived and upper-cased, so Kraken answered, and the silence watch
        // was satisfied because it compares against this same string — while
        // writing a *second* series under a key nothing joins, showing up as a
        // stray entry in the dashboard's product picker. The table-backed roster
        // guards exactly this with a `CHECK (product_id ~ '^[A-Z]{3}-[A-Z]{3}$')`;
        // this is the environment-backed equivalent.
        let product_id = product_id.to_ascii_uppercase();
        let (base, quote) = product_id.split_once('-').ok_or_else(|| {
            anyhow!(
                "roster entry {product_id:?} is not a canonical BASE-QUOTE id \
                 (it has no `-`)"
            )
        })?;
        if base.is_empty() || quote.is_empty() || quote.contains('-') {
            return Err(anyhow!(
                "roster entry {product_id:?} is not a canonical BASE-QUOTE id: \
                 it needs exactly one `-` with a non-empty leg either side"
            ));
        }
        if out.iter().any(|e| e.product_id == product_id) {
            return Err(anyhow!(
                "roster names {product_id:?} more than once; one collector \
                 polls each pair exactly once"
            ));
        }
        out.push(RosterEntry {
            product_id,
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
/// product this predates), then `default`.
///
/// The singular spelling is kept working for a **hand-run or externally
/// configured** process — a binary started directly, or a hosted task whose
/// environment was written against the old name — not for this repo's compose
/// file, which this change updates to `PRODUCT_IDS` everywhere. (An earlier
/// version of this comment claimed the opposite; the compose file no longer
/// sets the singular at all, so nothing here relies on it.) A roster of one is
/// exactly what the singular means, so honoring it costs nothing.
///
/// It is **not** consulted as a supplement — a file that sets both gets the
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
        let derived = roster[0].venue_symbol_or_else(|_| Ok("USDTUSD".to_string()));
        assert_eq!(derived.unwrap(), "USDTZUSD");
    }

    #[test]
    fn an_unpinned_entry_derives_its_symbol() {
        let roster = parse_roster("EURC-USD").unwrap();
        let derived = roster[0].venue_symbol_or_else(|p| Ok(p.replace('-', "")));
        assert_eq!(derived.unwrap(), "EURCUSD");
    }

    #[test]
    fn a_derive_failure_propagates_rather_than_being_swallowed() {
        // A failed derivation must fail startup, not be quietly omitted: the
        // venue adapters omit symbols they got no answer for, so a bad
        // derivation would masquerade as an unquoted pair.
        let roster = parse_roster("AUD-USD").unwrap();
        assert!(roster[0]
            .venue_symbol_or_else(|_| Err(anyhow!("nope")))
            .is_err());
    }

    #[test]
    fn a_canonical_id_is_upper_cased_so_it_cannot_fork_the_stored_series() {
        // The bug this closes: the venue side is derived and upper-cased, so a
        // lower-case entry priced perfectly well at the venue and satisfied the
        // silence watch — while writing a SECOND series under a key no consumer
        // joins on, visible only as a stray entry in the dashboard's product
        // picker.
        let roster = parse_roster("eurc-usd,AuD-uSd").unwrap();
        assert_eq!(roster[0].product_id, "EURC-USD");
        assert_eq!(roster[1].product_id, "AUD-USD");
    }

    #[test]
    fn a_lower_and_upper_spelling_of_one_pair_is_a_duplicate() {
        // Falls out of normalizing before the duplicate check, and is worth
        // pinning: pre-normalization these read as two different products.
        assert!(parse_roster("EURC-USD,eurc-usd").is_err());
    }

    #[test]
    fn an_id_that_is_not_base_quote_shaped_is_rejected() {
        // No separator at all, and more than one — both would otherwise reach a
        // venue as a plausible-looking symbol.
        assert!(parse_roster("EURCUSD").is_err());
        assert!(parse_roster("EUR-C-USD").is_err());
        assert!(parse_roster("-USD").is_err());
        assert!(parse_roster("EURC-").is_err());
    }

    #[test]
    fn a_roster_resolves_to_named_venue_product_pairs() {
        let roster = parse_roster("EURC-USD,USDT-USD=USDTZUSD").unwrap();
        let resolved = resolve_venue(&roster, kraken_pair).unwrap();
        assert_eq!(
            resolved,
            vec![
                VenueProduct {
                    venue_symbol: "EURCUSD".to_string(),
                    product_id: "EURC-USD".to_string(),
                },
                // The pinned spelling is what the venue answers under, but the
                // canonical id is still what gets stored — the whole point of
                // keeping the two separate.
                VenueProduct {
                    venue_symbol: "USDTZUSD".to_string(),
                    product_id: "USDT-USD".to_string(),
                },
            ]
        );
    }

    #[test]
    fn two_entries_resolving_to_one_venue_symbol_fail_startup() {
        // The corruption this prevents: the venue answers once for USDCUSD, and
        // indexing by venue symbol would file that single reading under
        // whichever canonical id survived — one pair's price stored as
        // another's, with the loser blamed on a roster typo by the silence
        // watch.
        let roster = parse_roster("USDC-USD,USDT-USD=USDCUSD").unwrap();
        let err = resolve_venue(&roster, kraken_pair).unwrap_err().to_string();
        assert!(err.contains("both resolve to the venue symbol"), "{err}");
        assert!(err.contains("USDCUSD"), "{err}");
        assert!(err.contains("stand for two pairs"), "{err}");
    }

    #[test]
    fn a_derive_failure_fails_the_whole_resolve() {
        // One entry's derivation failing takes the whole resolve down, rather
        // than yielding a roster quietly short of a pair.
        let roster = parse_roster("AUD-USD,EUR-USD").unwrap();
        assert!(resolve_venue(&roster, |_| Err(anyhow!("no spelling"))).is_err());
    }

    #[test]
    fn a_canonical_only_venue_takes_the_ids_unchanged() {
        let roster = parse_roster("EURC-USDC,USDC-USD").unwrap();
        assert_eq!(
            canonical_only(&roster).unwrap(),
            vec!["EURC-USDC".to_string(), "USDC-USD".to_string()]
        );
    }

    #[test]
    fn a_canonical_only_venue_rejects_a_pin_rather_than_ignoring_it() {
        // The quiet-config failure this closes: the parser accepts a pin from
        // any roster, so a collector that derives nothing would otherwise drop
        // an operator's deliberate override with no error.
        let roster = parse_roster("EURC-USDC=EURCUSD").unwrap();
        let err = canonical_only(&roster).unwrap_err().to_string();
        assert!(err.contains("derives no"), "{err}");
        assert!(err.contains("EURCUSD"), "{err}");
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
