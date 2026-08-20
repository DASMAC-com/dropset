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

Where roots overlap, the **more specific** root wins:
`docs/conventions/**` belongs to `agent-infra`, not to the broader
`docs`. A convention doc's failure modes are about the skills that
implement it — a rule nothing implements, a step naming a flag its
tool lacks — not about code it describes, which is what the `specs`
lens asks. `agent-infra` is weighted `med` despite a small blast
radius: a wrong instruction breaks no build, but it silently degrades
every session that reads it, which is an unusually wide failure mode
for a low-risk surface.

**Check committed command blocks against the real CLI**, including
external ones (`claude`, `gh`, `op`), not only this repo's tools. A
fenced block invoking a flag is **mechanically verifiable** — run the
CLI's `--help` and read it — and unlike prose it either works or does
not. This is worth its own pass because such a block is never
executed by CI and rarely by a reader, so it can be wrong from the day
it lands: the `paps` helper published in `local-integrations.md` probed
a `--list-sessions` flag that has never existed in any spelling, and
resumed by a name that `--resume` cannot take, and both survived until
somebody tried to wire it up. Treat a hedge in place of a check ("adjust
this to whatever form your CLI supports") as the finding itself.

```txt
program (solana-program, high): programs/dropset/src/**
sdk-math (rust-lib, high): sdk/math-core/src/**, sdk/interface/src/**
sdk-clients (gen-client, med): sdk/rs/src/**, sdk/ts/src/**, sdk/codama/**
frontend (web-app, med): frontend/**
decks (web-app, low): decks/**
tui (rust-lib, low): tui/**
docs (specs, med): docs/**
agent-infra (agent-infra, med): .claude/**, CLAUDE.md,
  docs/conventions/**
ci-infra (ci, low): .github/**, brand-assets/**, cfg/**, infra/**,
  keys/**, Makefile, Anchor.toml
maker-bot (rust-tool, low): bots/maker-bot/**
taker-bot (rust-tool, low): bots/taker-bot/**
util (rust-lib, low): util/**
indexer (rust-tool, low): indexer/**
feeds (rust-lib, low): feeds/**
fair-value (rust-lib, med): fair-value/**
market-data (rust-tool, low): market-data/**
db-schema (rust-lib, med): db-schema/**
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
  frontend/lib/hooks/useEclobQuote.ts + useRouterQuote.ts + useEclobSwap.ts)
  quotes and builds swaps through the SDK router's quoteEclob /
  quoteBestRoute (sdk/ts/src/router.ts), which wrap @dropset/sdk's
  simulateSwap — the WASM binding compiled from sdk/interface — so its
  off-chain fill math must compute identically to the on-chain engine; the
  conformance vectors (sdk/conformance) pin that parity. Both quoting paths
  go through the router, so the frontend hooks are lifecycle-only and must
  not re-derive routing decisions the router already owns. The declared
  platform fee is part of this seam: the quote (router quoteEclob) and the
  executor (frontend/lib/eclob/eclobSwap.ts) must clamp the configured rate to
  the market's max_platform_fee identically, or the displayed output stops
  equalling the fill. A separate drift to watch: the display-only float PnL
  re-implementation (frontend/lib/data/pnl.ts) that no conformance vector
  pins.
sdk-clients <-> DFlow: the router's aggregator leg (sdk/ts/src/dflow.ts)
  against DFlow's /quote, and the swap path (frontend/lib/dflow/) against
  /order. Two contracts to keep aligned — the platform-fee guard (declare a
  fee only when its ATA exists, since a declared fee eats slippage budget
  even uncollected) and platformFeeMode, which both callers pin explicitly
  rather than inheriting the server default. A best-route comparison is only
  honest while the aggregator quote is fetched net of the same fee the order
  will charge.
tui <-> sdk-math: the resting-book matcher surface (sdk/interface
  matching `resting_levels` / `BookLevel`) the TUI's order-book pane
  reconstructs depth from — the SDK normalizes a bid's quote leg to base
  at the level price, and the pane (tui/src/book.rs) de-scales by mint
  decimals, so the two must agree on the base-atom denomination.
maker-bot <-> program: the bot quotes and submits against the on-chain
  account/instruction contract (docs/interface.md) through the generated
  SDK clients (sdk/rs) — instruction args and accounts must match.
maker-bot <-> fair-value: the maker collects every source that answered
  into the engine's per-leg Candidates (fx / crypto_usdc / usdc_usd, plus
  static_usd) and reads the composed FairValue (fair, anchor, regime,
  basis, basis_age, health, uncertain, the basis/usdc breach flags, the
  basis_outlier flag, and the per-leg LegReports) from
  dropset-fair-value; the candidate collection and the result fields the
  killswitch and quoting path read must track the engine's model. Leg
  resolution itself is the engine's (median / agree-or-degrade /
  single-source, plus the dispersion gate), so the bot must not re-derive
  it. (The fair-value taker is a declared follow-up that shares this
  seam.)
taker-bot <-> program: the bot sizes orders off-chain against the live
  book (sdk/interface matching `simulate_swap`) and submits `swap`s
  through the generated SDK clients (sdk/rs) — the off-chain fill math
  and the swap instruction args/accounts must match the engine.
indexer <-> sdk-clients: the indexer extracts and decodes emit_cpi
  events through the shared dropset_sdk::events codec; its decoded event
  layouts and the 8-byte discriminators must track the IDL
  (sdk/idl/dropset.json).
feeds <-> indexer: the indexer runs on the feeds RPC-poll source and
  store sink (feeds/src/rpc.rs, feeds/src/store.rs), so the parity that
  used to be asserted between two copies is now a live dependency — the
  RawTx layout, the backfill walk's cursor discipline, and the
  StoreWriter contract (indexer/src/store.rs EventWriter) must stay
  consistent, and the indexer's resume position lives in the framework's
  feed_cursors. The load-bearing part is sink ORDER — AggregateSink
  (indexer/src/aggregate.rs) reads legs the store sink has already
  committed, and the two are ordered in indexer/src/bin/indexer.rs, so a
  reordering there silently stops takes being folded.
market-data <-> feeds: the collector app is the first consumer of the
  feeds framework — it now implements only StoreWriter
  (market-data/src/store.rs) and composes HttpClient /
  PgCursorStore / StoreSink / run around the shared Coinbase venue
  adapter (feeds/src/venues/coinbase.rs), which it no longer owns — so
  the trait signatures, the Batch/Cursor/caught_up contract, and the
  store sink's cursor-after-commit ordering must track the framework.
  That adapter is shared with maker-bot, so a change to it moves two
  consumers at once. Neither side owns
  DDL any more: feed_cursors and cex_prices are both defined in
  db-schema/migrations, so the seam is row types + queries against that
  migration. require_schema gates only the migration VERSION, so a
  column-shape mismatch inside a version still surfaces at query time.
secrets-enclave <-> feeds: a keyed venue declares its credential's
  canonical <provider>/<secret> name beside its adapter
  (feeds/src/venues/*.rs SECRET_NAME) and the provider
  (feeds/src/secrets.rs) maps that one name onto every store by prefix
  alone — env var, op:// reference, AWS secret id — so the three
  spellings cannot drift. The contract leaves the crate twice: consumers
  resolve by name (market-data/src/fx.rs), and the committed operator
  template (infra/localnet/secrets.local.env.example) restates each
  reference, pinned to the constants by
  market-data/tests/secrets_example.rs. That test enumerates its own
  roster, so a venue whose SECRET_NAME is absent from it is unguarded.
  The op stderr classifier is coupled to the CLI's prose and is the
  thing an op upgrade breaks first.
maker-bot <-> feeds: the maker bot is the first consumer of the feeds
  live (forward) sink — its price Sources are now the shared venue
  adapters (feeds/src/venues/coingecko.rs, coinmarketcap.rs,
  frankfurter.rs, kraken.rs) following the batched-poll convention —
  stated in feeds/src/venues/mod.rs's module docs and docs/data-feeds.md
  §4, and held by review rather than by a trait — plus two that
  deliberately sit OUTSIDE it: pyth.rs (whose FxQuote record
  carries a confidence half-width and a publish_time that a Quotes map's
  bare f64 cannot express) and coinbase.rs's CoinbaseTicker (keyed by a
  single product, so there is nothing to batch). Those two are the
  reason "every price Source is a batched quote venue" is NOT an
  invariant of this seam — the batched-poll convention does not reach
  them, and a change to Source reaches all six. It implements
  Source itself only for the logs-subscription fill socket bridged
  through ChannelSource (bots/maker-bot/src/fills.rs), driving both with
  run_until onto a ForwardSink its synchronous tick loop drains through
  the broadcast receiver (bots/maker-bot/src/tasks.rs) — so the
  Source/Sink signatures, the batched-poll convention, the Batch/caught_up
  contract, and the forward sink's bounded drop-to-latest policy must
  track the framework. The venue adapters are shared with market-data,
  so a change to one moves both consumers.
  The maker's leg tiering (FeedHub::legs) additionally couples adapter
  record SHAPE to fair-value semantics: pyth.rs's publish_time is what
  ages the FX anchor (not receipt time), so a change to that field's
  meaning silently changes when the weekend crypto-only regime engages.
sdk-clients <-> sdk-math: the TS market reader (sdk/ts/src/market.ts)
  reads the opaque Vault slab through the WASM binding built from
  sdk/interface (resting_book, over matching::resting_levels), so the
  slab decode and the book reconstruction have one implementation rather
  than a hand-mirror. The slab is still opaque to the IDL, so the
  generated client cannot catch drift here; what must now track
  sdk/interface is the binding's marshalling — the parallel price/size
  arrays market.ts zips back into levels — and the hand-built fixture
  offsets in sdk/ts/src/market.test.ts, which encode layout.rs's Vault
  stride in order to feed the engine.
frontend <-> sdk-clients: the trades tape decodes emit_cpi FillEvents
  through the hand-written TS events codec (sdk/ts/src/events.ts), which
  hand-copies the anchor event-CPI tag and the 8-byte discriminator from
  the Rust decoder (sdk/rs/src/events.rs) — Codama generates only the
  post-extraction body codec, so the generated client cannot catch
  envelope drift. The tag, the discriminators, and the trust guards (the
  emitting-program check and the failed-transaction refusal) must track
  the Rust original and the IDL.
db-schema <-> feeds: db-schema/migrations defines feed_cursors; the
  framework's PgCursorStore (feeds/src/store.rs) reads and upserts it
  without creating it, since feeds no longer runs sqlx::migrate!. The
  column set and the cursor-after-commit idempotent write must track the
  migration — a column the framework expects and the migration lacks
  fails at query time, not at startup. db-schema is a feeds
  dev-dependency, so feeds/tests/store_postgres.rs provisions the real
  migration rather than a copy of the table beside it — which is what
  makes this seam testable.
db-schema <-> indexer: db-schema/migrations defines the indexer's tables
  and the shared _sqlx_migrations state; indexer/src/store.rs holds the
  row types, reads, and ON CONFLICT writers against them and carries no
  migrator of its own. Its decoded-event columns must track both the
  migration and the event codec, and any migration it depends on has to
  land before the build that assumes it (Store::connect gates on
  require_schema).
db-schema <-> market-data: db-schema/migrations defines cex_prices,
  which the app's StoreWriter (market-data/src/store.rs) writes
  idempotently; the candle field set and the closed-bucket primary key
  must track the migration.
db-schema <-> grafana dashboards: db-schema/migrations owns the
  dropset_ro reader role and the SELECT grants behind it, which the
  provisioned datasource
  (market-data/grafana/provisioning/datasources) logs in as; the panel
  SQL in market-data/grafana/dashboards and the standalone queries in
  market-data/analytics read cex_prices and feed_cursors by column
  name, so a renamed or dropped column breaks a dashboard or a query
  silently — nothing compiles this SQL.
util <-> frontend: the human-price / atoms-ratio decimal-gap conversion
  is forked across languages — util/src/decimals.rs (which the TUI and
  the maker bot share) and the frontend's humanPrice
  (frontend/components/orderbook/format.ts). Two languages means the
  fork cannot be hoisted away, so the contract is the arithmetic
  association: both scale by 10^base / 10^quote as separate powers
  rather than the algebraically equal 10^(base-quote), whose single
  exponentiation is not correctly rounded — so the two panes would
  otherwise disagree by an ulp on the same level. Both sides now pin the
  grouping — util/src/decimals.rs and
  frontend/components/orderbook/format.test.ts — so a change to either
  fails its own suite rather than drifting silently. Note the probe that
  sizes the ratio is TS-only: humanPrice measures at 10^18 base atoms
  before scaling, and atoms_ratio_to_human takes an already-computed
  ratio, so that half of the frontend helper has no Rust counterpart.
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
keys/*.json
frontend/public/**
**/*.png
**/*.svg
**/*.min.*
.audits/**
```
