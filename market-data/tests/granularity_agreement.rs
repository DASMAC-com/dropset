//! The liveness view probes a literal list of candle widths, and this asserts
//! that list covers every width a collector can actually write.
//!
//! **Why the view has a list at all.** `instrument_source_liveness`
//! (`db-schema/migrations/0010_source_liveness.sql`) iterates the widths so each
//! `cex_prices` probe gets a full equality prefix on that table's primary key
//! `(source, product_id, granularity_secs, bucket_start)`. Without the
//! `granularity_secs` equality Postgres cannot apply its MIN/MAX index
//! transform and scans the whole `(source, product_id)` prefix instead —
//! measured at 76.9 ms and 7,335 buffers against the running store, growing
//! linearly with price history, versus a flat 0.8 ms and 884 buffers with it.
//!
//! **Why that needs a test, and why covering the dashboard is not enough.**
//! Partitioned `max` equals global `max` only when the partitions cover every
//! row, so a width present in `cex_prices` and absent from the view's list
//! makes that series read as never-collected — `last_data_at` NULL, `is_live`
//! false. That is a false "quiet", which `0009_instruments.sql` singles out as
//! the more harmful direction because it HIDES data: the pair silently drops
//! out of any default selection built on the view. Nothing errors.
//!
//! The reachable path is configuration, not code. `GRANULARITY_SECS` is read
//! per collector at startup (`market-data/src/config.rs`), so writing a width
//! outside a short list needs no rebuild — and the adapters accept far more
//! widths than the six Coinbase supports. So the property worth pinning is
//! **containment**: the view's list must be a superset of every width the
//! adapters can emit. Equality would be wrong, because the view deliberately
//! carries widths no collector is configured for today.
//!
//! **Where the writable set comes from.** OANDA and Twelve Data each keep an
//! explicit allowlist, for the reason OANDA's own comment gives — the venue is
//! not a reliable validator, so an unsupported width would silently produce
//! buckets of another size. Alpha Vantage is fixed at daily. Coinbase has **no
//! allowlist in our code**: the width is passed straight through and the venue
//! validates, so its supported set is what bounds it — and that set is exactly
//! the six the dashboard offers, which is where the view's original list came
//! from and why it was too narrow.
//!
//! **Why it compares text.** None of these copies is importable from here: two
//! are `match` arms in another crate, one is SQL inside a migration, one is a
//! string inside dashboard JSON. All are read as source instead, which has the
//! incidental benefit of failing if any file's shape drifts far enough that its
//! list can no longer be found, rather than silently comparing empty sets. The
//! emptiness assertions below make that explicit.

use std::collections::BTreeSet;

/// The widths the liveness view probes, from its `VALUES` list.
///
/// Asserts the anchor matches exactly once. Taking the first match would let a
/// second `VALUES` list added anywhere above it silently re-point this pin —
/// a wrong-pass, which the emptiness assertions below cannot catch.
fn view_vocabulary() -> BTreeSet<i64> {
    let sql = include_str!("../../db-schema/migrations/0010_source_liveness.sql");
    let anchored: Vec<usize> = sql
        .lines()
        .enumerate()
        .filter(|(_, l)| l.contains("AS gran (secs)"))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        anchored.len(),
        1,
        "expected exactly one `AS gran (secs)` anchor in \
         0010_source_liveness.sql, found {} — this test can no longer tell \
         which list it is pinning",
        anchored.len()
    );
    // The list opens at `(VALUES` and closes on the anchor line, so collect
    // every digit run between them rather than assuming a single line: the
    // superset list is wrapped across several.
    let lines: Vec<&str> = sql.lines().collect();
    let opens_at = lines[..=anchored[0]]
        .iter()
        .rposition(|l| l.contains("(VALUES"))
        .expect("the `(VALUES` opening the granularity list");
    lines[opens_at..=anchored[0]]
        .iter()
        .flat_map(|l| numbers_in(l))
        .collect()
}

