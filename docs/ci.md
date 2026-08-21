# CI build hygiene

How the Rust toolchain is pinned, how it gets bumped, and what the build
caches are expected to do. The companion to `rust-toolchain.toml` and the
three Rust-touching workflows (`lint.yml`, `sdk.yml`, `test.yml`).

## 1. The toolchain is pinned, in exactly one place

`rust-toolchain.toml` at the repo root is the single authority for the
compiler version. Nothing else names it — not a workflow, not a Dockerfile.

CI reads it by using `actions-rust-lang/setup-rust-toolchain` with **no**
`toolchain` input, which installs whatever the file specifies. The more
common `dtolnay/rust-toolchain` cannot do this: its `toolchain` input
defaults to matching its own `@rev`, so the version would have to be
repeated at all six call sites.

Two of that action's defaults are neutralized at every call site, and both
matter:

- `cache: false` — the action bundles `Swatinem/rust-cache`. Each job
  already has an explicit rust-cache step that owns its `shared-key`;
  letting the action add a second one would fight it.
- `rustflags: ''` — the action otherwise sets `RUSTFLAGS=-D warnings` for
  the whole job. The clippy hook passes its own flags, and a stray
  `RUSTFLAGS` also perturbs the rust-cache key.

The file sets `channel` and `components`. Listing clippy and rustfmt there
looks redundant — rustup's default profile already installs both, and only
the lint gate runs them — but it is what makes the shared cache key below
work. `Swatinem/rust-cache` hashes the installed toolchain into its key, so
while lint was the one job adding those two components it always computed a
different hash and could never share another job's cache entry.

Targets are **not** listed. Installed targets do not enter the rust-cache key
— measured: the SDK job installs the `wasm32` target and still hashed
identically to the jobs that do not — so the target stays declared in the SDK
workflow instead of being downloaded by all six jobs.

### Why it is pinned

On 2026-08-20 clippy 1.98 shipped a new lint (`chunks_exact_to_as_chunks`).
CI resolved the floating `@stable` channel and the clippy hook runs with
`-D warnings`, so main went red with **no code change**, and the merge queue
was blocked for every open PR at once. A floating channel makes that a
recurring, unscheduled event. Pinning converts it into a reviewed diff.

## 2. The bump cadence

Bumping the compiler is a deliberate PR, never a surprise:

1. Edit `channel` in `rust-toolchain.toml`.
1. Run the full-workspace gate locally **before** pushing:
   `cargo clippy --all-targets -- -D warnings`. This is the same command
   the pre-commit clippy hook runs, so a clean local run means the gate is
   clean.
1. Fix any new-lint churn in the same PR. New lints are the expected cost
   of a bump and belong with it, not spread across unrelated PRs.
1. Expect the first CI runs after the bump to be slow. The rust-cache key
   embeds a compiler-version component, so a bump invalidates every Rust
   cache at once and the next run of each job builds from scratch.

Do the bump on its own PR when convenient rather than under deadline: it is
the one change guaranteed to cost a full cold rebuild across every job.

## 3. What the caches are expected to do

Rust work is cached in four layers, and all four are expected to hit:

- **`Swatinem/rust-cache`**, scoped per `shared-key` — the registry plus
  dependency build artifacts. Workspace crates are deliberately pruned.
- **sccache**, whole-workspace — content-addressed rustc objects, wired only
  onto the nextest steps. This is what covers the workspace crates and test
  binaries that rust-cache prunes.
- **The SBF `.so` cache**, one entry per feature set — content-addressed over
  the program sources. An exact hit skips `cargo build-sbf` entirely.
- **The Solana toolchain cache**, one entry — keyed on the pinned Solana and
  anchor versions.

Cache entries are readable from the ref that saved them and from the default
branch. `test.yml` and `sdk.yml` both run on `push: main`, so their entries
exist on the default branch and merge-queue runs restore them. `lint.yml`
does **not** run on `push: main` and so never saves a default-branch entry
of its own — which is the second reason it shares the test jobs' key rather
than keeping one.

The `pre-commit` hook cache is still lint-only and still has no
default-branch copy, so it remains cold on merge-queue runs and on the first
run of a new PR. That costs the `install-hooks` step, measured at ~100s.

### One shared key for the Rust caches

The `lint` and `sdk` jobs and the three test jobs all name one cache key,
the test jobs' `rust-test` one. That input **replaces** rust-cache's
automatic job-id component, so jobs naming the same key resolve to the same
entry instead of each storing a near-identical copy. Only the first job to
finish saves it; the others restore and then rebuild whatever their own
feature set adds.

This was worth doing because the duplication was measured, not suspected: on
a single PR ref there were four entries with identical dependency hashes —
`rust-test` 1.19 GB, `rust-postgres` 0.81 GB, `rust-lint` 0.60 GB, `rust-sdk`
0.44 GB. Repo-wide the cache stood at ~10.8 GB against GitHub's 10 GB
per-repo limit, so entries were being evicted continuously, which is the real
explanation for the `pre-commit` cache's roughly even hit/miss split.

`test-postgres` deliberately keeps its own key. It builds no program and
needs none of the SBF-era `target/`, so pointing it at the shared key would
make it restore well over a gigabyte it never reads.

### The visibility guard

Every Rust test job ends with a `Cache effectiveness summary` step that
writes the SBF cache hit flag and `sccache --show-stats` into the run
summary. It runs on failure too. The point is that a regression to cold
compiles shows up in the run that suffers it, instead of being reconstructed
weeks later from a complaint that CI feels slow.
