//! Invalidate the crate whenever `./migrations` changes, so the history
//! `sqlx::migrate!` bakes into `MIGRATOR` cannot go stale against the tree.
//!
//! `sqlx::migrate!` reads the directory at **macro expansion** time, and a
//! proc macro contributes nothing to cargo's dependency graph: cargo tracks
//! the crate's `.rs` sources, notices that none of them changed, and reuses
//! the cached build — embedded history and all. So pulling in someone else's
//! migration on a rebase leaves `MIGRATOR` describing the previous tree,
//! while everything that reads the directory at runtime sees the new file.
//!
//! That divergence surfaces as a FALSE failure of the fence-manifest test in
//! `tests/schema_fence.rs`: the new migration's `.fence` is on disk, so the
//! directory scan finds it, but the stale `MIGRATOR` never claims it and it
//! is reported as an orphaned manifest. A guard whose red means "someone
//! else landed a migration" teaches sessions to disbelieve red, which is the
//! worst thing a guard can teach — hence fixing the invalidation rather than
//! documenting the symptom.
//!
//! One `rerun-if-changed` on the directory is the whole fix. Cargo scans a
//! watched directory recursively, so a migration APPEARING counts as a
//! change — which is the case that matters here, and the one a per-file list
//! could not cover, since the files it would need to name are exactly the
//! ones that do not exist yet.

/// The embedded history's source of truth, watched as a whole.
///
/// Matches the literal `sqlx::migrate!` is given in `src/lib.rs`; the two
/// have to name the same directory for the invalidation to cover the embed.
const MIGRATIONS_DIR: &str = "migrations";

fn main() {
    println!("cargo:rerun-if-changed={MIGRATIONS_DIR}");
}
