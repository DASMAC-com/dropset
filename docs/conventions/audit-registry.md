# Audit registry

`audit` reads its coverage map from here — the **subsystems**
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
indexer (rust-tool, low): indexer/**
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
sdk-math <-> frontend: the frontend's eCLOB route (frontend/lib/eclob/,
  frontend/lib/hooks/useEclobQuote.ts + useEclobSwap.ts) quotes and builds
  swaps via @dropset/sdk's simulateSwap — the WASM binding compiled from
  sdk/interface — so its off-chain fill math must compute identically to
  the on-chain engine; the conformance vectors (sdk/conformance) pin that
  parity. The best-route path still quotes via DFlow's API
  (frontend/lib/dflow/). A separate drift to watch: the display-only float
  PnL re-implementation (frontend/lib/data/pnl.ts) that no conformance
  vector pins.
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
indexer <-> sdk-clients: the indexer extracts and decodes emit_cpi
  events through the shared dropset_sdk::events codec; its decoded event
  layouts and the 8-byte discriminators must track the IDL
  (sdk/idl/dropset.json).
sdk-clients <-> sdk-math: the TS market reader (sdk/ts/src/market.ts)
  hand-decodes the opaque Vault slab and reconstructs the resting book,
  mirroring the on-chain byte layout (sdk/interface/src/layout.rs) and the
  Rust matcher (resting_levels / BookLevel) — the slab is opaque to the
  IDL, so the generated client can't catch drift; market.ts's byte offsets
  and level materialization must track layout.rs / matching.rs.
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
sdk/ts/src/wasm/**
sdk/conformance/**
target/types/**
frontend/lib/data/*.json
frontend/public/**
**/*.png
**/*.svg
**/*.min.*
.audits/**
```
