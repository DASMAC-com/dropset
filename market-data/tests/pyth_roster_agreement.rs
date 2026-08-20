// cspell:word splitn
//! The Pyth FX coordinates exist twice in this repo, deliberately, and this
//! asserts the two copies agree.
//!
//! **Why there are two.** The collector reads its roster from `pyth_fx_feeds`,
//! seeded by `db-schema/migrations/0004_pyth_fx_feeds.sql`, because ECS offers
//! no way to mount a configuration file and adding a cross should not need a
//! rebuild. The maker bot cannot read that table: Postgres is a *soft*
//! dependency in its quote path by design — an unreachable database means
//! degraded quoting, never a refusal to start — so its roster has to survive
//! with no store at all, and stays a compiled constant. Converging the maker
//! onto the table as an override is follow-up work; the constant remains as the
//! degraded-mode fallback either way.
//!
//! **Why that needs a test.** The two copies price the same crosses for the
//! same markets, and the failure mode of divergence is silent in both
//! directions: a wrong id in the seed makes the collector store nothing for
//! that cross (the adapter omits feeds it got no answer for), while a wrong id
//! in the constant makes the maker quote off a different cross than the history
//! it is compared against. Neither errors.
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

/// Pull the crosses out of the migration's seed `INSERT`.
///
/// A seed row is `('EUR', 'EUR-USD', '<64 hex>', false),` — three quoted
/// tokens and a boolean — which is distinctive enough to pick out without
/// parsing SQL.
fn from_migration_seed(sql: &str) -> Vec<Cross> {
    let mut out = Vec::new();
    for line in sql.lines() {
        let line = line.trim();
        if !line.starts_with("('") {
            continue;
        }
        let tokens = quoted_all(line, '\'');
        let [currency, _product_id, feed_id] = tokens.as_slice() else {
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

fn seed_crosses() -> Vec<Cross> {
    let mut crosses = from_migration_seed(include_str!(
        "../../db-schema/migrations/0004_pyth_fx_feeds.sql"
    ));
    crosses.sort();
    crosses
}

/// The point of the whole file: one set of coordinates, written twice.
#[test]
fn the_seed_and_the_maker_constant_name_the_same_crosses() {
    assert_eq!(
        seed_crosses(),
        maker_crosses(),
        "the Pyth roster seed and the maker bot's MARKETS constant have \
         diverged; they must price the same crosses the same way round"
    );
}

/// Guard against the comparison above passing vacuously — if either extractor
/// stops matching its file, this is what fails instead.
#[test]
fn both_copies_were_actually_found() {
    let seed = seed_crosses();
    let maker = maker_crosses();
    assert!(
        seed.len() >= 7,
        "found only {} crosses in the migration seed; the extractor has \
         probably stopped matching its format",
        seed.len()
    );
    assert!(
        maker.len() >= 7,
        "found only {} crosses in the maker constant; the extractor has \
         probably stopped matching its format",
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
