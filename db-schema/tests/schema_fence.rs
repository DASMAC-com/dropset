// cspell:word matview
// cspell:word matviews
// cspell:word unlogged
//! The fence manifests beside the migrations, checked without a database.
//!
//! Each migration carries a `<migration>.fence` sidecar declaring the
//! relations it produces. These tests hold the pairing in both directions
//! (every migration has a manifest, no manifest is orphaned), the directive
//! grammar `parse_manifest` accepts, and the build script that keeps the
//! embedded history in step with the directory. None of them needs a running
//! Postgres, so all of them run in the default suite.
//!
//! The container-backed tests that used to sit here, which ran the history
//! against a real server and probed each declared relation, were removed with
//! the old stack's Postgres tests. Nothing probes a manifest against a live
//! database anymore; the grammar and the pairing are what is left, and
//! where a comment below speaks of "the probe" it means that removed check.

use dropset_db_schema::MIGRATOR;
use std::collections::BTreeMap;
use std::fs;

/// One relation a migration's manifest declares it produces.
///
/// The vocabulary is deliberately tiny: every directive is one more thing a
/// future author has to get right.
///
/// It covers ordinary tables and views and, deliberately, nothing else. A
/// **materialized view satisfies neither** `table` nor `view` — measured on
/// PG 16, it is absent from both `pg_tables` and `pg_views` — so the first
/// migration to add one needs a `matview` directive beside these. (Partitioned
/// and unlogged tables both appear in `pg_tables`, so they need nothing.)
///
/// The fields are compared, never read: their one reader was the container
/// probe that checked each relation against a live database, removed with the
/// old stack's Postgres tests. The parser tests below still pin what each line
/// parses to.
#[allow(dead_code)]
#[derive(Debug, PartialEq, Eq)]
enum Declared {
    /// A table — specifically a table, in `public` — that must exist once the
    /// history has run.
    Table(String),
    /// A view that must exist in `public`. Distinct from `Table` because each
    /// probe asserts the relation *kind*, not merely that the name resolves:
    /// one directive covering both would pass just as happily if a later
    /// migration swapped a view for a table of the same name, or the reverse,
    /// and the difference is exactly what a consumer reading it depends on.
    View(String),
    /// A column added to a table an earlier migration created. The table probe
    /// cannot see one of these — a migration that only adds columns creates
    /// nothing, so without this it would pass having done nothing.
    Column { table: String, column: String },
    /// A row count the migration seeds itself. Seeded rows are written by the
    /// migration rather than by any app, so their absence surfaces only at read
    /// time, as a missing row in an aggregate far from the cause.
    Rows { table: String, count: i64 },
    /// The migration produces no relation at all, with a stated reason. It is
    /// explicit because a migration that declares nothing has to stay
    /// distinguishable from one whose author forgot to declare anything.
    Nothing,
}

