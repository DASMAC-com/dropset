# CI build hygiene

How the Rust toolchain is pinned, how it gets bumped, and what the build
caches are expected to do. The companion to `rust-toolchain.toml` and the
three Rust-touching workflows (`lint.yml`, `sdk.yml`, `test.yml`).

## 1. The toolchain is pinned, in exactly one place

`rust-toolchain.toml` at the repo root is the single authority for the
compiler version. No workflow names a version, and each Dockerfile that
builds Rust takes `FROM rust:1-bookworm` — a major-version base image whose
rustup then resolves this file, so the exact compiler still comes from here.

**Where** an image resolves it is load-bearing, not incidental. The base tag
floats, so rustup has to download the pinned toolchain — five components, and
roughly five minutes of them when measured on an Apple-silicon laptop — and the
layer that download lands in decides how often it is paid. Each Rust image
therefore copies `rust-toolchain.toml` **alone** into its `chef` stage and runs
`rustup toolchain install` there, before any source `COPY`: the layer is then
keyed only on the pin file, so it survives every source change, and
invalidates only on a pin bump or a new base-image digest. Both later stages
descend from it, so `cargo chef cook` and `cargo build` share one compiler.
Before this, the cook stage held only `recipe.json` — no pin file — so it built
on the image's own compiler while the build stage used the pinned one, and much
of the cooked dependency set was invalid at build time: measured at 144 crates
recompiled against 71 afterwards. On this workspace that costs well under a
second, so treat it as a correctness tidy-up rather than a saving; the download
above is where the time went.

`.claude/tools/dockerfile_stages.py` asserts that structure (the
`dockerfile-stages` pre-commit hook, or `make dockerfile-stages ARGS=--show`
to see the graph). It fails:

- a Rust stage that does not inherit the pin layer;
- a pin layer sitting behind a copy from the build context — in that stage or
  in any ancestor, and the test is where the content comes *from*, not how
  broad the source looks;
- a cargo call ahead of the resolve, whether in an earlier instruction or
  earlier in the same `RUN`;
- a resolve that runs somewhere the pin file did not land (rustup searches the
  working directory and its parents, so at-or-below is fine and a parent is
  not);
- any stage naming a second compiler version instead of inheriting one — an
  `x.y` base tag, a version or channel handed to rustup, `cargo +tok`, or
  `ENV RUSTUP_TOOLCHAIN`;
- and a file that builds `FROM` a Rust image in which no stage runs cargo,
  which means the guard checked nothing there.

Before it existed, the pin file reached the images only via `COPY . .`, so
every rebuild-after-commit re-paid the download **twice** — the planner and the
builder are sibling stages, so neither could reuse the other's. Those two
stages took 349.2s and 216.2s in one measured rebuild on 2026-09-15; each is a
whole-step total, roughly 10s of which is cargo and the rest the download.

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

The file sets `channel` and `components`. Listing clippy and rustfmt is
belt-and-braces — rustup's default profile installs both anyway — but it
keeps the one job that runs `-D warnings` from depending on an installer
default.

It is **not** what makes the shared cache key below work. rust-cache's
documented key inputs are the rustc version, this file's hash,
`Cargo.lock` / `Cargo.toml`, and environment variables matching `CARGO`,
`CC`, `CFLAGS`, `CXX`, `CMAKE` or `RUST` — not the installed component set.
The lint job's key diverged because `lint.yml` alone defined `RUST_VERSION`,
which matched that `RUST` prefix; deleting that dead variable is what let it
share.

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
   the pre-commit clippy hook runs in the full sweep (`make lint` and CI),
   so a clean local run means the gate is clean. `make lint-abridged`
   scopes it to the changed crates (§5), so it is not this check.
1. Fix any new-lint churn in the same PR. New lints are the expected cost
   of a bump and belong with it, not spread across unrelated PRs.
1. Expect the first CI runs after the bump to be slow. The rust-cache key
   embeds a compiler-version component, so a bump invalidates every Rust
   cache at once and the next run of each job builds from scratch.

