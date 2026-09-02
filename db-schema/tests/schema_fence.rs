// cspell:word bytea
// cspell:word matview
// cspell:word matviews
// cspell:word schemaname
// cspell:word tablename
// cspell:word unlogged
// cspell:word unprovisioned
// cspell:word viewname
//! End-to-end tests for the startup fence against a real Postgres in a
//! throwaway container.
//!
//! The fence's whole job is to distinguish three states a live database can be
//! in relative to the binary talking to it — unprovisioned, current, and
//! *ahead* — and to fail on only one of them. The "ahead" case is the one
//! worth pinning: migrations are additive-only precisely so an old binary can
//! keep serving through a deploy that has already advanced the schema, so a
//! fence that demanded equality would turn a supported window into a crash
//! loop. That behavior is invisible to a unit test and easy to regress.
//!
//! Seven tests here are not about the fence at all. They check the migrations
//! produced what they said they would, reading the `<migration>.fence` manifest
//! that sits beside each migration: `migrate_produces_every_declared_relation`
//! probes a container, and six run without one. `parse_manifest` carries the
//! directive grammar; the probe's own doc comment carries the reason the
//! declaration is per-migration rather than one list here, and the reason it is
//! a sidecar rather than a comment in the SQL.
//!
//! Most of these need a Docker daemon, so they are `#[ignore]`d and skipped by
//! the default test run. Run them with:
//!
//! ```sh
//! cargo test -p dropset-db-schema -- --ignored
//! ```

use dropset_db_schema::{connect, expected_version, migrate, require_schema, MIGRATOR};
use sqlx::PgPool;
use std::collections::BTreeMap;
use std::fs;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::{runners::AsyncRunner, ContainerAsync};