/// Postgres folds unquoted identifiers to lower case and this schema never
/// quotes one, so the accepted shape is exactly what the migrations already
/// write. Validating it is not ceremony: the row probe has to interpolate a
/// table name (an identifier cannot be a bind parameter), and this is what
/// keeps that interpolation to a fixed, committed alphabet.
fn is_identifier(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with(|c: char| c.is_ascii_digit())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// The message for a directive the parser recognized but whose identifier it
/// cannot accept, stated once so the three arms that need it agree.
fn bad_identifier(directive: &str, got: &str) -> String {
    format!(
        "`{directive}` wants a lowercase identifier — [a-z0-9_], not starting \
         with a digit — but got `{got}`"
    )
}

/// Parse one migration's fence manifest.
///
/// Blank lines and `#` comments are skipped; **every other line must parse**.
/// There is deliberately no "line this parser did not recognize" case, because
/// silently dropping a typo would quietly narrow the assertion for that
/// migration, and an assertion that passes vacuously is the one outcome this
/// whole mechanism exists to prevent.
fn parse_manifest(text: &str) -> Result<Vec<Declared>, String> {
    let mut declared = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        declared.push(match fields.as_slice() {
            ["table", name] if is_identifier(name) => Declared::Table((*name).to_string()),
            ["view", name] if is_identifier(name) => Declared::View((*name).to_string()),
            ["column", dotted] => match dotted.split_once('.') {
                Some((table, column)) if is_identifier(table) && is_identifier(column) => {
                    Declared::Column {
                        table: table.to_string(),
                        column: column.to_string(),
                    }
                }
                _ => return Err(format!("`column` wants `table.column`, got `{dotted}`")),
            },
            ["rows", table, "=", count] if is_identifier(table) => Declared::Rows {
                table: (*table).to_string(),
                count: match count.parse::<i64>() {
                    // A negative count parses as an integer but can never be
                    // satisfied, so rejecting it here keeps the diagnostic on
                    // the manifest line instead of deferring it to a container
                    // run that reports an unsatisfiable row count.
                    Ok(n) if n >= 0 => n,
                    _ => {
                        return Err(format!(
                            "`rows` wants a non-negative integer count, got `{count}`"
                        ))
                    }
                },
            },
            // A reason is mandatory: bare `none` falls to the error arm below,
            // so declaring nothing costs a sentence saying why.
            ["none", _, ..] => Declared::Nothing,
            // A directive whose shape is right but whose identifier is not gets
            // its own error. These must sit after the guarded arms above, and
            // they exist because the catch-all would otherwise answer a typo'd
            // hyphen with "unrecognized directive" — pointing the author at the
            // half of the line they got right.
            ["table", name] => return Err(bad_identifier("table", name)),
            ["view", name] => return Err(bad_identifier("view", name)),
            ["rows", table, "=", _] => return Err(bad_identifier("rows", table)),
            _ => return Err(format!("unrecognized directive: `{line}`")),
        });
    }

    if declared.is_empty() {
        return Err("declares nothing (write `none <reason>` if the migration \
                    creates no relation)"
            .to_string());
    }
    if declared.len() > 1 && declared.contains(&Declared::Nothing) {
        return Err("`none` cannot be combined with other directives".to_string());
    }
    Ok(declared)
}

/// Where the migrations and their manifests live.
///
/// Resolved at compile time rather than from the working directory, so this
/// does not depend on where the test was invoked from.
const MIGRATIONS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/migrations");