Worth knowing before you touch this file for any reason: the cache key also
embeds **the file's own hash**, so editing even a comment in
`rust-toolchain.toml` invalidates every Rust cache and costs one cold run
across all six jobs. That is not a reason to leave the comments wrong — it is
a reason not to reformat the file idly.

Do the bump on its own PR when convenient rather than under deadline: it is
the one change guaranteed to cost a full cold rebuild across every job.

## 3. What the caches are expected to do

Rust work is cached in up to four layers, each expected to hit in the jobs
that use it — `test-postgres` builds no program, so it has neither the SBF
nor the Solana-toolchain layer:

- **`Swatinem/rust-cache`**, scoped per `shared-key` — the registry plus
  dependency build artifacts. Workspace crates are deliberately pruned.
- **sccache**, whole-workspace — content-addressed rustc objects, wired only
  onto the nextest steps. This is what covers the workspace crates and test
  binaries that rust-cache prunes.
- **The SBF `.so` cache**, one entry per feature set — content-addressed over
  the program sources and the build config, the Makefile included (it picks
  the sBPF version), so any Makefile edit misses it. An exact hit skips
  `cargo build-sbf` entirely.
- **The Solana toolchain cache**, one entry — keyed on the pinned Solana,
  anchor-cli and platform-tools versions.

Cache entries are readable from the ref that saved them and from the default
branch. `test.yml` runs on `push: main`, so the shared entry exists on the
default branch and merge-queue runs restore it. `lint.yml` runs on
`push: main` only to warm the pre-commit hook cache (see below), skipping the
Rust toolchain and cache, and `sdk.yml` no longer saves a Rust cache at all —
which is the second reason both read the test jobs' key instead of keeping
their own.

### Only a push to main saves

The rust-cache steps in `test.yml` and the toolchain save in
`install-toolchain` save only on a push to main. Every other run restores
those layers and never writes them. The reason is the scoping rule above. A
merge-queue run's ref (`gh-readonly-queue/main/pr-N-<sha>`) is used once, so
nothing reads what it saves again. A PR ref's entries are read only by that
same PR's re-runs. Both kinds still count against the 10 GB repo limit.

Before this rule, every off-main run saved whenever it missed. A miss is
exactly what follows main's entry being evicted, so the misses fed the
eviction. Measured on 2026-10-06 at about 01:07 UTC: 11.4 GB across 2,908
entries, the oldest surviving entry about 90 minutes old. The one queue ref
inspected (an entry for PR 452) carried roughly 2 GB of rust-cache and
toolchain copies, and its toolchain save shows its lookup had missed main.
With saves confined to main:

- the rust-cache and toolchain layers are saved only on the default branch;
- every merge-queue run, and every PR run that touches code, restores main's
  entry on the same key. A restore advances the entry's last-accessed time.
  Observed on two PR-scoped entries, whose immutable keys were touched
  again by a later run's restore. That activity keeps main's entries inside
  GitHub's 7-day idle window.

What it does not cover:

- **A week with no CI runs at all.** Main's entries still expire. The next
  queue entry pays a cold build, and the push after it republishes the
  entries. A scheduled warm-up job would close this gap and is deliberately
  not used.
- **A PR that changes the lockfile or toolchain pins.** It misses main's
  exact entry on its own re-runs until it merges. Before this rule, its first
  run seeded a PR-scoped copy for those re-runs. Queue runs that land before
  the post-merge push has saved the new key miss too.
- **sccache, the SBF caches, and other workflows' caches.** They still save
  from every ref. The SBF entries are about 100 KB each. sccache writes
  per-object entries as it compiles, so these steps cannot gate it. The
  `pnpm`, `pre-commit` and `pip` caches belong to workflows this rule does
  not touch. They can still crowd main's entries under the limit.

