# cspell:word SIGTTIN

# Every target in this file is phony. Each `.PHONY:` declaration sits with its
# own rule rather than in one central sorted block: a single sorted list makes
# two branches that each add a target collide at the same line by
# construction — a merge conflict with no semantic content. Keep a new
# declaration adjacent to the rule it names.

.PHONY: all
all: lint test

# Nuke this worktree's heavy build artifacts to reclaim disk: the Rust
# target/ tree, every pnpm node_modules, and the Next.js .next build caches
# (all cheaply rebuilt). Leaves the committed generated trees (sdk/idl,
# sdk/ts, sdk/conformance) alone. Run at PR merge time (by review-pr) so a
# worktree that lingers before it is pruned doesn't keep its build tree.
.PHONY: clean
clean:
	cargo clean
	rm -rf node_modules frontend/node_modules decks/node_modules \
		sdk/ts/node_modules sdk/codama/node_modules
	rm -rf frontend/.next decks/.next sdk/ts/dist

# Local dev-server port allocation (the "reservation table"). There is no
# runtime enforcement — the OS fails-loud when a port is taken — so this
# comment is the single source of truth; pin each server to its slot and
# add a row when a new one lands.
#
# Every row below was re-checked against the compose file's published ports
# rather than inherited from the previous edit. Three had drifted: the
# explorer had moved off 3000 (so the collision warning it carried was
# describing a conflict that no longer exists), and both 3100 and 3200 read
# "(free)" while the explorer and Grafana were bound to them. Two services
# were missing outright. A table claiming a bound port is free is worse than
# no table at all, which is the argument for re-deriving it rather than
# patching the one row that prompted the look.
#   3000  frontend (make frontend, make frontend-localnet, make demo)
#   3100  explorer (make explorer) — serves 3000 in-container
#   3200  Grafana (make grafana, make collectors-up, make demo)
#   3300  decks (make decks)
#   5432  Postgres (make indexer-up, make collectors-up, make demo)
#   8080  indexer /v1 API (make indexer-up)
#   8899  solana-test-validator RPC (validator, not a web port)
#   8900  solana-test-validator WS (what frontend-localnet subscribes to)

# === Toolchain & prerequisite checks ===

# Required toolchain: anchor-cli 2.x, the Solana SBF toolchain, and a
# solana-cli / solana-test-validator on the 3.1 minor — matching the SDK's
# solana-client 3.1 so its `fetch` RpcClient and the local validator agree
# on wire/RPC (see sdk/rs/Cargo.toml). One prerequisite per tool keeps each
# recipe body small enough to wrap under the Makefile linter's length cap.
.PHONY: check-toolchain
check-toolchain: check-anchor check-sbf check-solana

.PHONY: check-anchor
check-anchor:
	@anchor --version | grep -q " 2\." \
		|| { echo "anchor-cli 2.x required"; exit 1; }

.PHONY: check-sbf
check-sbf:
	@command -v cargo-build-sbf >/dev/null \
		|| { echo "cargo build-sbf not found (install Solana toolchain)"; \
			exit 1; }

.PHONY: check-solana
check-solana:
	@solana --version | grep -q " 3\.1\." \
		|| { echo "solana-cli 3.1.x required"; exit 1; }
	@solana-test-validator --version | grep -q " 3\.1\." \
		|| { echo "solana-test-validator 3.1.x required"; exit 1; }

# Tooling for the non-on-chain subsystems, gated per-consumer (not folded
# into check-toolchain, which stays the on-chain build path) so a target only
# demands what it uses: the WASM binding build needs wasm-pack; the localnet
# Docker stack (explorer / indexer / bots) needs Docker with the compose v2
# plugin; the SDK and web dev servers need pnpm.
.PHONY: check-wasm
check-wasm:
	@command -v wasm-pack >/dev/null \
		|| { echo "wasm-pack not found (cargo install wasm-pack)"; \
			exit 1; }

.PHONY: check-docker
check-docker:
	@command -v docker >/dev/null \
		|| { echo "docker not found (install Docker)"; exit 1; }
	@docker compose version >/dev/null 2>&1 \
		|| { echo "docker compose v2 plugin required"; exit 1; }

.PHONY: check-pnpm
check-pnpm:
	@command -v pnpm >/dev/null \
		|| { echo "pnpm not found (npm i -g pnpm)"; exit 1; }

# `test-parity` runs the suite under nextest to match the CI job exactly
# (see that target). Every other test target uses plain `cargo test`, so
# this is the one place the runner is a hard prerequisite — name it rather
# than letting it surface as `no such command: nextest`.
.PHONY: check-nextest
check-nextest:
	@command -v cargo-nextest >/dev/null \
		|| { echo "cargo-nextest not found (cargo install cargo-nextest)"; \
			exit 1; }

# === On-chain program: build & keypair ===

# Materialize the program keypair into the (git-ignored) build dir from
# its canonical home, keys/AAAA.json, so anchor's build-time program-ID
# check — and the litesvm tests in programs/dropset/tests/common/mod.rs
# that read the file — see keypair == declare_id!. keys/AAAA.json is the
# single committed source; target/deploy/ is a pure build artifact.
.PHONY: program-keypair
program-keypair:
	mkdir -p target/deploy
	cp keys/AAAA.json target/deploy/dropset-keypair.json

.PHONY: program
program: check-toolchain program-keypair
	anchor keys sync && anchor build

# Build the program .so WITHOUT `admin-teardown` (the shape of the final
# immutable deploy). `anchor build`'s trailing args are forwarded to
# `cargo build-sbf`, so this rebuilds `dropset.so` feature-off. Split out
# from `test-no-teardown` so CI can cache this .so and skip the rebuild on
# a cache hit (see .github/workflows/test.yml).
.PHONY: program-no-teardown
program-no-teardown: check-toolchain program-keypair
	anchor build -- --no-default-features