/// Start a throwaway Postgres and return a connected pool, with **no** schema
/// applied — each test decides what state to put it in.
async fn start_pg() -> (ContainerAsync<Postgres>, PgPool) {
    let container = Postgres::default()
        .start()
        .await
        .expect("start postgres container");
    let port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("resolve mapped port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = connect(&url).await.expect("connect pool");
    (container, pool)
}

/// Record an applied migration at `version` without running any SQL, to put the
/// bookkeeping table into a state this build's history cannot itself produce.
async fn stamp_version(pool: &PgPool, version: i64, description: &str) {
    sqlx::query(
        "INSERT INTO _sqlx_migrations
             (version, description, installed_on, success, checksum,
              execution_time)
         VALUES ($1, $2, now(), TRUE, ''::bytea, 0)",
    )
    .bind(version)
    .bind(description)
    .execute(pool)
    .await
    .expect("stamp migration row");
}

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn fence_rejects_an_unprovisioned_database() {
    let (_pg, pool) = start_pg().await;

    // Nothing has run: there is not even a bookkeeping table, which the fence
    // must report as a missing schema rather than tripping over the absent
    // relation.
    let err = require_schema(&pool)
        .await
        .expect_err("an empty database must not pass the fence");
    let msg = err.to_string();
    assert!(
        msg.contains("dropset-migrate"),
        "the error must name the fix, got: {msg}"
    );
}

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn fence_accepts_a_migrated_database() {
    let (_pg, pool) = start_pg().await;
    migrate(&pool).await.expect("apply migrations");

    require_schema(&pool)
        .await
        .expect("a freshly migrated database must pass the fence");
}

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn fence_accepts_a_database_ahead_of_this_build() {
    let (_pg, pool) = start_pg().await;
    migrate(&pool).await.expect("apply migrations");

    // The deploy window: the schema has moved on but this binary has not.
    // Additive-only migrations make that safe, so the fence must allow it.
    stamp_version(&pool, expected_version() + 1, "a later migration").await;

    require_schema(&pool)
        .await
        .expect("a database ahead of this build must pass the fence (>=, not ==)");
}

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn fence_rejects_a_database_behind_this_build() {
    let (_pg, pool) = start_pg().await;
    migrate(&pool).await.expect("apply migrations");

    // Roll the bookkeeping back to before this build's history: the tables are
    // real, but the recorded version no longer covers what the binary expects.
    sqlx::query("DELETE FROM _sqlx_migrations")
        .execute(&pool)
        .await
        .expect("clear migration bookkeeping");
    stamp_version(&pool, expected_version() - 1, "an earlier migration").await;

    let err = require_schema(&pool)
        .await
        .expect_err("a database behind this build must not pass the fence");
    let msg = err.to_string();
    assert!(
        msg.contains("dropset-migrate"),
        "the error must name the fix, got: {msg}"
    );
}

/// The bookkeeping table exists but records no successful migration.
///
/// This is the **most reachable** failure state, not a theoretical one: sqlx
/// creates `_sqlx_migrations` *before* applying the first migration, so a run
/// that dies partway through `0001` leaves the table present and **empty**. The
/// opening migration uses plain `CREATE TABLE`, so a database still carrying
/// tables from the retired per-app regimes fails in precisely that way.
///
/// The two halves are deliberately distinct, because only the first can
/// actually occur. On Postgres, sqlx's bookkeeping insert hardcodes
/// `success = TRUE` and the migration shares its transaction, so a failed
/// migration leaves **no row at all** rather than a false one — the
/// `success = FALSE` row exists for the backends that cannot run DDL inside a
/// transaction. So the empty-table case is the real partial-first-run state,
/// and the false-row case is defensive coverage of the query's `WHERE success`
/// filter.
#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn fence_rejects_a_database_with_no_successful_migration() {
    let (_pg, pool) = start_pg().await;
    migrate(&pool).await.expect("apply migrations");

    // The reachable state: the table exists, but holds no rows, exactly as a
    // rolled-back first migration leaves it.
    sqlx::query("DELETE FROM _sqlx_migrations")
        .execute(&pool)
        .await
        .expect("clear migration bookkeeping");
    let err = require_schema(&pool)
        .await
        .expect_err("an empty bookkeeping table must not pass the fence");
    assert!(
        err.to_string().contains("dropset-migrate"),
        "the error must name the fix, got: {err}"
    );

    // The defensive case: a row exists but is not successful, so the filtered
    // aggregate still reads NULL rather than that row's version.
    sqlx::query(
        "INSERT INTO _sqlx_migrations
             (version, description, installed_on, success, checksum,
              execution_time)
         VALUES (1, 'a failed attempt', now(), FALSE, ''::bytea, 0)",
    )
    .execute(&pool)
    .await
    .expect("stamp a failed migration row");

    let err = require_schema(&pool)
        .await
        .expect_err("a database with no successful migration must not pass");
    let msg = err.to_string();
    assert!(
        msg.contains("dropset-migrate"),
        "the error must name the fix, got: {msg}"
    );
}

/// One relation a migration's manifest declares it produces.
///
/// The vocabulary is deliberately tiny: every directive is one more thing a
/// future author has to get right, and the probe below is its only consumer.
///
/// It covers ordinary tables and views and, deliberately, nothing else. A
/// **materialized view satisfies neither** `table` nor `view` — measured on
/// PG 16, it is absent from both `pg_tables` and `pg_views` — so the first
/// migration to add one needs a `matview` directive and a `pg_matviews` probe
/// beside these. Writing that arm now would be speculation, and the failure is
/// loud rather than silent, which is what makes deferring it safe. (Partitioned
/// and unlogged tables both appear in `pg_tables`, so they need nothing.)
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
    assert!(
        orphans.is_empty(),
        "fence manifests with no matching migration: {orphans:?} — if you just \
         added the migration, note `sqlx::migrate!` embeds the history at compile \
         time and cargo does not always notice a new file in the directory, so \
         try rebuilding before believing this"
    );
    declared
}

