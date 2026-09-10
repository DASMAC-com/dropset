// cspell:word peekable
//! Every collector's default roster exists twice — once as a Rust constant,
//! once as the `${…:-…}` default in `infra/localnet/docker-compose.yml` — and
//! this pins the two together.
//!
//! **Why there are two.** The Rust constant is what the binary polls when
//! `PRODUCT_IDS` is unset: running a collector by hand, or under any deployment
//! that is not this compose file. The compose default is what localnet actually
//! runs with, and it has to be written out there because an operator overrides
//! it per service (`ERAPI_PRODUCT_IDS`, `KRAKEN_PRODUCT_IDS`, …) without
//! rebuilding. Neither can be derived from the other: compose cannot read a
//! Rust constant, and the binary cannot read a compose file it is not deployed
//! with.
//!
//! **Why that needs a test.** Divergence is silent in both directions and looks
//! like the venue rather than like config. A pair added to compose and not to
//! the constant is simply absent for anyone running the binary directly — no
//! error, just a series that never appears. A pair added to the constant and
//! not to compose is absent from localnet, where it reads exactly like a
//! currency the provider does not carry, which is the failure the collectors'
//! own silence watches are written to make visible and cannot here.
//!
//! **One mode: every service must match compose exactly.** All eight carry
//! their own `DEFAULT_PRODUCTS`, in the binary that polls the roster, so the
//! property is equality for all of them.
//!
//! **This file used to have a second, much weaker mode**, and what it cost is
//! worth recording. The three keyed FX venues shared one
//! `fx::DEFAULT_PRODUCTS`, narrowed to the single pair every one of them
//! quotes, so they could only be pinned by **containment** — which for a
//! one-pair constant reduces to "the compose roster contains `AUD-USD`", and
//! left those three compose rosters effectively unpinned. Measured: deleting
//! `EUR-USD` from oanda's compose default left the suite green — a silent
//! de-roster of an MVP anchor pair. Giving each venue its own constant (see
//! `fx::FxDefaults::default_products`) is what turned that into an exact
//! match, and it is why containment is gone rather than merely tightened.
//!
//! **Why it compares text.** Every constant here lives in a binary crate or is
//! private to its module, so none can be imported; and compose is YAML with no
//! parser in this crate's dependency tree. Both sides are read as source, which
//! has the incidental benefit of failing if either file's shape drifts far
//! enough that this test can no longer find the roster — rather than silently
//! comparing two empty sets. `the_extractors_actually_found_every_roster`
//! below is what makes that explicit.

use std::collections::{BTreeMap, BTreeSet};

const COMPOSE: &str = include_str!("../../infra/localnet/docker-compose.yml");

/// One compose service, and where its Rust-side default is written.
struct Wiring {
    /// The service key in `docker-compose.yml`.
    service: &'static str,
    /// That service's binary's source, for the error message.
    rust_source: &'static str,
    /// The whole source text of the file that owns the constant, pulled in with
    /// `include_str!`. The constant's own value is extracted from it later by
    /// [`rust_default`] — this is the haystack, not the needle.
    rust_source_text: &'static str,
}

/// Every compose service that takes a roster, and the constant behind it.
///
/// **A new collector has to be added here**, which is the intended friction:
/// `every_rostered_service_is_pinned` fails until it is, so a service cannot
/// join the file and quietly escape the check — the way it would if this test
/// only iterated over the rows it already knew.
fn wirings() -> Vec<Wiring> {
    vec![
        Wiring {
            service: "alphavantage",
            rust_source: "market-data/src/bin/alphavantage.rs",
            rust_source_text: include_str!("../src/bin/alphavantage.rs"),
        },
        Wiring {
            service: "coinbase",
            rust_source: "market-data/src/config.rs",
            rust_source_text: include_str!("../src/config.rs"),
        },
        Wiring {
            service: "coinbase-ticker",
            rust_source: "market-data/src/bin/coinbase_ticker.rs",
            rust_source_text: include_str!("../src/bin/coinbase_ticker.rs"),
        },
        Wiring {
            service: "erapi",
            rust_source: "market-data/src/bin/erapi.rs",
            rust_source_text: include_str!("../src/bin/erapi.rs"),
        },
        Wiring {
            service: "frankfurter",
            rust_source: "market-data/src/bin/frankfurter.rs",
            rust_source_text: include_str!("../src/bin/frankfurter.rs"),
        },
        Wiring {
            service: "kraken",
            rust_source: "market-data/src/bin/kraken.rs",
            rust_source_text: include_str!("../src/bin/kraken.rs"),
        },
        Wiring {
            service: "oanda",
            rust_source: "market-data/src/bin/oanda.rs",
            rust_source_text: include_str!("../src/bin/oanda.rs"),
        },
        Wiring {
            service: "twelvedata",
            rust_source: "market-data/src/bin/twelvedata.rs",
            rust_source_text: include_str!("../src/bin/twelvedata.rs"),
        },
    ]
}

