//! Invalidate the crate whenever `./migrations` changes, so the history
//! `sqlx::migrate!` bakes into `MIGRATOR` cannot go stale against the tree.
//!
//! `sqlx::migrate!` reads the directory at **macro expansion** time, and the
//! files it reads that way do not reach cargo's dependency graph: cargo
//! tracks the crate's `.rs` sources, notices that none of them changed, and
//! reuses the cached build — embedded history and all. So pulling in someone
//! else's migration on a rebase leaves `MIGRATOR` describing the previous
//! tree, while everything that reads the directory at runtime sees the new
//! file.
//!
//! That divergence surfaces as a **false** failure of the fence-manifest
//! test in `tests/schema_fence.rs`: the new migration's `.fence` is on disk,
//! so the directory scan finds it, but the stale `MIGRATOR` never claims it
//! and it is reported as an orphaned manifest — a guard going red for a
//! reason that has nothing to do with the tree being wrong.
//!
//! One `rerun-if-changed` on the directory is the whole fix. Cargo takes the
//! **newest mtime** under a watched directory, so a migration **appearing**
//! counts as a change — which is the case that matters here, and the one a
//! per-file list could not cover, since the files it would need to name are
//! exactly the ones that do not exist yet.

/// The embedded history's source of truth, watched as a whole.
///
/// Names the same directory as the literal `sqlx::migrate!` is given in
/// `src/lib.rs` (`"./migrations"`); the two have to resolve to one directory
/// for the invalidation to cover the embed. Both are relative to
/// `CARGO_MANIFEST_DIR`, which is what makes the two spellings equivalent.
const MIGRATIONS_DIR: &str = "migrations";

fn main() {
    println!("cargo:rerun-if-changed={MIGRATIONS_DIR}");
}
