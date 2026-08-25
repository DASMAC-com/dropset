// cspell:word splitn
//! The Pyth FX coordinates exist twice in this repo, deliberately, and this
//! asserts the maker's copy is contained in the collector's.
//!
//! **Why there are two.** The collector reads its roster from `pyth_fx_feeds`,
//! seeded by `db-schema/migrations/0005_pyth_fx_feeds.sql`, because ECS offers
//! no way to mount a configuration file and adding a cross should not need a
//! rebuild. The maker bot cannot read that table: Postgres is a *soft*
//! dependency in its quote path by design — an unreachable database means
//! degraded quoting, never a refusal to start — so its roster has to survive
//! with no store at all, and stays a compiled constant. Converging the maker
//! onto the table as an override is follow-up work; the constant remains as the
//! degraded-mode fallback either way.
//!
//! **Why that needs a test.** The failure mode of divergence is silent in both
//! directions: a wrong id in the seed makes the collector store nothing for
//! that cross (the adapter omits feeds it got no answer for), while a wrong id
//! in the constant makes the maker quote off a different cross than the history
//! it is compared against. Neither errors.
//!
//! **Containment, not equality — the sets stopped coinciding on purpose.**
//! `0006_pyth_fx_crosses` widened the collector to every cross Hermes publishes
//! for a roster currency, including crosses with no USD leg, because history
//! cannot be backfilled into a market that did not exist yet. The maker still
//! quotes only its configured markets. So a collected cross the maker ignores
//! is now the normal state, and the property worth pinning is the other
//! direction: the maker must never quote a cross whose history nothing records.
//!
//! **Why it compares text.** The maker bot is a binary-only crate with no `lib`
//! target, so `MARKETS` cannot be imported. Both files are read as source
//! instead — which has the incidental benefit of also failing if either file's
//! shape changes enough that this test can no longer find the coordinates,
//! rather than silently comparing two empty sets. The emptiness assertions
//! below make that explicit.

/// One cross's coordinates, as written in either file.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Cross {
    currency: String,
    feed_id: String,
    invert: bool,
}

/// The first double-quoted token on a line — the Rust-source case.
fn quoted(line: &str) -> Option<&str> {
    let mut parts = line.splitn(3, '"');
    parts.next()?;
    parts.next()
}

/// Every quoted token on a line, in order.
fn quoted_all(line: &str, quote: char) -> Vec<&str> {
    line.split(quote).skip(1).step_by(2).collect()
}

fn is_feed_id(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Pull the crosses out of the maker bot's `MARKETS` constant.
///
/// Each entry writes `currency`, then `pyth_feed_id`, then `pyth_invert`, in
/// that order, so a small state machine over the lines is enough — and it
/// notices if that order ever stops holding, because a feed id with no invert
/// after it is never completed into a `Cross`.
fn from_maker_constant(source: &str) -> Vec<Cross> {
    let mut out = Vec::new();
    let mut currency: Option<String> = None;
    let mut feed_id: Option<String> = None;
    for line in source.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("currency:") {
            currency = quoted(rest).map(str::to_string);
        } else if let Some(rest) = line.strip_prefix("pyth_feed_id:") {
            feed_id = quoted(rest).map(str::to_string);
        } else if let Some(rest) = line.strip_prefix("pyth_invert:") {
            let invert = rest.contains("true");
            if let (Some(currency), Some(feed_id)) = (currency.take(), feed_id.take()) {
                out.push(Cross {
                    currency,
                    feed_id,
                    invert,
                });
            }
        }
    }
    out
}

/// Pull the crosses out of a migration's seed `INSERT`.
///
/// Two row shapes exist, because `0006_pyth_fx_crosses` added the quote leg as
/// its own column:
///
/// - `('EUR', 'EUR-USD', '<64 hex>', false),` — the original three tokens.
/// - `('EUR', 'GBP', 'EUR-GBP', '<64 hex>', false),` — four, with `quote`.
///
/// Both are matched on the **feed id being last**, rather than on a fixed
/// arity. Keying off position from the front is what would silently skip every
/// row of the newer shape — and a skipped row is invisible to a containment
/// check, since a smaller seed still contains the maker's set right up until it
/// doesn't.
fn from_migration_seed(sql: &str) -> Vec<Cross> {
    let mut out = Vec::new();
    for line in sql.lines() {
        let line = line.trim();
        if !line.starts_with("('") {
            continue;
        }
        let tokens = quoted_all(line, '\'');
        let (Some(currency), Some(feed_id)) = (tokens.first(), tokens.last()) else {
            continue;
        };
        if !is_feed_id(feed_id) {
            continue;
        }
        out.push(Cross {
            currency: currency.to_string(),
            feed_id: feed_id.to_string(),
            invert: line.contains("true"),
        });
    }
    out
}