/// Split a `AUD-USD,EUR-USD` roster spec into its normalized entries: trimming
/// the whitespace a YAML fold leaves behind, skipping blank entries, and
/// **upper-casing**.
///
/// The upper-casing is not cosmetic. `parse_roster` normalizes every id before
/// a collector sees it, so `eur-usd` in compose is `EUR-USD` at runtime — which
/// means without it this file would report a false divergence on a pure case
/// difference, and the canonical-id guard below would reject a spelling that
/// works perfectly in production. Matching the runtime normalization is what
/// keeps both from being latent.
///
/// **This is deliberately NOT a re-implementation of `parse_roster`**, and the
/// difference is load-bearing in one direction: that function *rejects* a
/// duplicate canonical id, whereas a `BTreeSet` silently collapses one. A
/// compose default carrying the same pair twice would therefore crash every
/// collector at startup while a set-only comparison stayed green — exactly the
/// silent divergence this file exists to close. [`entry_count`] is what covers
/// it. (`parse_roster` also accepts the `CANONICAL=VENUE` pin form, which is
/// not re-implemented either — but that one fails loudly through the
/// canonical-id guard, since `=` is not an upper-case letter.)
fn pairs(spec: &str) -> BTreeSet<String> {
    entries(spec).map(str::to_ascii_uppercase).collect()
}

/// The spec's entries before deduping — the input [`pairs`] collapses.
fn entries(spec: &str) -> impl Iterator<Item = &str> {
    spec.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
}

/// How many entries a spec names, counting a repeat twice.
fn entry_count(spec: &str) -> usize {
    entries(spec).count()
}

/// The value of a `const DEFAULT_PRODUCTS: &str = "…";` in Rust source.
///
/// Handles the backslash-newline continuation the longer rosters are written
/// with, which the compiler strips along with the next line's indentation — so
/// this has to strip it the same way or a folded constant reads as carrying a
/// pair called `\n                                JPY-USD`.
fn rust_default(source: &str) -> Option<String> {
    let start = source.find("const DEFAULT_PRODUCTS: &str =")?;
    let rest = &source[start..];
    let end = rest.find(';')?;
    let literal = &rest[..end];
    let open = literal.find('"')?;
    let close = literal.rfind('"')?;
    if close <= open {
        return None;
    }
    let literal = &literal[open + 1..close];
    let mut out = String::with_capacity(literal.len());
    for (i, fragment) in literal.split('\\').enumerate() {
        if i == 0 {
            out.push_str(fragment);
        } else {
            // Everything after a continuation backslash up to the first
            // non-whitespace byte is what the compiler drops.
            out.push_str(fragment.trim_start());
        }
    }
    Some(out)
}

/// Every compose service that defines a `PRODUCT_IDS` default, mapped to that
/// default's roster spec.
///
/// A hand-rolled scan rather than a YAML parse: this crate has no YAML
/// dependency, and the two shapes in the file are narrow enough to read
/// directly — an inline `'${VAR:-…}'` and a folded `>-` block whose
/// continuation lines are joined with a space, exactly as the folded scalar
/// resolves.
fn compose_defaults() -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut service: Option<String> = None;
    let mut lines = COMPOSE.lines().peekable();
    while let Some(line) = lines.next() {
        // A service key: exactly two spaces of indent, then `name:`.
        if let Some(name) = line.strip_prefix("  ") {
            if !name.starts_with(' ') && !name.starts_with('#') {
                if let Some(name) = name.strip_suffix(':') {
                    service = Some(name.to_string());
                }
            }
        }
        // Matched at its EXACT indent — six spaces, under `environment:` —
        // rather than at any depth. A depth-insensitive match would pair a key
        // at some other depth with the continuation rule below, which keys on
        // eight spaces, and silently yield an empty roster. Anchoring at column
        // zero also excludes the two INNER `${PRODUCT_IDS:-…}` references (the
        // coinbase pair), so the ten textual occurrences reduce to the eight
        // keys, all of which sit at six spaces today.
        //
        // The two rules are still only pinned to today's file, not derived
        // from each other: YAML would accept a seven-space continuation, which
        // this loop would treat as the end of the block. Such a truncation is
        // now loud for every service — it drops pairs from the compose side of
        // an exact comparison — where under the old containment mode it was
        // silent for the three FX venues.
        let Some(value) = line.strip_prefix("      PRODUCT_IDS:") else {
            continue;
        };
        let Some(service) = service.clone() else {
            continue;
        };
        let mut value = value.trim().to_string();
        if value == ">-" || value == ">" {
            // A folded scalar: every following line indented past the key
            // belongs to it, joined with a space.
            value = String::new();
            while let Some(next) = lines.peek() {
                if next.trim().is_empty() || !next.starts_with("        ") {
                    break;
                }
                if !value.is_empty() {
                    value.push(' ');
                }
                value.push_str(lines.next().unwrap().trim());
            }
        }
        out.insert(service, unwrap_default(&value));
    }
    out
}