The `pre-commit` hook cache is lint-only, and its default-branch copy comes
from a dedicated `push: main` run of `lint.yml`. PR and queue runs still save
it on a miss, as before. That run executes only checkout, the Node and Python
setup, the pip install, the hook-cache restore, `install-hooks` and the save;
every other step carries `if: github.event_name != 'push'`, and setup-node's
pnpm cache is off there, since nothing installs into the store. Without
it the cache was cold on every merge-queue run and on the first run of each
new PR, costing the `install-hooks` step: a median of 1.6 minutes across 86
queue Lint jobs in 2026-10-12 to 2026-10-21, about a fifth of the queue's
critical path, which Lint set on 77 of 88 entries. The warm-up costs up to
about two runner-minutes per merge, after the merge and so off the critical
path, and saves only when the key moves. A `schedule:` warm-up would also reach
the default branch, but a config change would then stay cold in the queue
until the next scheduled run, so it is not used.

### One shared key for the Rust caches

The `lint` and `sdk` jobs and the three test jobs all name one cache key,
the test jobs' `rust-test` one. That input **replaces** rust-cache's
automatic job-id component, so jobs naming the same key resolve to the same
entry instead of each storing a near-identical copy.

Only the three test jobs **write** that entry, and only on a push to main
(see above). `lint` and `sdk` set
`save-if: false` and are restore-only, which is load-bearing rather than
tidy: rust-cache skips its save whenever the restore was an exact key match,
so the first job to finish on a fresh key decides that entry's contents for
the whole lockfile-plus-toolchain generation. `sdk` is both the fastest job
and the one with the thinnest dependency set, so letting it win that race
would freeze a thin entry the program-building jobs then restore on every
later run — slowing the critical path, which is the opposite of the point.

The race is narrowed rather than removed: the three writers still contend,
but their dependency builds are near-identical, so whichever wins, the
stored entry is representative. Measured after the change: all five jobs
restore one `v0-rust-rust-test-…` entry with `full match: true`.

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

Every Rust test job ends with a `Cache effectiveness summary` step writing
`sccache --show-stats` into the run summary, fenced so the column-aligned
table survives Markdown rendering. The three program-building jobs also
report their SBF cache hit flag; `test-postgres` omits it, having no such
step. The step runs on failure but not on cancellation, and it is
`continue-on-error` — a visibility guard must never be the thing that reds
an otherwise-green job. The point is that a regression to cold compiles
shows up in the run that suffers it, instead of being reconstructed weeks
later from a complaint that CI feels slow.

## 4. Re-measuring the merge queue

`make ci-metrics` (`.claude/tools/ci_metrics.py`) is the way to re-measure.
It groups the last `--days` of `merge_group` runs by queue entry and reports
each entry's critical path (earliest job start to latest job completion
across every workflow), per-job duration percentiles, which job was slowest
per entry, and the entries that were not all-green with their failed jobs.
An entry with a run still going is in flight: it is left out of the
per-entry statistics and named on a line of its own, though its completed
jobs still count toward the per-job durations.
`--steps JOB` breaks one job down by step, `--failed-log JOB_ID` classifies
a dequeue from its log's failure-shaped lines, and `--hook-timing JOB_ID`
ranks the pre-commit hooks inside one Lint job. The first fetch of a week
takes minutes of `gh api` reads, so pass `--cache FILE` and re-run from it.

## 5. Local lint versus CI

There are two local forms of one hook set (`cfg/pre-commit-lint.yml`), and
CI runs only the first:

- **`make lint`** is the full sweep: every tracked and untracked file, with
  `clippy` and `rustdoc` over the whole workspace. Its commands are the
  ones `lint.yml` runs under `pre-commit run --all-files`, which covers the
  merge queue. Together the two are the source of truth.
- **`make lint-abridged`** is the iteration form: only this branch's files
  (the merge-base diff with `origin/main`, plus untracked), with `clippy`
  and `rustdoc` scoped to the crates those files belong to. A session
  iterates on it, and a pass on it is enough to take the next step.

The scoping is `.claude/tools/cargo_scope.py`, which both hooks run
through. It maps each file to its owning package (the nearest `Cargo.toml`
with a `[package]`) and runs `cargo clippy -p <crate>…` or
`cargo doc -p <crate>…`, passing each crate the features the whole
workspace enables on it. Cargo unifies features only across the selected
packages, so without that a bare `-p dropset-feeds` would build with no
features and skip its feature-gated modules. The set comes from
`cargo metadata`, which counts dev-dependencies: it matches clippy's
`--all-targets` build exactly and is a superset for `cargo doc`, so the
abridged rustdoc can fail on a dev-only feature the full sweep never
documents. When the files cover every tracked `.rs` file it
runs the workspace commands unchanged, which is how the full sweep and CI
stay identical. A Rust file outside any member package maps to the whole
workspace, so a gap in the mapping costs a full sweep and never skips a
check.