fn maker_crosses() -> Vec<Cross> {
    let mut crosses = from_maker_constant(include_str!("../../bots/maker-bot/src/config.rs"));
    crosses.sort();
    crosses
}

/// Every seeded cross, across both migrations that seed the roster.
///
/// A new migration that seeds more crosses has to be added here, which is the
/// intended friction: the containment check below is only as good as the set it
/// compares against, and a seed file nobody reads makes it pass vacuously.
fn seed_crosses() -> Vec<Cross> {
    let mut crosses = from_migration_seed(include_str!(
        "../../db-schema/migrations/0005_pyth_fx_feeds.sql"
    ));
    crosses.extend(from_migration_seed(include_str!(
        "../../db-schema/migrations/0006_pyth_fx_crosses.sql"
    )));
    crosses.sort();
    crosses
}

/// The point of the whole file: every cross the maker quotes is one the
/// collector records.
///
/// **This was an equality check, and equality was right until it wasn't.** The
/// two sets coincided while the collector stored exactly what the maker quoted.
/// `0006_pyth_fx_crosses` ends that deliberately — the collector now ingests
/// every cross Hermes publishes for a roster currency, because history cannot
/// be backfilled into a market that did not exist yet, while the maker still
/// quotes only its configured markets. Containment is what actually protects
/// the maker: it may not quote a cross whose history is not being recorded,
/// because that is the reading its quote is compared against.
///
/// The reverse direction is intentionally unconstrained. A collected cross the
/// maker does not quote is the normal, desired state now.
#[test]
fn every_cross_the_maker_quotes_is_collected() {
    let seed = seed_crosses();
    let missing: Vec<Cross> = maker_crosses()
        .into_iter()
        .filter(|cross| !seed.contains(cross))
        .collect();
    assert!(
        missing.is_empty(),
        "the maker bot's MARKETS constant names crosses the collector does not \
         record, so it would quote against history that is never stored: {missing:?}"
    );
}

/// Guard against the comparison above passing vacuously — if either extractor
/// stops matching its file, this is what fails instead.
#[test]
fn both_copies_were_actually_found() {
    let seed = seed_crosses();
    let maker = maker_crosses();
    // 7 from 0005 plus 20 from 0006. Pinned as a floor rather than an equality
    // so seeding a new cross does not fail this, while a parser that stops
    // matching either file's row shape still does — which is the whole job,
    // since a short seed makes the containment check pass for the wrong reason.
    assert!(
        seed.len() >= 27,
        "found only {} crosses in the migration seeds; the extractor has \
         probably stopped matching one file's row shape",
        seed.len()
    );
    // This floor is also the ONLY thing now catching a market silently dropped
    // from the maker's MARKETS. Under the old equality check, maker shrinkage
    // failed at any roster size; under containment a shorter maker set is still
    // contained, so it passes. Bump this number whenever MARKETS grows, or the
    // protection quietly stops covering the difference.
    assert!(
        maker.len() >= 7,
        "found only {} crosses in the maker constant; either the extractor has \
         stopped matching its format, or a market was dropped from MARKETS",
        maker.len()
    );
}

/// The shape checks the table's own constraints enforce on any later `INSERT`,
/// applied to the seed itself — so a bad row fails here rather than at deploy.
#[test]
fn every_seeded_feed_id_is_32_bytes_of_lowercase_hex_and_unique() {
    let crosses = seed_crosses();
    let mut ids = std::collections::HashSet::new();
    for cross in &crosses {
        assert!(
            is_feed_id(&cross.feed_id) && cross.feed_id == cross.feed_id.to_ascii_lowercase(),
            "{} has a malformed feed id: {}",
            cross.currency,
            cross.feed_id
        );
        assert_eq!(
            cross.currency.len(),
            3,
            "{} is not ISO 4217",
            cross.currency
        );
        assert!(
            ids.insert(cross.feed_id.clone()),
            "{} reuses another cross's feed id",
            cross.currency
        );
    }
}