# Build BOTH artifacts the Rust↔ASM parity tests need: the reference
# (feature-off) build stashed as `dropset_ref.so`, then the default asm
# build left in `dropset.so`. The reference build runs first because
# `anchor build` always writes `dropset.so`; the trailing default build
# restores the asm artifact every other test deploys.
.PHONY: program-parity
program-parity: check-toolchain program-keypair
	anchor build -- --no-default-features --features admin-teardown
	cp target/deploy/dropset.so target/deploy/dropset_ref.so
	anchor build

.PHONY: debugger
debugger: program
	anchor debugger

# === Program tests ===

.PHONY: test
test: program
	cargo test

# Feature-off coverage: build the program WITHOUT `admin-teardown` and
# assert every teardown instruction returns `TeardownDisabled` — only the
# feature-off-gated test target is run.
.PHONY: test-no-teardown
test-no-teardown: program-no-teardown
	cargo test --no-default-features --test teardown_disabled

# Rust↔ASM parity: deploy both artifacts and assert the assembly fast path
# (the default `dropset.so`) matches the reference kernel — identical stamp
# bytes and domain error codes.
#
# Mirrors the required `Tests (asm parity)` CI job in both respects that
# can hide a false green. The runner is `nextest`, the one CI uses, so a
# local pass and a CI pass mean the same thing. And
# `DROPSET_REQUIRE_PARITY_ORACLE` makes the suite refuse to *skip*: the
# `program-parity` prerequisite just built the oracle, so if the tests
# still cannot find it, that is a real failure and this target is where it
# should surface. A bare `cargo test --test asm_parity` leaves the
# variable unset and still skips, which is what makes an asm-only local
# run cheap.
.PHONY: test-parity
test-parity: check-nextest program-parity
	DROPSET_REQUIRE_PARITY_ORACLE=1 cargo nextest run --test asm_parity

# === SDK & codegen ===

# Regenerate the checked-in IDL from the program. Pin anchor-cli to the
# same anchor-next rev as the program crate (see install-anchor-v2) so
# the IDL-diff baseline doesn't drift — interface.md § SDK, CI discipline.
# Depend on program-keypair (like program: does) so the canonical
# keys/AAAA.json is staged before the build — otherwise anchor syncs
# declare_id! and the IDL `address` to a throwaway build keypair.
.PHONY: idl
idl: check-toolchain program-keypair
	anchor idl build -o sdk/idl/dropset.json

# Regenerate the TS + Rust clients from the checked-in IDL via Codama,
# then normalize the Rust output with `cargo fmt` so it lands in canonical
# form (clean under the rustfmt hook, reproducible by the SDK CI gate).
.PHONY: sdk
sdk: check-pnpm
	cd sdk/codama && pnpm install && pnpm generate
	cargo fmt -p dropset-sdk

# Build the WASM package for the TS client (requires wasm-pack:
# `cargo install wasm-pack`). Built over `dropset-interface`, whose `wasm`
# feature turns on `dropset-math-core`'s, so the one package exports both the
# `simulate_swap` binding and the `Price` codec bindings. Emits the glue
# straight into the TS SDK (sdk/ts/src/wasm) so `@dropset/sdk` can import it
# and the SDK CI type-checks against it; the `simulate` module wraps it.
.PHONY: wasm
wasm: check-wasm
	cd sdk/interface && wasm-pack build --target web \
		--out-dir ../ts/src/wasm --features wasm
	rm -f sdk/ts/src/wasm/.gitignore sdk/ts/src/wasm/package.json \
		sdk/ts/src/wasm/README.md sdk/ts/src/wasm/LICENSE

# Regenerate the checked-in conformance vectors from their generators.
# The `--write` flag makes each example write its canonical JSON straight
# to sdk/conformance/*.json (instead of stdout, avoiding a shell redirect),
# so the generators stay the single source of truth.
.PHONY: conformance-vectors
conformance-vectors:
	cargo run -p dropset-math-core --example gen_conformance -- --write
	cargo run -p dropset-math-core --example gen_quoting -- --write
	cargo run -p dropset-math-core --example gen_share -- --write
	cargo run -p dropset-interface --example gen_simulate_swap -- --write

# Freshness gate (CI): regenerate the vectors, then stage + diff against
# HEAD so a hand-edited or stale vector — and an added or removed one —
# all fail the gate, not just in-place edits (mirrors the IDL/clients gate
# in .github/workflows/sdk.yml). A generator / `Price` math change not
# followed by `make conformance-vectors` is exactly what this catches.
.PHONY: check-conformance-vectors
check-conformance-vectors: conformance-vectors
	git add -A -- sdk/conformance/
	git diff --cached --exit-code -- sdk/conformance/

# Run the SDK test suites: Rust (math-core + interface + dropset-sdk, incl.
# the conformance vectors) and the TS conformance check.
.PHONY: sdk-test
sdk-test: check-pnpm
	cargo test -p dropset-math-core -p dropset-interface -p dropset-sdk
	cd sdk/ts && pnpm test

# === Dev servers & control plane ===