/// Every migration declares a manifest, and it parses.
///
/// This one runs in the **default** suite — no Docker — so a migration added
/// without a manifest fails an ordinary `cargo test`, with a message naming
/// what to write. That is the enforcement; the container probe below is the
/// verification.
#[test]
fn every_migration_declares_a_fence_manifest() {
    let manifests = manifests();
    assert!(!manifests.is_empty(), "no migrations embedded");
    // The one vacuity `manifests()` cannot catch on its own. A missing manifest
    // panics there and an empty one is a parse error, so the only way to reach
    // an empty work list is for every migration to legitimately declare `none`
    // — at which point the probe below iterates over nothing and passes, which
    // looks exactly like success.
    assert!(
        manifests
            .iter()
            .any(|(_, d)| d.iter().any(|d| !matches!(d, Declared::Nothing))),
        "no migration declares a relation — the probe would be vacuous"
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

/// Every relation the migrations declare actually exists once they have run.
///
/// The fence tests only read `_sqlx_migrations`, and no consumer crate has a
/// Postgres integration test over its own tables, so nothing else would notice
/// a table lost from the squashed history — or from a future edit to it. This
/// asserts the shape mechanically, so fidelity stops depending on review by
/// eye.
///
/// **Each migration declares its own relations, in its own file.** That used to
/// be one central list here, which every migration-bearing branch appended to
/// at the same place — three textual conflicts in one week. A declaration that
/// travels with the migration that produces it puts two such branches in
/// disjoint files, so the conflict is gone by construction rather than merely
/// made less likely.
///
/// **The manifest is a sidecar, not a comment in the SQL**, and that is forced
/// rather than stylistic. `sqlx` checksums a migration's raw file text
/// (`Sha384::digest(sql.as_bytes())`) and `Migrator::run` refuses a migration
/// whose checksum no longer matches what was applied, so a comment added to a
/// shipped migration breaks every database that already ran it — and would make
/// a manifest **uncorrectable** once merged. A `.fence` file is not part of the
/// text `sqlx` hashes, so a mis-declaration stays fixable. `sqlx` ignores any
/// file in the directory that does not end in `.sql`, which is what lets the
/// pair sit side by side.
///
/// **This is a declaration, not a derivation.** Nothing here parses
/// `CREATE TABLE` out of the DDL, which would make the assertion agree with
/// itself whatever the migration did. The manifest is hand-written and says
/// what the author intends; the probe reads what Postgres actually has; a
/// disagreement is the failure. Losing a table from the DDL fails exactly as
/// it did before.
///
/// One thing the vocabulary deliberately does not cover: a migration that only
/// *widens seeded data* — 0006 adds roster rows — has nothing to declare beyond
/// its column, and `rows … = N` would pin a count that is expected to grow.
/// `market-data/tests/pyth_roster_agreement.rs` pins that content instead.
#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn migrate_produces_every_declared_relation() {
    let (_pg, pool) = start_pg().await;
    migrate(&pool).await.expect("apply migrations");

    for (version, declared) in manifests() {
        for decl in declared {
            match decl {
                Declared::Table(table) => {
                    // `pg_tables` scoped to `public`, not `to_regclass`. The
                    // latter resolves *any* relation kind through the
                    // `search_path`, so it would answer yes for a view of the
                    // same name — leaving the substitution `view` exists to
                    // catch unguarded in the one direction that matters more,
                    // since a table silently becoming a view breaks every
                    // writer of it.
                    let present: bool = sqlx::query_scalar(
                        "SELECT EXISTS (
                             SELECT 1 FROM pg_tables
                             WHERE schemaname = 'public' AND tablename = $1
                         )",
                    )
                    .bind(table.as_str())
                    .fetch_one(&pool)
                    .await
                    .expect("probe table");
                    assert!(
                        present,
                        "v{version} declares table `{table}`, which is absent after \
                         migrating (or is not a table)"
                    );
                }
                Declared::View(view) => {
                    let present: bool = sqlx::query_scalar(
                        "SELECT EXISTS (
                             SELECT 1 FROM pg_views
                             WHERE schemaname = 'public' AND viewname = $1
                         )",
                    )
                    .bind(view.as_str())
                    .fetch_one(&pool)
                    .await
                    .expect("probe view");
                    assert!(
                        present,
                        "v{version} declares view `{view}`, which is absent after \
                         migrating (or is not a view)"
                    );
                }
                Declared::Column { table, column } => {
                    // Scoped to `public`, as the table and view probes above
                    // are: without it a same-named table in any visible schema
                    // would satisfy this.
                    let present: bool = sqlx::query_scalar(
                        "SELECT EXISTS (
                             SELECT 1 FROM information_schema.columns
                             WHERE table_schema = 'public'
                               AND table_name = $1
                               AND column_name = $2
                         )",
                    )
                    .bind(table.as_str())
                    .bind(column.as_str())
                    .fetch_one(&pool)
                    .await
                    .expect("probe column");
                    assert!(
                        present,
                        "v{version} declares column `{table}.{column}`, which is absent \
                         after migrating"
                    );
                }
                Declared::Rows { table, count } => {
                    // Interpolated, not bound — see `is_identifier`. Qualified
                    // with `public.` so this resolves the same relation the
                    // table and view probes above do, rather than through
                    // whatever `search_path` happens to be set to.
                    let rows: i64 =
                        sqlx::query_scalar(&format!("SELECT count(*) FROM public.{table}"))
                            .fetch_one(&pool)
                            .await
                            .unwrap_or_else(|e| panic!("count rows in `{table}`: {e}"));
                    assert_eq!(
                        rows, count,
                        "v{version} declares {count} seeded row(s) in `{table}`"
                    );
                }
                Declared::Nothing => {}
            }
        }
    }
}

