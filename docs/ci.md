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

Per-job extras stay in the workflow that needs them, not in the toolchain
file: clippy and rustfmt for the lint gate, the `wasm32` target for the SDK
gate. Putting them in the file would install them in all six jobs, including
the four that need neither.

Only `channel` is set. rustup installs a pinned channel with its default
profile, which already includes clippy and rustfmt, so a fresh clone gets a
working lint toolchain with no extra step.

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
does **not** run on `push: main`, so no `rust-lint` or `pre-commit-lint`
entry is ever saved on the default branch — a queue run, and the first run
of any new PR, starts cold there.

### The visibility guard

Every Rust test job ends with a `Cache effectiveness summary` step that
writes the SBF cache hit flag and `sccache --show-stats` into the run
summary. It runs on failure too. The point is that a regression to cold
compiles shows up in the run that suffers it, instead of being reconstructed
weeks later from a complaint that CI feels slow.