# Build everything the running TUI shells out to, up front, so no in-TUI
# action stalls the demo with a compile mid-log: the on-chain program `.so`
# (the "Deploy" / "Bootstrap all" action publishes it — building it here with
# the same `anchor build --no-idl` deploy uses means that publish is a fast
# no-op) and the maker/taker bot binaries (the TUI spawns them as children —
# building them here means the spawned bot is always current with the just-
# built program's account layout, closing the stale-binary hazard where an
# old bot decodes a current market against a superseded `MarketHeader` size
# and dies with `SectorOverflow`). The `tui-prebuild-explorer` prerequisite
# warms the explorer's Docker image too, so after `make clean && make tui`
# every in-TUI command runs without building.
.PHONY: tui-prebuild
tui-prebuild: check-toolchain program-keypair tui-prebuild-explorer
	anchor keys sync && anchor build --no-idl
	cargo build -p dropset-maker-bot -p dropset-taker-bot

# Warm the local explorer's Docker image so the `docker compose up` the TUI
# runs in the background at launch is instant. `create` resolves the image the
# same way `up` will — pull the CI-published tag, or build from source as a
# fallback — but without starting it, and it is a no-op when the image is
# already cached (so it never fails offline on a warm cache). Guarded on a
# `docker` CLI so a no-Docker host (which falls back to the hosted explorer) is
# unaffected. A `tui-prebuild` prerequisite, so `make tui` warms it too.
# `--quiet-pull` drops the per-layer download progress cascade, keeping only the
# final per-image line (per docs/conventions/context-economy.md — that cascade
# is a top token sink when an agent runs this).
.PHONY: tui-prebuild-explorer
tui-prebuild-explorer:
	@if command -v docker >/dev/null 2>&1; then \
		docker compose -f infra/localnet/docker-compose.yml \
			create --quiet-pull explorer; \
	else echo "docker not found — skipping explorer image prebuild"; fi

# Localnet control-plane TUI. Spawns its own
# solana-test-validator (ledger in a temp dir), so it needs no running
# validator first — `tui-prebuild` handles the toolchain gate and warms every
# build the panel will need. Named `tui` (not `localnet`) because the same
# panel will later drive mainnet too. Passes `--bootstrap` by default so the
# panel auto-runs "Bootstrap all" once the localnet is up; override with
# `make tui TUI_ARGS=` for the step-by-step manual control plane.
TUI_ARGS ?= --bootstrap
.PHONY: tui
tui: tui-prebuild
	cargo run -p dropset-tui -- $(TUI_ARGS)

# Headless rent reclamation — the same teardown the TUI's "Teardown & reclaim"
# action runs, with no UI. Defaults to localnet; pass WALLET to override the
# admin keypair and ARGS for the rest (e.g. a real cluster, which prompts for
# confirmation — add --yes to skip that prompt in automation):
# `make teardown WALLET=~/admin.json ARGS="--rpc-url <url> --yes"`.
.PHONY: teardown
teardown:
	cargo run -p dropset-tui --bin dropset-teardown -- \
		$(if $(WALLET),--wallet $(WALLET)) $(ARGS)