/// The shared reader role can read every table and cannot write any of them.
///
/// This is the only test in the suite that connects as a role other than the
/// owner, and it has to: `dropset_ro`'s value is entirely in what it is
/// *unable* to do, and every privilege here — the role's own existence, schema
/// `USAGE`, table `SELECT`, and the absence of `INSERT` — is invisible to a
/// connection that holds them all implicitly. Granting one grant too many
/// would leave every other test passing.
///
/// The write probe targets `market_stats` because every column but its primary
/// key carries a default or is nullable, so a one-column `INSERT` is valid SQL
/// for a role that *is* allowed to write — which is what makes a rejection
/// attributable to privileges rather than to a malformed statement.
#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn reader_role_can_read_everything_and_write_nothing() {
    let (pg, pool) = start_pg().await;
    migrate(&pool).await.expect("apply migrations");

    let port = pg
        .get_host_port_ipv4(5432)
        .await
        .expect("resolve mapped port");
    let url = format!("postgres://dropset_ro:dropset_ro@127.0.0.1:{port}/postgres");
    let reader = connect(&url)
        .await
        .expect("the reader role must exist and be able to log in");

    // Derived from the catalog rather than hardcoded. Which relations are
    // expected is declared per-migration and asserted by
    // `migrate_produces_every_declared_relation`; here such a list would only
    // be a fixture, and a stale one — a table added by a later migration would
    // silently never be probed while this test kept passing.
    let tables: Vec<String> =
        sqlx::query_scalar("SELECT tablename FROM pg_tables WHERE schemaname = 'public'")
            .fetch_all(&pool)
            .await
            .expect("list public tables");
    assert!(
        !tables.is_empty(),
        "no tables in `public` — the probe below would vacuously pass"
    );

    for table in &tables {
        sqlx::query(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&reader)
            .await
            .unwrap_or_else(|e| panic!("reader cannot SELECT from `{table}`: {e}"));
    }

    // Views are absent from `pg_tables`, so the loop above cannot see one — and
    // the instruments dimension is read through a view. `ALTER DEFAULT
    // PRIVILEGES ... ON TABLES` does extend to views, but that is precisely the
    // kind of fact worth asserting rather than assuming: were it false, every
    // class-filtered panel would render empty, with no error anywhere.
    let views: Vec<String> =
        sqlx::query_scalar("SELECT viewname FROM pg_views WHERE schemaname = 'public'")
            .fetch_all(&pool)
            .await
            .expect("list public views");
    assert!(
        !views.is_empty(),
        "no views in `public` — the probe below would vacuously pass"
    );

    for view in &views {
        sqlx::query(&format!("SELECT count(*) FROM {view}"))
            .fetch_one(&reader)
            .await
            .unwrap_or_else(|e| panic!("reader cannot SELECT from view `{view}`: {e}"));
    }

    // The `ALTER DEFAULT PRIVILEGES` clause is the one grant nothing above
    // exercises: every table that exists was already covered by the blanket
    // `GRANT SELECT ON ALL TABLES`, so a migration that dropped the default
    // privileges would still pass. It is also the clause the migration's
    // comment leans on hardest, and the one a future change is most likely
    // to break. Create a table as the owner *after* migrating and confirm
    // the reader can read it without any further grant.
    sqlx::query("CREATE TABLE later_migration_table (id INT PRIMARY KEY)")
        .execute(&pool)
        .await
        .expect("create a table as the owner");
    sqlx::query("SELECT count(*) FROM later_migration_table")
        .fetch_one(&reader)
        .await
        .expect("default privileges must cover a table created after 0002");

    let err = sqlx::query("INSERT INTO market_stats (market) VALUES ('probe')")
        .execute(&reader)
        .await
        .expect_err("the reader role must not be able to write");

    // 42501 is `insufficient_privilege`. Asserting the code rather than the
    // message keeps this from passing on some unrelated failure — a syntax
    // error or a dropped table would also be an `Err`.
    let code = err
        .as_database_error()
        .and_then(|e| e.code())
        .map(|c| c.to_string());
    assert_eq!(
        code.as_deref(),
        Some("42501"),
        "expected a privilege rejection, got: {err}"
    );
}