/// Every `<version>_<name>.fence` beside the migrations, keyed by version.
fn manifest_files() -> BTreeMap<i64, (String, String)> {
    let entries =
        fs::read_dir(MIGRATIONS_DIR).unwrap_or_else(|e| panic!("read {MIGRATIONS_DIR}: {e}"));
    let mut found = BTreeMap::new();
    for entry in entries {
        let path = entry.expect("read dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("fence") {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .expect("manifest filename is UTF-8")
            .to_string();
        // The same `<version>_` prefix sqlx parses a migration filename with.
        // Pairing is on the parsed **version** only — reading the version out
        // of the filename rather than reconstructing the filename from the
        // version, which would have to assume the zero-padding width. The
        // descriptive half is not compared, so a `.fence` whose suffix drifts
        // from its migration's still pairs; that is cosmetic, and checking it
        // would couple this to how sqlx derives a description.
        let version: i64 = name
            .split_once('_')
            .and_then(|(v, _)| v.parse().ok())
            .unwrap_or_else(|| panic!("`{name}` needs an integer version prefix"));
        let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read `{name}`: {e}"));
        assert!(
            found.insert(version, (name.clone(), text)).is_none(),
            "two fence manifests claim version {version} (`{name}`)"
        );
    }
    found
}

/// Every migration's manifest, paired with the version that declared it.
///
/// Pairing is checked in **both** directions. A migration with no manifest is
/// the case the whole mechanism turns on, and an orphaned manifest is the
/// quieter one: left behind by a renamed or removed migration, it would
/// otherwise sit there declaring relations nobody probes.
fn manifests() -> Vec<(i64, Vec<Declared>)> {
    let mut files = manifest_files();
    let declared = MIGRATOR
        .iter()
        .map(|m| {
            let (name, text) = files.remove(&m.version).unwrap_or_else(|| {
                panic!(
                    "migration v{} ({}) has no `.fence` manifest beside it in \
                     {MIGRATIONS_DIR}; add one declaring its relations, or \
                     `none <reason>` if it creates none",
                    m.version, m.description
                )
            });
            let declared = parse_manifest(&text).unwrap_or_else(|e| panic!("`{name}`: {e}"));
            (m.version, declared)
        })
        .collect();
    let orphans: Vec<&String> = files.values().map(|(name, _)| name).collect();
    // The cause list is exhaustive because `build.rs` keeps the embedded
    // history in step with this directory, so a stale `MIGRATOR` is not among
    // the ways it can fire; `build_script_emits_rerun_if_changed_for_migrations`
    // is what holds that.
    assert!(
        orphans.is_empty(),
        "fence manifests with no matching migration: {orphans:?} — no \
         migration declares one of these versions. Either there is no \
         migration at that version (write it, or delete the fence), or a \
         fence's version prefix does not match its migration's (correct the \
         prefix)."
    );
    declared
}

/// The build script that keeps the embedded history fresh is still wired in.
///
/// `sqlx::migrate!` reads `./migrations` at macro-expansion time, and the files
/// it reads that way never reach cargo's dependency graph — so without a
/// `rerun-if-changed` on that directory, a rebase that lands someone else's
/// migration leaves the embedded history describing the previous tree, and
/// `manifests` reports the newcomer's `.fence` as an orphan.
///
/// This is a guard rather than a comment because the fix lives in a file
/// nothing else in the crate references: dropping it passes every other check
/// in the repo — the suite, `cargo check --workspace`, `make lint` — while
/// quietly removing the guarantee the orphan message above depends on.
///
/// Two assertions, because "the file is present" and "cargo runs it as a build
/// script" are different claims. `OUT_DIR` is set for a package's targets only
/// when cargo has a build script to run, so it is the one channel that
/// separates a wired-in `build.rs` from one sitting on disk unused — a
/// `build = false`, or a `package.exclude`, which `include_str!` alone cannot
/// see, and which this crate is exposed to because it declares no `build =` key
/// and relies on auto-detection. `include_str!` covers the other half: it fails
/// to **compile** when the file is gone, and rustc records its targets in
/// dep-info, so the guard cannot go stale against the file it reads.
///
/// The needle carries the format capture rather than the expanded path, which
/// keeps it from matching prose that merely mentions the directive — this file
/// and `build.rs` both discuss it in comments — and leaves it indifferent to
/// the newer `cargo::` spelling.
///
/// The path itself is deliberately not re-asserted. Renaming the directory
/// breaks `sqlx::migrate!("./migrations")` at compile time, so the compiler
/// already enforces that coupling, and a mistyped const leaves cargo watching a
/// path that does not exist — which errs toward rebuilding too often rather
/// than toward the silent stale embed this exists to prevent.
///
/// The limit, stated plainly: this proves the directive is written and that
/// cargo runs the script, never that an invalidation actually fired. Nothing
/// here can prove the latter, because a stale embed is observable only
/// indirectly — as an orphan indistinguishable from a genuine one.
#[test]
fn build_script_emits_rerun_if_changed_for_migrations() {
    assert!(
        option_env!("OUT_DIR").is_some(),
        "OUT_DIR is unset, so cargo is not running db-schema/build.rs as a \
         build script — the file may be present but unwired, e.g. by a \
         `build = false` or a `package.exclude`"
    );
    const BUILD_RS: &str = include_str!("../build.rs");
    assert!(
        BUILD_RS.contains("rerun-if-changed={MIGRATIONS_DIR}"),
        "db-schema/build.rs must emit its `rerun-if-changed` for the migrations \
         directory through the MIGRATIONS_DIR capture, or `sqlx::migrate!`'s \
         embedded history goes stale against the tree and the orphaned-manifest \
         assertion fires after an unrelated rebase"
    );
}

/// Every migration declares a manifest, and it parses.
///
/// It needs no database, so a migration added without a manifest fails an
/// ordinary `cargo test`, with a message naming what to write.
#[test]
fn every_migration_declares_a_fence_manifest() {
    let manifests = manifests();
    assert!(!manifests.is_empty(), "no migrations embedded");
    // The one vacuity `manifests()` cannot catch on its own. A missing manifest
    // panics there and an empty one is a parse error, so the only way to reach
    // a manifest set that declares no relation at all is for every migration
    // to legitimately declare `none`.
    assert!(
        manifests
            .iter()
            .any(|(_, d)| d.iter().any(|d| !matches!(d, Declared::Nothing))),
        "no migration declares a relation — the manifests would assert nothing"
    );
}

#[test]
fn manifest_parser_accepts_each_directive() {
    let text = "# a comment\n\
                \n\
                table push_health\n\
                view instruments\n\
                column maker_legs.fused_value\n\
                rows indexer_cursor = 1\n";
    assert_eq!(
        parse_manifest(text).expect("the manifest must parse"),
        vec![
            Declared::Table("push_health".to_string()),
            Declared::View("instruments".to_string()),
            Declared::Column {
                table: "maker_legs".to_string(),
                column: "fused_value".to_string(),
            },
            Declared::Rows {
                table: "indexer_cursor".to_string(),
                count: 1,
            },
        ]
    );
}

/// `none` gets its own acceptance test rather than riding the combination case
/// in the rejection table below.
///
/// Delete the `["none", _, ..]` arm and that rejection case still passes — it
/// just fails as "unrecognized directive" instead, while its label still claims
/// it proved `none` cannot be combined with a real declaration. So the arm was
/// pinned only indirectly, by 0002 being the one migration that uses it.
#[test]
fn manifest_parser_accepts_none_with_a_reason() {
    assert_eq!(
        parse_manifest("none creates a login role and its grants, not a relation\n")
            .expect("the manifest must parse"),
        vec![Declared::Nothing]
    );
}

/// Each of these would otherwise narrow the probe without failing it, which is
/// the failure mode worth pinning: a manifest that parses to fewer relations
/// than the author wrote still passes, and says nothing about what it dropped.
#[test]
fn manifest_parser_rejects_what_would_silently_narrow_the_probe() {
    for (text, why) in [
        ("# only a comment\n", "a manifest declaring nothing"),
        ("none\n", "`none` without a reason"),
        ("column maker_legs\n", "a column without its table"),
        ("rows t = many\n", "a non-integer row count"),
        ("tables t\n", "an unrecognized directive"),
        ("table t extra\n", "a directive with a trailing word"),
        (
            "none nothing to declare\ntable t\n",
            "`none` combined with a real declaration",
        ),
    ] {
        assert!(parse_manifest(text).is_err(), "must reject {why}");
    }
}

/// Every branch of `is_identifier`, on every directive that has to enforce it.
///
/// Split out from the table above because these are the guard on the one probe
/// that **interpolates** its table name into SQL rather than binding it — so
/// the alphabet is the whole bound, and one hyphen case was not enough to pin
/// it. The `rows` case is the load-bearing one.
#[test]
fn manifest_parser_rejects_every_shape_of_bad_identifier() {
    for (text, why) in [
        ("table push-health\n", "a hyphen"),
        ("table Push_health\n", "an uppercase letter"),
        ("table 0feed\n", "a leading digit"),
        ("view push-health\n", "a hyphen on a view"),
        (
            "column maker-legs.fused_value\n",
            "a hyphen on the table half",
        ),
        (
            "column maker_legs.fused-value\n",
            "a hyphen on the column half",
        ),
        ("rows t-x = 1\n", "a hyphen on an interpolated table name"),
        ("rows T = 1\n", "an uppercase interpolated table name"),
    ] {
        assert!(parse_manifest(text).is_err(), "must reject {why}");
    }

    // Assert the message, not just the rejection. Every case above still fails
    // if the three `bad_identifier` arms are deleted — it just fails as
    // "unrecognized directive" — so `is_err()` alone does not pin the one thing
    // those arms exist for, which is telling the author which half of the line
    // is wrong. This is the same argument `manifest_parser_accepts_none_with_a_reason`
    // makes about its own arm.
    let err = parse_manifest("table push-health\n").expect_err("must reject a hyphen");
    assert!(
        err.contains("lowercase identifier"),
        "the error must name the identifier rule, got: {err}"
    );
}

/// A negative count parses as an integer but can never be satisfied, so it is
/// rejected at the manifest rather than deferred to a container run that would
/// report it as an unsatisfiable row count.
#[test]
fn manifest_parser_rejects_a_negative_row_count() {
    assert!(parse_manifest("rows indexer_cursor = -1\n").is_err());
    assert_eq!(
        parse_manifest("rows indexer_cursor = 0\n").expect("zero is a legal count"),
        vec![Declared::Rows {
            table: "indexer_cursor".to_string(),
            count: 0,
        }]
    );
}