# Open the default browser on a localhost port once it accepts connections, as
# a silenced background job: `$(call open-browser,<port>)`. Shared by the dev
# server targets so the wait-then-open block lives in one place.
define open-browser
@( until nc -z localhost $(1) 2>/dev/null; do sleep 0.2; done; \
	opener=$$(command -v open || command -v xdg-open) \
		&& $$opener http://localhost:$(1) ) >/dev/null 2>&1 &
endef

# Run next dev and open the browser once it's accepting connections.
.PHONY: frontend
frontend: check-pnpm
	cd frontend && pnpm install
	$(call open-browser,3000)
	cd frontend && pnpm dev

# Run the frontend against a local validator and open the browser once it's up:
# the localnet cluster + local RPC/WS, overriding the mainnet endpoints in
# .env.local (a process env var wins over .env files in Next). Assumes a
# validator is up with the markets seeded, which the `tui` control plane does;
# run `make tui` alongside this, or use `make demo` to launch both.
.PHONY: frontend-localnet
frontend-localnet: check-pnpm
	cd frontend && pnpm install
	$(call open-browser,3000)
	cd frontend && NEXT_PUBLIC_CLUSTER=localnet \
		NEXT_PUBLIC_RPC_URL=http://127.0.0.1:8899 \
		NEXT_PUBLIC_WS_URL=ws://127.0.0.1:8900 pnpm dev

# Run the decks deck dev server (port 3300, set in the dev script) and
# open the browser once it's accepting connections.
.PHONY: decks
decks: check-pnpm
	cd decks && pnpm install
	$(call open-browser,3300)
	cd decks && pnpm dev

# Production-build the decks (what CI gates on). The `prebuild` hook sources
# the brand assets and mirrors the remote ones, so a deck that references a
# missing image fails here rather than on a projector.
#
# The `.next` wipe is load-bearing: a warm cache from an earlier build or dev
# server intermittently fails this app-router-only package with "Cannot find
# module for page: /_document", after compiling and type-checking cleanly. CI
# always starts cold, so clearing first is also what makes a local run mean the
# same thing as the gate.
.PHONY: decks-build
decks-build: check-pnpm
	cd decks && pnpm install
	rm -rf decks/.next
	cd decks && pnpm build

# The whole localnet demo in one command (`make tui` already runs the
# control-plane TUI on its own localnet; this adds the web frontend): the TUI
# in the foreground (it spawns the validator and seeds the markets) plus the
# localnet frontend in the background, pointed at that validator. Quitting the
# TUI stops the frontend too; the frontend retries until the validator is up,
# so start order doesn't matter. The browser auto-opens because
# `frontend-localnet` opens it by default.
#
# Cleanup kills the background half's whole process group, not a `pkill -f
# "next dev"` pattern — the sub-make spawns `pnpm dev` which spawns
# `next dev`, so killing the job alone would orphan the grandchildren, but
# pattern-matching the command line also killed any *unrelated* next dev on
# the machine (another worktree's frontend, a separate project). `set -m`
# turns on job control so the background job becomes its own process-group
# leader, which makes `$!` a group id the trap can signal to reach every
# descendant and nothing outside this invocation. Job control goes back off
# (`set +m`) once that id is captured, so the foreground TUI keeps the
# terminal and signal handling it would have had without it.
#
# Job control is also why the background job's stdin is closed
# (`</dev/null`): it is no longer in the terminal's foreground process group,
# so any tty read would raise SIGTTIN and *stop* it — silently, since its
# output already goes to the log. A background dev server wants no stdin
# anyway; neither `pnpm` nor `next dev` needs it.
#
# The TUI runs on the alternate screen, so the background frontend's stdout
# would paint over it — `next dev`'s output is redirected to a log file, and
# the browser-opener job is silenced, so only the TUI draws to the terminal.
# The frontend log is written to $(FRONTEND_LOG) while the demo runs.
#
# Both halves are turnkey by default: `frontend-localnet` opens the browser and
# `tui` passes `--bootstrap`, so the TUI auto-runs "Bootstrap all" once the
# localnet is up. Wiping or tearing down does not re-bootstrap on its own —
# re-run it from the menu.
#
# The market-data collectors and Grafana come up too, on the same reasoning
# that already puts Grafana behind `collectors-up`: a demo that shows the book
# but not the prices feeding it is showing half the system. Grafana opens on
# http://localhost:3200 alongside the frontend's own tab.
#
# That is every collector, keyed venues included — `collectors-up` gates the
# keyed half on the secrets enclave itself, so this target needs no FX step of
# its own and no credential handling.
#
# `collectors-up` runs FIRST and synchronously, before the TUI takes the
# alternate screen — it is the slow, chatty step (it builds the Rust images on
# a cold tree) and the one that fails when Docker is not running, so both its
# progress and its errors belong on a plain terminal rather than under a TUI.
# It is also why this target now needs `check-docker`.
#
# On exit the trap stops the background frontend and nothing else, because
# nothing else is this recipe's to stop. Everything the TUI owns tears itself
# down when the TUI quits: `Drop for Validator` kills the validator child and
# its temp ledger, `Drop for BotManager` kills and reaps every maker/taker
# child, and `Drop for App` stops the explorer container. That holds for
# Ctrl-C as well as `q` — the TUI runs in raw mode, so crossterm delivers
# Ctrl-C as a key event rather than a signal, and it routes to the same clean
# quit that runs those destructors instead of bypassing them.
#
# What is left when the demo ends is therefore the collectors it started —
# every one, or the keyless four if the enclave gate declined — plus
# Grafana. They are deliberately left running: they are a standing
# recording service, not a demo fixture. Every minute they are down is a hole
# in the stored history that no later run can backfill at tick resolution, and
# `restart: unless-stopped` already says they are meant to outlive whatever
# started them. Quitting the demo should cost the demo, not the data.
#
# So stopping the collectors is always an explicit act: `make
# collectors-down` for the collectors alone, or `make clean-docker` when
# tearing the whole localnet down. The browser tabs are the operator's to
# close, and so is the decision to stop recording.
#
# This is why the target does not try to stop only what it started. It has no
# way to tell — `collectors-up` is idempotent, so a demo cannot distinguish
# the collectors it launched from the ones already running — and under the
# rule above it does not need to: the answer is the same either way.
#
# `DEMO_CLEANUP` still disarms the trap (`trap -`) as its first act. INT and
# EXIT are both trapped and the shell runs the EXIT handler after the INT one,
# so without it the handler runs twice on Ctrl-C. That is harmless for a bare
# `kill` and it is correct hygiene regardless.
FRONTEND_LOG ?= /tmp/dropset-frontend.log
# The background half and its cleanup, hoisted into variables the way `FX_UP`
# is: the Makefile linter caps a recipe body at 5 lines and counts
# continuation lines toward it, and the body already sat exactly on that cap
# before these additions.
DEMO_FRONTEND = $(MAKE) --no-print-directory frontend-localnet \
	>$(FRONTEND_LOG) 2>&1 </dev/null
DEMO_CLEANUP = trap - INT TERM EXIT; kill -TERM -$$group 2>/dev/null
.PHONY: demo
demo: check-docker
	@echo "frontend logs → $(FRONTEND_LOG) (kept off the TUI screen)"
	$(MAKE) --no-print-directory collectors-up KEYED_PAUSE=1
	$(call open-browser,3200)
	@set -m; $(DEMO_FRONTEND) & group=$$!; set +m; \
	trap '$(DEMO_CLEANUP)' INT TERM EXIT; $(MAKE) --no-print-directory tui

# === Localnet Docker stacks ===

# Localnet Docker stack: the local Solana Explorer (infra/localnet). The
# dropset-tui control plane manages this automatically; these targets drive it
# by hand. `up` pulls the CI-published image (or builds from source as a
# fallback, or reuses a cached one) per the compose `pull_policy`; later runs
# reuse the cache. Pin or bump the version via the `image:` tag in
# docker-compose.yml (with EXPLORER_REF in explorer.Dockerfile). Every `up`
# target here passes `--quiet-pull` to drop the per-layer progress cascade
# (per docs/conventions/context-economy.md).
.PHONY: explorer
explorer: check-docker
	docker compose -f infra/localnet/docker-compose.yml \
		up -d --quiet-pull explorer
.PHONY: explorer-down
explorer-down: check-docker
	docker compose -f infra/localnet/docker-compose.yml \
		rm -sf explorer

# Nuke the localnet Docker state for a cold start, KEEPING the recorded market
# data: stop and remove every stack container (explorer, migrate, indexer,
# collectors, grafana, bots, postgres — both the `taker` and `fx` profiles
# included), drop any orphans, remove the untagged images compose built
# locally (migrate + indexer + collectors + bots), and prune the build cache.
# The named postgres volume survives; `clean-docker-volume` below is the only
# target that destroys it.
#
# **Preserving the volume is the default because a reflexive clean must never
# cost history.** A CEX backfill window is finite — Kraken's keyless OHLC
# reaches ~720 candles per interval — and the spot tick stream cannot be
# backfilled at any depth, so a dropped volume is unrecoverable data rather
# than a rebuild. This target used to pass `-v` and take the volume with it;
# the split exists so the destructive half has to be named on purpose.
#
# Both profiles are named explicitly, and that is a fix rather than a
# flourish: `down` only reaches services in the profiles it is given, so the
# previous lone `--profile taker` left the three keyed FX collectors running
# behind what called itself a full reset.
#
# The container removal takes each container's logs with it. `--rmi local`
# removes only untagged local builds, so the tagged explorer image
# (dasmac/dropset-localnet-explorer, pulled or built) and pulled base images
# (e.g. postgres) survive it — `tui-prebuild` then reuses the cached explorer;
# `docker rmi` it by name (or `docker system prune`) for a fully cold explorer.
# Note the `docker builder prune -f` is host-wide — it clears every project's
# build cache on this machine, not only this stack's. It is also what reclaims
# a transferred build context, which is why it stays on the volume-preserving
# path rather than moving to the destructive one.
#
# To stop one app's own services and leave the rest up, use the per-service
# targets (`indexer-down` / `collectors-down` / `explorer-down`) — each is
# scoped precisely because `postgres` is shared.
CLEAN_DOWN = docker compose -f infra/localnet/docker-compose.yml \
	--profile taker --profile fx down --rmi local --remove-orphans
.PHONY: clean-docker
clean-docker: check-docker
	$(CLEAN_DOWN)
	docker builder prune -f

# `clean-docker` plus the named postgres volume — every recorded candle and
# spot tick on this machine. Deliberately unreachable from any other target:
# what it drops cannot be re-fetched (see above), so destroying it is its own
# decision, never the tail of a cleanup. Reach for it when starting the schema
# over is the actual goal.
.PHONY: clean-docker-volume
clean-docker-volume: check-docker
	$(CLEAN_DOWN) -v
	docker builder prune -f

# Localnet indexer stack: the shared Postgres + the one-shot schema migration
# + the event indexer worker + the /v1 API (infra/localnet, docs/indexer.md
# §8). Needs a running validator (the tui or a host-run
# solana-test-validator) as the live event source. First run builds the Rust
# images (slow); later runs reuse the cargo-chef dependency cache. The /v1
# surface comes up on http://localhost:8080.
#
# `migrate` is named explicitly even though compose would pull it in as a
# dependency, so `up` reports its result rather than hiding a failed schema
# step behind a service that then cannot start.
#
# `indexer-down` stops only the indexer's own services and leaves `postgres`
# running. Postgres is now shared infrastructure — the collectors use the same
# container (and `coinbase` is `restart: unless-stopped`, so it would
# error-loop against a removed database), so no per-app `down` target may take
# it away. `clean-docker` is what stops the whole data plane, and
# `clean-docker-volume` is the only thing that discards the volume;
# `docker compose ... stop postgres` covers the ad-hoc case.
#
# One first-contact snag: `up` builds only when an image is ABSENT, so a
# worktree holding a pre-consolidation indexer image reuses it, and that image
# still runs its own migrator against what is now the shared database —
# failing with a sqlx checksum mismatch rather than the fence's actionable
# text. Run `make clean-docker` once, or `up --build`, after picking this up.
.PHONY: indexer-up
indexer-up: check-docker
	docker compose -f infra/localnet/docker-compose.yml \
		up -d --quiet-pull postgres migrate indexer indexer-api
.PHONY: indexer-down
indexer-down: check-docker
	docker compose -f infra/localnet/docker-compose.yml \
		rm -sf indexer indexer-api

# Market-data collectors: the shared Postgres + the schema migration + every
# **keyless** feed (docs/data-feeds.md §5, §8). Independent of the
# validator — these poll public REST APIs — so they run with or without a
# localnet up, and they share the one `dropset` database with the indexer.
# Stopping them leaves the recorded history on the volume.
#
# Four keyless feeds, across both tiers. Candles into `cex_prices`: the Coinbase
# reference price. Spot ticks into `spot_ticks`: the Coinbase ticker (the prints
# between candle closes), Kraken (batched peg truth — a real market print of
# `USDC/USD`), and Pyth Hermes (batched FX with a published confidence).
#
# The three credentialed venues (`KEYED_UP` below) come up in the same breath,
# behind a gate on the secrets enclave existing. There is deliberately no
# separate target for them: every service here is a market-data collector, and
# which ones happen to need an API key is a property of the vendor rather than
# a distinction worth spending a target name on — the old `fx-` prefix said
# "foreign exchange" while actually meaning "needs credentials", and would have
# aged badly the first time a keyed venue published something other than FX.
#
# The gate is what keeps this target working on a machine with no credentials
# at all: the keyless four come up regardless, and a keyed half that cannot
# start warns loudly without failing the run (see `KEYED_WARN` below for why
# it is loud and why it is non-fatal). A machine holding its keys as plain
# exported environment variables rather than in the enclave has no target
# for them any more — it runs the `--profile fx` compose invocation by
# hand. The gate deliberately does not guess.
#
# Grafana comes up with them, because a collector you cannot see is a
# collector you cannot verify: the point of starting a feed is watching what
# it writes. `collectors-down` stops it too — it has nothing to show once the
# feeds are gone — while `make grafana` runs it against the history already
# on the volume with no collector at all.
#
# `--build` is not optional, and the failure it prevents is genuinely
# confusing. A bare `up -d` builds only when an image is MISSING, so it
# silently reuses whatever image already carries the service's name — and
# since the compose project name is shared across worktrees, that is whichever
# branch last built it. A code change then appears to do nothing. Worse, it
# applies to `migrate` too: a stale migrate image embeds another branch's
# migration set, finds the database already at its own high-water mark, and
# exits 0 having applied nothing — so a collector starts against a schema
# nobody in this tree wrote. `cargo chef` caches the dependency graph, so on an
# unchanged tree this costs a cache check rather than a build.
.PHONY: collectors-up
collectors-up: check-docker
	docker compose -f infra/localnet/docker-compose.yml \
		up -d --build --quiet-pull postgres migrate coinbase coinbase-ticker \
		kraken pyth grafana
	@$(KEYED_UP)
.PHONY: collectors-down
collectors-down: check-docker
	docker compose -f infra/localnet/docker-compose.yml --profile fx \
		rm -sf coinbase coinbase-ticker kraken pyth grafana oanda \
		twelvedata alphavantage

# Grafana alone, on http://localhost:3200, serving the provisioned
# market-data ingestion dashboard (market-data/grafana/, docs/data-feeds.md
# §8). Useful without any collector running: the dashboards read whatever
# history is on the volume, so this is also how you look at yesterday's
# candles. Add `?kiosk` to the URL for a chrome-free screenshare.
#
# The dashboard JSON and the provisioning tree are bind-mounted, so editing
# a committed dashboard shows up in the browser within the provider's refresh
# interval — no restart, no rebuild. `grafana-down` leaves `postgres` alone,
# like every other per-app target here.
.PHONY: grafana
grafana: check-docker
	docker compose -f infra/localnet/docker-compose.yml \
		up -d --quiet-pull postgres migrate grafana
.PHONY: grafana-down
grafana-down: check-docker
	docker compose -f infra/localnet/docker-compose.yml \
		rm -sf grafana

# The credentialed half of `collectors-up` — the free-tier FX venues
# (docs/data-feeds.md §9, "The free-tier FX roster"). No target of its own:
# `collectors-up` runs `KEYED_UP` behind the enclave gate, and
# `collectors-down` removes these three alongside the keyless four. Each
# service reads its credential from the environment and refuses to start
# without one, naming the variable it wanted.
#
# Each takes a **roster** (`FX_PRODUCT_IDS`), so one service per venue covers
# every pair rather than one service per pair. The two metered venues widen
# their own poll interval to keep the roster inside their daily quota and log
# the effective cadence — adding a pair cannot silently push the account over
# its limit.
#
# Credentials come from the local secrets enclave (docs/data-feeds.md §12):
# `op run` resolves the `op://` references in the git-ignored
# infra/localnet/secrets.local.env and exports the values into the compose
# invocation's environment. The containers themselves have no `op` and no
# vault access — they are handed resolved values, which is exactly the shape
# the hosted deploy has with Secrets Manager.
#
# The enclave is optional, hence the gate rather than a hard dependency: a
# checkout without 1Password access still gets the keyless four. (No CI
# workflow runs any collector target, so CI is not a consumer of that
# path — it is there for a fresh checkout.) `op run` resolves eagerly, so a
# bad reference stops the stack here instead of starting a collector that
# 401s a minute later.
#
# The enclave file is `include`d as well as passed to `op run`, because the
# two need different things from it. `op run` injects it into the *child*
# process, so a `DROPSET_OP_ACCOUNT` line inside it never reaches `op` itself
# — and on a machine signed in to more than one 1Password account, `op` fails
# at client init before it reads any reference. Including the file makes that
# value a make variable, so it can be passed as the `--account` flag it has to
# be. The flag is conditional: a single-account machine deletes the line, and
# an empty `--account` would swallow the next argument.
#
# Set FX_PRODUCT_IDS to collect pairs other than the default roster. It is a
# comma-separated list of canonical BASE-QUOTE ids; each venue's own spelling is
# derived from them. It replaces the singular FX_PRODUCT_ID, which the compose
# file no longer reads — a roster of one is just a list with one entry.
#
# **A pinned spelling (`CANONICAL=VENUE`) must go in a per-venue variable**, not
# in the shared one: OANDA_PRODUCT_IDS, TWELVEDATA_PRODUCT_IDS, or
# ALPHAVANTAGE_PRODUCT_IDS, each falling back to FX_PRODUCT_IDS when unset. A
# pin is inherently venue-specific — the three vendors spell one pair three
# ways — so putting one in the shared variable would hand a spelling meant for
# one venue to all three. Alpha Vantage derives no single symbol at all (it
# takes the two legs separately) and refuses a pin outright rather than ignoring
# it, so a pin in the shared variable would stop that service at startup over a
# setting aimed at a different venue.
FX_ENV = infra/localnet/secrets.local.env
-include $(FX_ENV)
OP_ACCT = $(if $(DROPSET_OP_ACCOUNT),--account '$(DROPSET_OP_ACCOUNT)',)
# `--build` for the same reason as `collectors-up` above: without it these
# services keep whichever image another worktree last built.
FX_UP = docker compose -f infra/localnet/docker-compose.yml \
	--profile fx up -d --build --quiet-pull postgres migrate oanda \
	twelvedata alphavantage
# A keyed bring-up that does not happen is LOUD, and it does not abort the
# run. Loud because the failure is otherwise invisible in exactly the way that
# matters: the keyless feeds are up, Grafana is green, and the three keyed
# venues quietly record nothing — the same no-data-reads-as-healthy trap the
# dashboards have. A one-line notice scrolls past under a compose build; a
# banner does not.
#
# Non-fatal because this is now the default bring-up and sits on the `demo`
# path: a 1Password hiccup should cost the keyed venues, not the whole stack.
# The keyless four are unaffected by anything that goes wrong here, so the
# useful thing to do is start them, say plainly what is missing, and continue.
# The alternative — exit non-zero on a bad `op://` reference, treating it as
# the config bug it usually is — was the road not taken; it would have made
# `make demo` fail on a credential problem that has nothing to do with the
# demo.
#
# Two failure modes route through the banner: no enclave file at all, and a
# keyed bring-up that exits non-zero — `op` missing or signed out, a bad
# reference, a wrong account, or the compose build itself failing. `op run`
# exits with its CHILD's status, so the reason names the symptom rather than
# diagnosing a cause it cannot actually distinguish; the underlying error
# prints immediately above the banner.
#
# A third mode is NOT covered, and the gap is recorded here rather than
# papered over. `docker compose up -d` returns as soon as the containers
# start, and the compose file passes each credential as `${VAR:-}` rather
# than the required form — so an enclave that resolves but is MISSING one of
# the three references starts a collector that names its variable and dies.
# That exits 0 and prints no banner.
#
# That `${VAR:-}` is not an unforced choice: the explorer-image workflow runs
# `docker compose config` over this file with no environment at all, and the
# required form fails that parse for every service (docker-compose.yml, the
# alphavantage comment). So the fix is not to tighten it.
#
# `--wait` is the candidate — it was measured to exit 0 here despite the
# one-shot `migrate`, which `depends_on: service_completed_successfully`
# covers — but it is not adopted yet: these services are
# `restart: unless-stopped`, so a crash-looping container can read as
# running, and an unbounded `--wait` would hang `make demo` rather than warn
# it. Adopting it wants a `--wait-timeout` and its own verification.
#
# `KEYED_WARN` is not self-contained: it reads a `reason` shell variable its
# caller must set in the same shell. `KEYED_UP` below is that caller.
# `demo` passes `KEYED_PAUSE=1`, which holds the terminal after the banner
# until the operator acknowledges it. That target is the whole reason the
# banner has to be loud and the only place it is not: `demo` opens a Grafana
# tab on the next line — taking window focus — and the TUI takes the
# alternate screen on the line after, so the warning is behind a green
# dashboard within a second of printing and does not resurface until quit.
# Every other caller prints and carries on.
#
# Gated on stdin being a tty as well, so a script, a CI job or a backgrounded
# run never blocks on a prompt nobody is there to answer.
KEYED_PAUSE =
KEYED_WARN = printf '\n%s\n%s\n%s\n%s\n%s\n\n' \
	'=====================================================================' \
	'  WARNING — the keyed venues are NOT running.' \
	"  Reason: $$reason" \
	'  OANDA, Twelve Data and Alpha Vantage will record nothing.' \
	'====================================================================='; \
	if [ -n '$(KEYED_PAUSE)' ] && [ -t 0 ]; then \
	printf '  press enter to continue… '; read -r _; printf '\n'; fi
KEYED_UP = if [ ! -f "$(FX_ENV)" ]; then \
	reason='no $(FX_ENV) (cp its .example)'; $(KEYED_WARN); \
	elif ! op run $(OP_ACCT) --env-file=$(FX_ENV) -- $(FX_UP); then \
	reason='the keyed bring-up failed (see the error above)'; $(KEYED_WARN); fi

# Localnet bot stack: the maker bot (infra/localnet). It signs with the repo
# keys/ keypairs and reaches the host-run validator at
# host.docker.internal:8899. Needs a running validator with the market
# bootstrapped and seeded (the tui control plane). First run builds the Rust
# image (slow); later runs reuse the cargo-chef dependency cache. The taker is
# opt-in (`taker-up`), never started here — the demo market stays quiet until
# an operator asks for organic flow.
.PHONY: bots-up
bots-up: check-docker
	docker compose -f infra/localnet/docker-compose.yml \
		up -d --quiet-pull maker-bot
.PHONY: bots-down
bots-down: check-docker
	docker compose -f infra/localnet/docker-compose.yml \
		rm -sf maker-bot taker-bot

# Opt-in localnet flow: start / stop the benign stochastic taker so the seeded
# books move and the maker takes fills. Off by default (gated behind the compose
# `taker` profile); flip it on for a walkthrough, off to leave the market quiet.
.PHONY: taker-up
taker-up: check-docker
	docker compose -f infra/localnet/docker-compose.yml \
		--profile taker up -d --quiet-pull taker-bot
.PHONY: taker-down
taker-down: check-docker
	docker compose -f infra/localnet/docker-compose.yml \
		rm -sf taker-bot

# === Repo tooling ===

# Run the lint hook set over every file in the working tree. Note this is NOT
# `pre-commit run --all-files`: that enumerates files with `git ls-files`, so a
# new file that has never been `git add`ed is invisible to every hook and the
# run passes without opening it — while CI, whose checkout has the file
# committed and therefore tracked, fails on it. The tool builds the file list as
# tracked + untracked-but-not-ignored, making the local set a superset of CI's.
# The rationale in full (including why the chunked cspell batches in the output
# are not the culprit) is in the tool's module docstring — read it before
# swapping this back to `--all-files`. Pass a single hook after `--`, e.g.
# `python3 .claude/tools/lint_paths.py -- cspell`.
.PHONY: lint
lint:
	python3 .claude/tools/lint_paths.py

# Check the root .dockerignore still bounds the Docker build context. Every
# Rust service in infra/localnet/docker-compose.yml builds with `context:
# '../..'` and `COPY . .`, so without that file each build ships the whole
# checkout to the daemon. Both figures below are the base checkout: 90.9 GB
# across 868,258 files without this file, 7.0 MB across 630 with it. (A
# worktree measures a few files more or fewer — it carries different
# untracked paths — so pin a quoted count to one checkout before comparing.)
# Run by the `docker-context` pre-commit
# hook too, so `make lint` covers it; this target is for looking at the number
# directly. `ARGS=--measure` reports the size and which trees were pruned.
.PHONY: docker-context
docker-context:
	python3 .claude/tools/docker_context.py $(ARGS)

# Report committed guard hooks that no settings file wires. A script under
# .claude/hooks/ does nothing until a PreToolUse entry points at it, and the
# wiring is deliberately uncommitted (both settings files are git-ignored), so
# a guard can sit committed and inert indefinitely — as two of three did until
# 2026-08-14. CI cannot check this: it has no settings to inspect, which is why
# it is a make target `housekeeping` drives from the base repo instead. Reports
# only; wiring a guard is the operator's call. Exits 0 clean / 1 unwired /
# 2 if the scan itself could not run.
.PHONY: hook-wiring
hook-wiring:
	python3 .claude/tools/hook_wiring.py $(ARGS)

# Check that every rendered region in a skill file still matches its single
# source under .claude/shared/. Changing a shared convention otherwise means
# editing every skill that restates it and remembering to look — a hand-sync
# tax paid on every meta batch, and one batch updated the same rule in three
# separate files. Unlike `hook-wiring` this IS checkable in CI: it reads only
# committed files. Exits 0 in sync / 1 stale (run `render-skills` and commit) /
# 2 on a malformed or dangling marker.
.PHONY: render-check
render-check:
	python3 .claude/tools/render_skills.py --check

# Refill every rendered region from its source, in place.
.PHONY: render-skills
render-skills:
	python3 .claude/tools/render_skills.py --write

# Mirror every Grafana dashboard query into market-data/grafana/sql/, so a
# change to a panel's SQL shows up as a one-line diff instead of a
# one-character edit inside a 1,500-character JSON string. The JSON stays the
# source of truth; this only reads it. Regenerate and commit the result
# alongside any dashboard change.
.PHONY: dashboard-sql
dashboard-sql:
	python3 .claude/tools/dashboard_sql.py extract

# Fail if the mirror has drifted from the dashboards. Reads only committed
# files, so it runs in CI (like `render-check`, and unlike `hook-wiring`).
# Also enforces two guards that Grafana itself fails silently on: a nested
# paren inside a macro argument (Grafana truncates it at the first closing
# paren and the query never interpolates), and a `:regex`-formatted template
# variable inside a single-quoted SQL literal (the regex formatter does not
# escape quotes).
.PHONY: dashboard-sql-check
dashboard-sql-check:
	python3 .claude/tools/dashboard_sql.py check

# Lint the extracted SQL as Postgres, after substituting the Grafana macros and
# template variables for typed stand-ins. Deliberately narrow — see
# cfg/sqlfluff-dashboards.cfg for why the full rule set is wrong (and unsafe)
# for SQL whose column names Grafana dictates. Needs sqlfluff IMPORTABLE by the
# invoking interpreter (the tool runs `sys.executable -m sqlfluff`), not merely
# an executable on PATH — without it the run fails with "No module named
# sqlfluff", which reads like a lint finding. The pre-commit hook installs it
# into its own env, so prefer `make lint` locally.
.PHONY: dashboard-sql-lint
dashboard-sql-lint:
	python3 .claude/tools/dashboard_sql.py lint

# Account for where a session's tokens went (the deterministic core of the
# session-metrics skill). A stdlib-only Python skill-tool under .claude/tools/
# (not a Cargo workspace member — see CLAUDE.md "Skill tooling"). Resolves the
# transcript itself from the Claude home (CLAUDE_CONFIG_DIR or ~/.claude) and
# the working-directory project slug, reads it in its own process, and prints
# a compact ranked-sink summary. Pass the session id:
# `make session-metrics SESSION=<uuid>` (add ARGS=--json for JSON).
.PHONY: session-metrics
session-metrics:
	python3 .claude/tools/session_metrics.py --session-id $(SESSION) $(ARGS)

# Run every Python skill-tool's unit tests (stdlib `unittest`, no third-party
# dep). Covers the `.claude/tools/` skill helpers (tests live under
# `.claude/tools/tests/`) plus the Python under `.claude/scripts/` (the iTerm
# tab-ordering logic). The tools tests import their modules bare (`import
# firm_core`), so discovery runs with the tests dir as start and the tool home
# as top-level (`-t`) to keep those imports resolving. Run in CI's lint job.
#
# `.claude/hooks` is deliberately NOT a discovery root — the guards are hook
# entry points, not unittest modules. Their in-file case tables (150+ cases
# across four parsers) reach this target through
# `.claude/tools/tests/test_hook_self_tests.py`, which shells each guard's
# `--self-test`. Before that they ran only when a human typed the flag by hand,
# which left the most security-sensitive files here the least covered.
.PHONY: tools-tests
tools-tests:
	python3 -m unittest discover \
		-s .claude/tools/tests -t .claude/tools -p 'test_*.py'
	python3 -m unittest discover -s .claude/scripts -p 'test_*.py'

# https://github.com/solana-foundation/anchor/tree/anchor-next/lang-v2
.PHONY: install-anchor-v2
install-anchor-v2:
	CARGO_PROFILE_RELEASE_LTO=off cargo install \
		--git https://github.com/solana-foundation/anchor.git \
		--branch anchor-next \
		anchor-cli --force