/// The instruments view derives a pair's class from its two legs' kinds.
///
/// The existence probe above cannot see this: a view with a typo in its `CASE`
/// is created exactly as happily as a correct one, and every consequence of
/// getting it wrong is silent — a mislabelled pair lands in the wrong class
/// filter, and a *dropped* pair vanishes from the dashboard while the panel
/// still renders. So the derivation is asserted directly, one product per class
/// the `CASE` can produce.
///
/// This also pins the seeded `currency_kinds` rows the derivation reads: were
/// `EURC` seeded as `fiat`, or `EUR` omitted, the expectations below would move
/// even though the view itself was untouched.
#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn the_instruments_view_derives_a_class_from_the_legs() {
    let (_pg, pool) = start_pg().await;
    migrate(&pool).await.expect("apply migrations");

    // The seed carries no 'crypto' row, because no collector polls an unpegged
    // token yet — so the crypto arm is unreachable from the seed alone. Seed one
    // here, because that arm is the ORDERING-sensitive one: it must win over
    // both the fiat and stablecoin arms, and nothing else tests that.
    sqlx::query("INSERT INTO currency_kinds (currency, kind) VALUES ('SOL', 'crypto')")
        .execute(&pool)
        .await
        .expect("seed a crypto currency");

    // `ZZZ` is the unseeded leg — a symbol the roster does not contain, which is
    // what an unclassified product actually looks like. It has to be distinct
    // from the crypto fixture now that SOL is seeded.
    for (source, product) in [
        ("probe", "EUR-USD"),
        ("probe", "EURC-USDC"),
        ("probe", "EURC-EUR"),
        ("probe", "SOL-USDC"),
        ("probe", "ZZZ-USDC"),
        // The same product under a SECOND source. The dimension must still
        // report exactly one row for it: the view groups by product_id, and
        // that collapse is what stops four collectors polling EUR-USD from
        // duplicating every entry in the dashboard's product picker. Note
        // `fetch_one` below would happily return the first of several rows
        // without erring, so the row count needs its own assertion.
        ("second-src", "EUR-USD"),
    ] {
        sqlx::query(
            "INSERT INTO instrument_registry
                 (source, product_id, first_registered_at, last_registered_at)
             VALUES ($1, $2, 1, 1)",
        )
        .bind(source)
        .bind(product)
        .execute(&pool)
        .await
        .unwrap_or_else(|e| panic!("register `{product}` under `{source}`: {e}"));
    }

    let (rows, sources): (i64, i32) = sqlx::query_as(
        "SELECT count(*), max(source_count)::int FROM instruments
             WHERE product_id = 'EUR-USD'",
    )
    .fetch_one(&pool)
    .await
    .expect("count EUR-USD rows in the dimension");
    assert_eq!(rows, 1, "two sources must collapse to one dimension row");
    assert_eq!(sources, 2, "both sources must be counted");

    for (product, base, quote, class) in [
        ("EUR-USD", "EUR", "USD", "fx-pair"),
        ("EURC-USDC", "EURC", "USDC", "stablecoin-pair"),
        // The peg pair, and why it is its own class rather than a stablecoin
        // pair: it trades at ~1.0 and the only interesting thing about it is
        // the deviation, so folding the two together would hide exactly what
        // this pair exists to measure.
        ("EURC-EUR", "EURC", "EUR", "peg-pair"),
        // Any unpegged leg makes the pair crypto whatever the other leg is, so
        // this arm has to beat the stablecoin one. Ordering-sensitive.
        ("SOL-USDC", "SOL", "USDC", "crypto"),
        // An unseeded leg must leave the product PRESENT and labelled, never
        // drop it. An inner join here would make a class filter hide the
        // series outright while the panel carried on rendering — the same
        // silent-failure shape as a candle field map that fails to a flat
        // line.
        ("ZZZ-USDC", "ZZZ", "USDC", "unclassified"),
    ] {
        let row: (String, String, String) = sqlx::query_as(
            "SELECT base, quote, asset_class FROM instruments WHERE product_id = $1",
        )
        .bind(product)
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|e| panic!("`{product}` missing from the instruments view: {e}"));

        assert_eq!(
            (row.0.as_str(), row.1.as_str(), row.2.as_str()),
            (base, quote, class),
            "wrong derivation for `{product}`"
        );
    }
}