/// The innermost `${VAR:-DEFAULT}` default out of a compose value.
///
/// Keyed off the **last** `:-` rather than the first, which is what reads the
/// nested `${ALPHAVANTAGE_PRODUCT_IDS:-${FX_PRODUCT_IDS:-…}}` form correctly:
/// the innermost default is the roster that applies when no override is set at
/// all, and taking the outer one would yield a literal `${FX_PRODUCT_IDS:-…}`.
fn unwrap_default(value: &str) -> String {
    let value = value.trim().trim_matches('\'').trim_matches('"');
    match value.rfind(":-") {
        Some(at) => value[at + 2..]
            .trim_end_matches(['}', '\'', '"'])
            .to_string(),
        None => value.to_string(),
    }
}

/// The point of the whole file: no collector's two rosters may drift apart.
#[test]
fn every_rust_default_agrees_with_its_compose_default() {
    let compose = compose_defaults();
    for wiring in wirings() {
        let rust = pairs(&rust_default(wiring.rust_source_text).unwrap_or_else(|| {
            panic!(
                "no DEFAULT_PRODUCTS constant found in {} — the extractor has \
                 stopped matching its shape",
                wiring.rust_source
            )
        }));
        let composed = pairs(compose.get(wiring.service).unwrap_or_else(|| {
            panic!(
                "docker-compose.yml defines no PRODUCT_IDS default for service \
                 `{}`",
                wiring.service
            )
        }));
        assert_eq!(
            rust,
            composed,
            "service `{}`: the roster in {} and the compose default have \
             diverged. Only in Rust: {:?}; only in compose: {:?}",
            wiring.service,
            wiring.rust_source,
            rust.difference(&composed).collect::<Vec<_>>(),
            composed.difference(&rust).collect::<Vec<_>>(),
        );
    }
}

/// Every rostered service in compose is one this file pins.
///
/// The guard that keeps a new collector from escaping the check: without it,
/// adding a service to compose and to a new binary passes here forever, because
/// the loop above only visits rows someone remembered to add.
#[test]
fn every_rostered_service_is_pinned() {
    let pinned: BTreeSet<&str> = wirings().into_iter().map(|w| w.service).collect();
    let unpinned: Vec<String> = compose_defaults()
        .into_keys()
        .filter(|service| !pinned.contains(service.as_str()))
        .collect();
    assert!(
        unpinned.is_empty(),
        "these compose services take a PRODUCT_IDS roster but are not pinned \
         against a Rust default: {unpinned:?} — add each to `wirings()`",
    );
}

/// Guard against the comparison above passing vacuously — if either extractor
/// stops matching its file's shape, this is what fails instead.
#[test]
fn the_extractors_actually_found_every_roster() {
    let compose = compose_defaults();
    // Eight rostered services today. A floor rather than an equality so adding
    // a collector does not fail this — `every_rostered_service_is_pinned` is
    // what covers that direction — while a scan that stops matching the file's
    // shape still does.
    assert!(
        compose.len() >= 8,
        "found PRODUCT_IDS defaults for only {} compose services; the scan has \
         probably stopped matching the file's shape: {:?}",
        compose.len(),
        compose.keys().collect::<Vec<_>>(),
    );
    for wiring in wirings() {
        let rust = rust_default(wiring.rust_source_text)
            .unwrap_or_else(|| panic!("no DEFAULT_PRODUCTS in {}", wiring.rust_source));
        assert!(
            !pairs(&rust).is_empty(),
            "the roster extracted from {} is empty, so the check above would \
             pass for the wrong reason",
            wiring.rust_source,
        );
        let composed = &compose[wiring.service];
        assert!(
            !pairs(composed).is_empty(),
            "the compose roster for `{}` is empty, so the check above would \
             pass for the wrong reason",
            wiring.service,
        );
        // Neither side may name a pair twice. This is the one way `pairs`'s
        // set semantics diverge from `parse_roster` in the DANGEROUS
        // direction: the parser rejects a duplicate canonical id and refuses
        // to start, while a set just collapses it — so without this a repeated
        // pair in compose would crash every collector at startup with this
        // suite green.
        for (side, spec) in [("rust", rust.as_str()), ("compose", composed.as_str())] {
            assert_eq!(
                entry_count(spec),
                pairs(spec).len(),
                "the {side} roster for `{}` names a pair twice, which \
                 `parse_roster` rejects at startup: {spec:?}",
                wiring.service,
            );
        }
        // Every entry on both sides is a canonical `BASE-QUOTE` id. This is
        // what catches a fold that swallowed a separator: `EUR-USD GBP-USD`
        // parses as one entry and would otherwise just look like a missing pair.
        for entry in pairs(&rust).iter().chain(pairs(composed).iter()) {
            assert!(
                entry.split('-').count() == 2
                    && entry
                        .split('-')
                        .all(|leg| leg.len() >= 3 && leg.chars().all(|c| c.is_ascii_uppercase())),
                "`{entry}` in service `{}` is not a canonical BASE-QUOTE id",
                wiring.service,
            );
        }
    }
}
