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
//! **Two modes, because two kinds of collector share this file.** Five services
//! carry their own constant and must match compose exactly. The three FX
//! venues share `fx::DEFAULT_PRODUCTS`, one pair wide, because they do not all
//! quote the same instruments — so for those the property is **containment**:
//! whatever a lone binary falls back to must be something its compose roster
//! also carries, or the two disagree about what the service even collects.
//!
//! **Be honest about what containment buys, which is little.**
//! `fx::DEFAULT_PRODUCTS` is the single pair `AUD-USD`, so for oanda,
//! twelvedata and alphavantage the check reduces to "the compose roster
//! contains `AUD-USD`". Adding a pair to one of those three compose defaults,
//! or removing any pair but `AUD-USD` from one, is **not** caught here. Their
//! compose rosters are effectively unpinned, and that is a consequence of the
//! shared one-pair fallback rather than a property this file establishes —
//! recorded so a later reader does not credit it with more coverage than it
//! has. The five `Exact` services are where the real pinning is.
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

/// How a service's Rust default relates to its compose default.
#[derive(Clone, Copy)]
enum Mode {
    /// The two name the same pairs. The service owns its constant.
    Exact,
    /// The Rust default is a subset of the compose default. The service shares
    /// the FX venues' one-pair fallback — see `fx::DEFAULT_PRODUCTS`.
    SubsetOfCompose,
}

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
    mode: Mode,
}

/// Every compose service that takes a roster, and the constant behind it.
///
/// **A new collector has to be added here**, which is the intended friction:
/// `every_rostered_service_is_pinned` fails until it is, so a service cannot
/// join the file and quietly escape the check — the way it would if this test
/// only iterated over the rows it already knew.
fn wirings() -> Vec<Wiring> {
    let fx = include_str!("../src/fx.rs");
    vec![
        Wiring {
            service: "alphavantage",
            rust_source: "market-data/src/fx.rs",
            rust_source_text: fx,
            mode: Mode::SubsetOfCompose,
        },
        Wiring {
            service: "coinbase",
            rust_source: "market-data/src/config.rs",
            rust_source_text: include_str!("../src/config.rs"),
            mode: Mode::Exact,
        },
        Wiring {
            service: "coinbase-ticker",
            rust_source: "market-data/src/bin/coinbase_ticker.rs",
            rust_source_text: include_str!("../src/bin/coinbase_ticker.rs"),
            mode: Mode::Exact,
        },
        Wiring {
            service: "erapi",
            rust_source: "market-data/src/bin/erapi.rs",
            rust_source_text: include_str!("../src/bin/erapi.rs"),
            mode: Mode::Exact,
        },
        Wiring {
            service: "frankfurter",
            rust_source: "market-data/src/bin/frankfurter.rs",
            rust_source_text: include_str!("../src/bin/frankfurter.rs"),
            mode: Mode::Exact,
        },
        Wiring {
            service: "kraken",
            rust_source: "market-data/src/bin/kraken.rs",
            rust_source_text: include_str!("../src/bin/kraken.rs"),
            mode: Mode::Exact,
        },
        Wiring {
            service: "oanda",
            rust_source: "market-data/src/fx.rs",
            rust_source_text: fx,
            mode: Mode::SubsetOfCompose,
        },
        Wiring {
            service: "twelvedata",
            rust_source: "market-data/src/fx.rs",
            rust_source_text: fx,
            mode: Mode::SubsetOfCompose,
        },
    ]
}

/// Split a `AUD-USD,EUR-USD` roster spec the way `roster::parse_roster` does:
/// trimming the whitespace a YAML fold leaves behind, skipping blank entries,
/// and **upper-casing**.
///
/// The upper-casing is not cosmetic. `parse_roster` normalizes every id before
/// a collector sees it, so `eur-usd` in compose is `EUR-USD` at runtime — which
/// means without it this file would report a false divergence on a pure case
/// difference, and the canonical-id guard below would reject a spelling that
/// works perfectly in production. Matching the runtime normalization is what
/// keeps both from being latent.
fn pairs(spec: &str) -> BTreeSet<String> {
    spec.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_ascii_uppercase)
        .collect()
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
        // rather than at any depth. The continuation rule below keys on eight
        // spaces, so a depth-insensitive match here would let the two rules
        // disagree: a `PRODUCT_IDS:` at some other depth would match, then find
        // none of its own continuation lines and silently yield an empty
        // roster. All eight occurrences in the file sit at six.
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
        match wiring.mode {
            Mode::Exact => assert_eq!(
                rust,
                composed,
                "service `{}`: the roster in {} and the compose default have \
                 diverged. Only in Rust: {:?}; only in compose: {:?}",
                wiring.service,
                wiring.rust_source,
                rust.difference(&composed).collect::<Vec<_>>(),
                composed.difference(&rust).collect::<Vec<_>>(),
            ),
            Mode::SubsetOfCompose => {
                let stray: Vec<_> = rust.difference(&composed).collect();
                assert!(
                    stray.is_empty(),
                    "service `{}`: {} falls back to {stray:?}, which its compose \
                     roster does not carry — a binary run by hand would collect \
                     a pair localnet never does",
                    wiring.service,
                    wiring.rust_source,
                );
            }
        }
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