/// The liveness view picks its staleness bound by asset class, so an FX
/// weekend is not read as a dead collector.
///
/// This is the assertion that stops the two thresholds quietly collapsing into
/// one. A single 60-hour silence is required to read as LIVE for an `fx-pair`
/// and STALE for a stablecoin pair — and the FX half is exactly what a flat
/// 48-hour bound would get wrong, every weekend, for every FX pair on the
/// dashboard. Getting it wrong is silent: the pair simply drops out of the
/// default selection.
#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn liveness_picks_its_staleness_bound_by_asset_class() {
    let (_pg, pool) = start_pg().await;
    migrate(&pool).await.expect("apply migrations");

    // (source, product, hours silent) — `None` meaning registered but never
    // collected.
    //
    // The hour values pin BOTH constants from BOTH sides, which one value
    // cannot. A single 60h fixture catches the two mutations that matter most
    // — swapping 72/48, and collapsing them to one number — but `48 -> 24` and
    // `72 -> 96` both survive it. The first of those matters: 24h sits inside
    // the 24-27h publication gap this view explicitly budgets for, so a
    // tightening regression there would be silent.
    let fixtures: [(&str, &str, Option<i64>); 7] = [
        // fx-pair quiet 60h: inside the 72h session bound, so live. This is the
        // weekend case a flat 48h bound gets wrong for every FX pair.
        ("probe", "EUR-USD", Some(60)),
        // fx-pair quiet 80h: past 72h, so stale. Pins 72 from above.
        ("probe", "AUD-USD", Some(80)),
        // stablecoin-pair quiet 60h: past 48h, so stale. The same silence as
        // EUR-USD, opposite verdict — that contrast IS the two-tier design.
        ("probe", "EURC-USDC", Some(60)),
        // stablecoin-pair quiet 30h: inside 48h, so live. Pins 48 from below,
        // at a duration a daily-bar source legitimately reaches.
        ("probe", "USDT-USDC", Some(30)),
        // One product under TWO sources, one long-stale and one fresh. The
        // aggregate must take the freshest: otherwise a single parked collector
        // would drag a live pair out of the default selection. Nothing
        // exercised the cross-source collapse before.
        ("stale-src", "CAD-USD", Some(80)),
        ("fresh-src", "CAD-USD", Some(30)),
        // Registered, never collected.
        ("probe", "GBP-USD", None),
    ];

    for (source, product, silent_hours) in fixtures {
        sqlx::query(
            "INSERT INTO instrument_registry
                 (source, product_id, first_registered_at, last_registered_at)
             VALUES ($1, $2, 1, 1)",
        )
        .bind(source)
        .bind(product)
        .execute(&pool)
        .await
        .unwrap_or_else(|e| panic!("register `{product}` under `{source}`: {e}"));

        if let Some(hours) = silent_hours {
            sqlx::query(
                "INSERT INTO cex_prices
                     (source, product_id, granularity_secs, bucket_start,
                      low, high, open, close, volume)
                 VALUES ($1, $2, 60,
                         EXTRACT(EPOCH FROM now())::BIGINT - $3, 1, 1, 1, 1, 0)",
            )
            .bind(source)
            .bind(product)
            .bind(hours * 3600)
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("insert a bar for `{product}`: {e}"));
        }
    }

    for (product, class, live, has_data) in [
        ("EUR-USD", "fx-pair", true, true),
        ("AUD-USD", "fx-pair", false, true),
        ("EURC-USDC", "stablecoin-pair", false, true),
        ("USDT-USDC", "stablecoin-pair", true, true),
        ("CAD-USD", "fx-pair", true, true),
        // Not live, and carrying no timestamp rather than a zero that would
        // render as 1970 on any panel formatting it as a time.
        ("GBP-USD", "fx-pair", false, false),
    ] {
        let (asset_class, is_live, last_data_at): (String, bool, Option<i64>) = sqlx::query_as(
            "SELECT asset_class, is_live, last_data_at
                 FROM instrument_liveness WHERE product_id = $1",
        )
        .bind(product)
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|e| panic!("`{product}` missing from instrument_liveness: {e}"));

        assert_eq!(asset_class, class, "wrong class for `{product}`");
        assert_eq!(is_live, live, "wrong liveness verdict for `{product}`");
        assert_eq!(
            last_data_at.is_some(),
            has_data,
            "`{product}` last_data_at presence is wrong (got {last_data_at:?})"
        );
    }
}

