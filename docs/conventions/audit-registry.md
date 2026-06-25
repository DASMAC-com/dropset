# Audit registry

`audit-loop` reads its coverage map from here — the **subsystems**
to range over, the **interfaces** between them where contract drift
hides, and the **skip-globs** of generated / vendored paths never
worth auditing (`audit-scope` reads just the subsystem `kind`). These
lists live in this committed, shared doc (referenced from `CLAUDE.md`)
rather than in per-worktree state, and `review-pr` refreshes them on
every run: when a diff introduces a new subsystem, a new seam between
subsystems, or a new generated-file family, it appends the entry here
so the registry stays current as the system grows. Keep all three
blocks lint-clean (MD013 80-col, mdformat).

**Subsystems** — `name (kind, risk): roots`. `kind` selects the
per-platform audit checklist; `risk` weights selection.

```txt
program (solana-program, high): programs/dropset/src/**
sdk-math (rust-lib, high): sdk/math-core/src/**, sdk/interface/src/**
sdk-clients (gen-client, med): sdk/rs/src/**, sdk/ts/src/**, sdk/codama/**
frontend (web-app, med): frontend/**
tui (rust-lib, low): tui/**
docs (specs, med): docs/**
ci-infra (ci, low): .github/**, cfg/**, infra/**, Makefile, Anchor.toml
tools (rust-tool, low): tools/**
maker-bot (rust-tool, low): bots/maker-bot/**
taker-bot (rust-tool, low): bots/taker-bot/**
```

**Inter-subsystem interfaces** — the seams where contract drift
hides; `A <-> B: the contract that crosses the boundary`.

```txt
program <-> sdk-clients: the Anchor IDL (sdk/idl/dropset.json) is
  generated from the program; the Rust/TS clients are generated from
  the IDL — accounts, instructions, and on-chain events (FillEvent)
  must stay in lockstep.
program <-> sdk-math: the program depends on the shared math
  (sdk/math-core, sdk/interface) and must compute identically to it;
  the conformance vectors (sdk/conformance) pin price/share/quoting
  parity across the boundary.
program <-> frontend: the on-chain account/instruction contract in
  docs/interface.md, which the frontend builds transactions against
  through the generated clients.
sdk-math <-> frontend: no live wiring today. The frontend imports no
  sdk-math (a grep of frontend/ for @dropset/sdk is empty) and consumes
  no WASM; it quotes via DFlow's API (frontend/lib/dflow/) and shows a
  display-only float PnL re-implementation (frontend/lib/data/pnl.ts) of
  the share kernels that no conformance vector pins, over static mock
  data. The drift to watch is the day pnl.ts is wired to live on-chain
  reserves: its float math can then diverge from the integer engine.
tui <-> sdk-math: the resting-book matcher surface (sdk/interface
  matching `resting_levels` / `BookLevel`) the TUI's order-book pane
  reconstructs depth from — the SDK normalizes a bid's quote leg to base
  at the level price, and the pane (tui/src/book.rs) de-scales by mint
  decimals, so the two must agree on the base-atom denomination.
maker-bot <-> program: the bot quotes and submits against the on-chain
  account/instruction contract (docs/interface.md) through the generated
  SDK clients (sdk/rs) — instruction args and accounts must match.
taker-bot <-> program: the bot sizes orders off-chain against the live
  book (sdk/interface matching `simulate_swap`) and submits `swap`s
  through the generated SDK clients (sdk/rs) — the off-chain fill math
  and the swap instruction args/accounts must match the engine.
```

**Skip-globs** — generated / vendored / binary paths the file audit
never picks. One glob per line.

```txt
target/**
**/node_modules/**
Cargo.lock
**/pnpm-lock.yaml
**/package-lock.json
**/yarn.lock
**/*.gen.*
**/generated/**
**/idl/**
sdk/conformance/**
target/types/**
frontend/lib/data/*.json
frontend/public/**
**/*.png
**/*.svg
**/*.min.*
.audits/**
```