The trade: the abridged form does not see a change to one crate's public
API that breaks a **dependent** crate's build or doc links, nor a change
that touches only `Cargo.toml` or deletes a `.rs` file (neither hook is
selected). The merge queue's full run catches all of these, and that is the
trade the `--changed` mode was built to make.

## 6. Local tests versus CI

Tests have the same two forms, and CI again runs only the first:

- **`make test`** is the full suite: `make program`, then a workspace
  `cargo test`, the command `test.yml` runs. It is the source of truth
  together with CI.
- **`make test-abridged`** is the iteration form: `cargo test` over the
  **affected set** of this branch's files (the same `--changed` resolution
  as `make lint-abridged`, with deleted files kept).

The affected set is wider than the lint scope, because a test run has to
cover the crates that use a change, not just the crate that holds it.
`cargo_scope.py test` takes the crates that own the changed files and adds
every workspace crate that depends on them, transitively, over normal, dev
and build edges, read from the `cargo metadata` resolve graph. A changed
file that a Rust test reads seeds that test's crate, using the fixture scan
`review_diff.py` already runs. This holds for a data file and for another
crate's `.rs` source alike: `market-data`'s roster tests read the maker-bot
config this way. A Rust-reachable path that no member
owns, such as `Cargo.lock` or `rust-toolchain.toml`, gives the whole
workspace. Two refinements sit in the tool's `ARTIFACT_CONSUMERS` table,
since the graph cannot see them: the program crate in the set runs
`make program` first and brings in every crate whose tests load the `.so`,
and a change to that build's own inputs (the `Makefile` recipe,
`Anchor.toml`) puts the program crate in the set.
A branch with no Rust-reachable change and no fixture hit makes no cargo
call at all, so a docs-only branch never waits on the cargo pool's lock.
Each scoped call carries the workspace's resolved features, as the lint does, so
feature-gated tests still run. A set that covers every member runs
`make program` and `cargo test`, the full suite exactly.

The trade: the abridged form misses a test whose result moves through a
path neither the graph nor the fixture scan can see. A test reading a file
by a path built at runtime is one example; a crate that starts loading the
`.so` without joining the table is another. The full run catches both.
`review-pr` iterates on the abridged form through the race below.

## 7. Racing local against CI

A verification point is one call, `.claude/tools/verify_head.py`. It
records HEAD, runs `make lint-abridged` and then `make test-abridged`,
each as a process group it owns, and, when HEAD is pushed and the tree is clean,
polls CI for the same SHA. Each check settles on whichever side finishes
first. A green CI `Lint` kills the local lint. The `Tests*` checks all
green kill or skip the local test; the `code` filter reports an excluded
PR's Tests jobs as passing, while a `skipping` one (its `Detect changes`
failed) verified nothing and never settles the check. A red on
either side ends the race. The result is one verdict line naming the
settling side of each check and the SHA, and the call also appends a
`machine_load.py --sample` line to the fleet log.

A local pass is enough to take the next step, never to merge: CI's full
sweep still runs behind it, and the merge queue runs it again. CI is
skipped when it cannot speak for the tree: when HEAD is unpushed, when
the tree is dirty, or after three failed reads in a row. The verdict
then says `not-consulted`. An unpushed or dirty tree heals by committing
and pushing; only an unreachable CI makes a local `make lint` and
`make test` stand in for the full sweep. A new commit mid-race ends it
`HEAD-MOVED` and `--timeout` ends it `TIMEOUT`; either way the caller
acts on a verdict only when its `sha` is HEAD's. The kill signals the
whole process group, SIGTERM and then SIGKILL, so no orphaned `rustc`
holds the cargo pool lock. That lock is a file lock, released when its
process dies.