/// Every width a collector can write: the union across the adapters.
fn writable_widths() -> BTreeSet<i64> {
    let mut widths = BTreeSet::new();
    widths.extend(match_arm_widths(
        include_str!("../../feeds/src/venues/oanda.rs"),
        "fn granularity_code",
    ));
    widths.extend(match_arm_widths(
        include_str!("../../feeds/src/venues/twelvedata.rs"),
        "fn interval_token",
    ));
    // Alpha Vantage serves one width and ignores the configured value. Read
    // the right-hand side only, with separators stripped: scanning the whole
    // line would pick up the `i64` in the type annotation and split `86_400`
    // at its underscore.
    let constant = include_str!("../../feeds/src/venues/alphavantage.rs")
        .lines()
        .find(|l| l.contains("pub const GRANULARITY_SECS"))
        .expect("alphavantage's GRANULARITY_SECS constant")
        .split_once('=')
        .expect("an assignment in the GRANULARITY_SECS constant")
        .1
        .replace('_', "");
    let parsed = numbers_in(&constant);
    assert_eq!(
        parsed.len(),
        1,
        "expected one width in alphavantage's GRANULARITY_SECS, got {parsed:?}"
    );
    widths.extend(parsed);
    // Coinbase has no allowlist, so the dashboard's list stands in for the
    // venue's supported set — the same six it has always offered.
    widths.extend(dashboard_widths());
    widths
}

/// The `secs => "token"` arms of a venue's width-mapping function.
///
/// Reads from the `fn` line to the first `other =>` fallback, taking the
/// left-hand integer of each arm. Underscores in Rust integer literals are
/// stripped by [`numbers_in`]'s digit-run scan only if they do not split the
/// number, so they are removed first.
fn match_arm_widths(source: &str, signature: &str) -> BTreeSet<i64> {
    let lines: Vec<&str> = source.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.contains(signature))
        .unwrap_or_else(|| panic!("`{signature}` in the venue adapter"));
    let arms: BTreeSet<i64> = lines[start..]
        .iter()
        .take_while(|l| !l.contains("other =>"))
        .filter(|l| l.contains("=>"))
        .filter_map(|l| {
            let (lhs, _) = l.split_once("=>")?;
            let cleaned = lhs.replace('_', "");
            numbers_in(&cleaned).into_iter().next()
        })
        .collect();
    assert!(
        !arms.is_empty(),
        "found no width arms under `{signature}` — the mapping function's \
         shape changed and this test is no longer reading it"
    );
    arms
}

/// The dashboard's `granularity` custom variable.
///
/// Anchored on the variable's `"name"` and then the first `"query"` after it.
/// Shape alone is not enough: this dashboard has a second numeric custom
/// variable (`coverage_mins`), so a digits-and-commas match finds two lists and
/// cannot tell which it is pinning.
fn dashboard_widths() -> BTreeSet<i64> {
    let json = include_str!("../grafana/dashboards/market-data.json");
    let lines: Vec<&str> = json.lines().collect();
    let name_at = lines
        .iter()
        .position(|l| l.contains("\"name\": \"granularity\""))
        .expect("the `granularity` variable in market-data.json");
    let query = lines[name_at..]
        .iter()
        .find_map(|l| {
            let (_, rest) = l.split_once("\"query\": \"")?;
            let (value, _) = rest.split_once('"')?;
            Some(value)
        })
        .expect("a `query` after the `granularity` variable's name");
    assert!(
        query.chars().all(|c| c.is_ascii_digit() || c == ','),
        "the `granularity` variable's query is no longer a literal list of \
         seconds (found {query:?}) — if it became query-driven, this test \
         cannot read the venue-supported set from it any more"
    );
    numbers_in(query).into_iter().collect()
}

/// Every run of ASCII digits in `text`, as integers.
fn numbers_in(text: &str) -> Vec<i64> {
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            current.push(ch);
        } else if !current.is_empty() {
            out.push(current.parse().expect("a run of ASCII digits"));
            current.clear();
        }
    }
    if !current.is_empty() {
        out.push(current.parse().expect("a run of ASCII digits"));
    }
    out
}

#[test]
fn the_liveness_view_probes_every_width_a_collector_can_write() {
    let view = view_vocabulary();
    let writable = writable_widths();

    // Neither side may be empty, or the containment check below passes
    // vacuously after a refactor moves a list somewhere this test cannot see.
    assert!(
        !view.is_empty(),
        "found no widths in 0010_source_liveness.sql's `VALUES` list — it \
         moved or changed shape, and this test is no longer pinning anything"
    );
    assert!(
        !writable.is_empty(),
        "found no writable widths across the venue adapters"
    );

    let missing: Vec<&i64> = writable.difference(&view).collect();
    assert!(
        missing.is_empty(),
        "the liveness view cannot see {missing:?} — a collector configured \
         with one of those widths (GRANULARITY_SECS needs no code change) \
         would write bars the view never probes, so every one of its pairs \
         would read as never-collected. That is a false `quiet`, which hides \
         the data rather than reporting it. Add these to the `VALUES` list in \
         0010_source_liveness.sql.\n  view probes: {view:?}\n  writable:    \
         {writable:?}"
    );
}
