//! The candle granularity vocabulary exists twice, and this asserts the two
//! copies agree.
//!
//! **Why there are two.** `instrument_source_liveness`
//! (`db-schema/migrations/0010_source_liveness.sql`) iterates the vocabulary so
//! each `cex_prices` probe gets a full equality prefix on that table's primary
//! key `(source, product_id, granularity_secs, bucket_start)`. Without the
//! `granularity_secs` equality Postgres cannot apply its MIN/MAX index
//! transform and scans the whole `(source, product_id)` prefix instead —
//! measured at 76.9 ms and 7,335 buffers against the running store, growing
//! linearly with price history, versus 0.8 ms and 884 buffers with it. The
//! dashboard's `granularity` variable
//! (`market-data/grafana/dashboards/market-data.json`) holds the same list
//! because a Grafana custom variable is a literal list by construction.
//!
//! **Why that needs a test.** Divergence is silent, and silent in the worse
//! direction. A granularity present in `cex_prices` but missing from the view's
//! list contributes no rows to that series' `max`, so the pair reads as
//! never-collected — `last_data_at` NULL, `is_live` false. That is a false
//! "quiet", which `0009_instruments.sql` singles out as the more harmful error
//! because it HIDES data: the pair silently drops out of any default selection
//! built on the view. Nothing errors, and the panel looks fine.
//!
//! This repo has already been bitten by the mutable half of the same problem.
//! The `fx-analytics` dashboard's granularity variable was converted from a
//! fixed list to a query-driven one precisely because the fixed list "offered
//! every bucket size the schema permits while any one venue and product holds
//! exactly one", so most of its entries selected a silently empty panel. The
//! view cannot take that escape — a `SELECT DISTINCT` over the prefix is the
//! very scan the vocabulary exists to avoid — so it keeps the list and pays for
//! it with this test.
//!
//! **Why it compares text.** Neither copy is importable: one is SQL inside a
//! migration, the other a string inside dashboard JSON. Both are read as source
//! instead, which has the incidental benefit of failing if either file's shape
//! drifts far enough that the list can no longer be found, rather than silently
//! comparing two empty sets. The emptiness assertions below make that explicit.

/// The migration's `VALUES` list, as written in the `bars` lateral.
fn view_vocabulary() -> Vec<i64> {
    let sql = include_str!("../../db-schema/migrations/0010_source_liveness.sql");
    let line = sql
        .lines()
        .find(|l| l.contains("(VALUES ("))
        .expect("the `(VALUES (` granularity list in 0010_source_liveness.sql");
    let mut secs = numbers_in(line);
    secs.sort_unstable();
    secs
}

/// The dashboard's `granularity` custom variable, as written in its JSON.
///
/// Anchored on the variable's `"name"` and then the first `"query"` after it.
/// Shape alone is not enough to identify it: this dashboard has a second
/// numeric custom variable (`coverage_mins`, a list of minute windows), so a
/// digits-and-commas match would find two lists and could not tell which one
/// it was pinning. The first draft of this test did exactly that and failed
/// loudly, which is the behavior to preserve — hence the assertions below
/// rather than a `find().unwrap_or_default()`.
fn dashboard_vocabulary() -> Vec<i64> {
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
         cannot pin the view's vocabulary any more and the view needs a \
         different guard"
    );
    let mut secs = numbers_in(query);
    secs.sort_unstable();
    secs
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
fn the_liveness_view_and_the_dashboard_agree_on_the_granularities() {
    let view = view_vocabulary();
    let dashboard = dashboard_vocabulary();

    // Neither side may be empty, or the comparison below passes vacuously
    // after a refactor moves one of the lists somewhere this test cannot see.
    assert!(
        !view.is_empty(),
        "found no granularities in 0010_source_liveness.sql — the `(VALUES (` \
         line moved or changed shape, and this test is no longer pinning \
         anything"
    );
    assert!(
        !dashboard.is_empty(),
        "found no granularities in market-data.json's granularity variable"
    );

    assert_eq!(
        view, dashboard,
        "the liveness view's granularity vocabulary and the dashboard's \
         granularity variable have diverged. Add the granularity in BOTH \
         places. A granularity missing from the view's list makes its series \
         read as never-collected — a false `quiet` that hides the data rather \
         than reporting it."
    );
}