/// The per-source view answers what the per-product one aggregates away: which
/// COLLECTOR went dark on a pair, rather than whether the pair still has data
/// from somebody.
///
/// The contrast in the `CAD-USD` rows is the whole point, and it is the same
/// fixture shape the test above uses for the opposite purpose. Two sources poll
/// it, one silent 80 hours and one fresh, so `instrument_liveness` reads the
/// PRODUCT live — correctly, and unhelpfully: an operator reading only that
/// view cannot see that half its collectors are dark. Both halves are asserted
/// together here, because either alone is satisfiable by the wrong
/// implementation.
///
/// **What this pins is a regression TOWARD the old shape.** A future edit
/// re-adding a `GROUP BY product_id` to the per-source view would still satisfy
/// every assertion in the test above, and every per-product consumer, while
/// silently restoring the masking this view was added to remove. The row-count
/// assertion is what catches that directly.
#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn source_liveness_separates_a_dark_collector_from_a_live_pair() {
    let (_pg, pool) = start_pg().await;
    migrate(&pool).await.expect("apply migrations");

    // (source, product, hours silent) — `None` meaning registered but never
    // collected. The classes are picked to exercise both staleness bounds on
    // the per-source view too, so a copy of the view that dropped the
    // class-aware `CASE` and took one flat bound fails here rather than only in
    // the per-product test.
    let fixtures: [(&str, &str, Option<i64>); 5] = [
        // One product, two collectors, opposite verdicts. The pair the whole
        // view exists for.
        ("stale-src", "CAD-USD", Some(80)),
        ("fresh-src", "CAD-USD", Some(30)),
        // fx-pair silent 60h: inside the 72h session bound, so live per-source
        // as well — the weekend case must not read as a dark collector.
        ("probe", "EUR-USD", Some(60)),
        // stablecoin-pair silent 60h: past its 48h bound, so stale. Same
        // silence as EUR-USD, opposite verdict, on the per-source view.
        ("probe", "EURC-USDC", Some(60)),
        // Registered, never collected: not live, and carrying no timestamp.
        ("probe", "GBP-USD", None),
    ];

    for (source, product, silent_hours) in fixtures {
        sqlx::query(
            "INSERT INTO instrument_registry
                 (source, product_id, first_registered_at, last_registered_at)
             VALUES ($1, $2, 1, 1)",
        )
        .bind(source)
        .bind(product)
        .execute(&pool)
        .await
        .unwrap_or_else(|e| panic!("register `{product}` under `{source}`: {e}"));

        if let Some(hours) = silent_hours {
            sqlx::query(
                "INSERT INTO cex_prices
                     (source, product_id, granularity_secs, bucket_start,
                      low, high, open, close, volume)
                 VALUES ($1, $2, 60,
                         EXTRACT(EPOCH FROM now())::BIGINT - $3, 1, 1, 1, 1, 0)",
            )
            .bind(source)
            .bind(product)
            .bind(hours * 3600)
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("insert a bar for `{product}`: {e}"));
        }
    }

    for (source, product, class, live, has_data, bound_hours) in [
        ("stale-src", "CAD-USD", "fx-pair", false, true, 72),
        ("fresh-src", "CAD-USD", "fx-pair", true, true, 72),
        ("probe", "EUR-USD", "fx-pair", true, true, 72),
        ("probe", "EURC-USDC", "stablecoin-pair", false, true, 48),
        ("probe", "GBP-USD", "fx-pair", false, false, 72),
    ] {
        let (asset_class, is_live, last_data_at, stale_after_secs): (
            String,
            bool,
            Option<i64>,
            i32,
        ) = sqlx::query_as(
            "SELECT asset_class, is_live, last_data_at, stale_after_secs
                 FROM instrument_source_liveness
                 WHERE source = $1 AND product_id = $2",
        )
        .bind(source)
        .bind(product)
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|e| {
            panic!("`{source}`/`{product}` missing from instrument_source_liveness: {e}")
        });

        assert_eq!(asset_class, class, "wrong class for `{source}`/`{product}`");
        assert_eq!(
            is_live, live,
            "wrong liveness verdict for `{source}`/`{product}`"
        );
        assert_eq!(
            last_data_at.is_some(),
            has_data,
            "`{source}`/`{product}` last_data_at presence is wrong \
             (got {last_data_at:?})"
        );
        assert_eq!(
            stale_after_secs,
            bound_hours * 3600,
            "wrong staleness bound for `{source}`/`{product}`"
        );
    }

    // The masking, asserted directly: the product reads live off the aggregate
    // while one of its two collectors is 80 hours dark. Without this pair of
    // assertions standing together, a view that simply reported the product's
    // verdict under each source would pass everything above.
    let (product_is_live,): (bool,) =
        sqlx::query_as("SELECT is_live FROM instrument_liveness WHERE product_id = 'CAD-USD'")
            .fetch_one(&pool)
            .await
            .expect("`CAD-USD` missing from instrument_liveness");
    assert!(
        product_is_live,
        "the per-product aggregate must still read CAD-USD live — if this \
         fails, the re-derivation changed per-product semantics rather than \
         only adding the per-source axis"
    );

    // One row per registry row, not one per product. This is the assertion a
    // re-introduced `GROUP BY product_id` fails.
    let (rows,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM instrument_source_liveness WHERE product_id = 'CAD-USD'",
    )
    .fetch_one(&pool)
    .await
    .expect("count CAD-USD rows in instrument_source_liveness");
    assert_eq!(
        rows, 2,
        "expected one row per collector for CAD-USD, got {rows} — the view is \
         aggregating across sources"
    );
}

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn migrate_is_idempotent() {
    let (_pg, pool) = start_pg().await;

    // The compose init step re-runs on every stack restart, so a second pass
    // over an up-to-date database has to be a no-op rather than an error on
    // an already-existing table.
    migrate(&pool).await.expect("apply migrations");
    migrate(&pool).await.expect("re-apply migrations");

    require_schema(&pool)
        .await
        .expect("fence passes after re-run");
}
