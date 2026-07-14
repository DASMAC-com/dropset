.PHONY: all
.PHONY: bots-down
.PHONY: bots-up
.PHONY: check-anchor
.PHONY: check-conformance-vectors
.PHONY: check-docker
.PHONY: check-pnpm
.PHONY: check-sbf
.PHONY: check-solana
.PHONY: check-toolchain
.PHONY: check-wasm
.PHONY: clean
.PHONY: clean-docker
.PHONY: conformance-vectors
.PHONY: debugger
.PHONY: decks
.PHONY: explorer
.PHONY: explorer-down
.PHONY: frontend
.PHONY: frontend-localnet
.PHONY: idl
.PHONY: indexer-down
.PHONY: indexer-up
.PHONY: install-anchor-v2
.PHONY: lint
.PHONY: localnet
.PHONY: program
.PHONY: program-keypair
.PHONY: program-no-teardown
.PHONY: program-parity
.PHONY: sdk
.PHONY: sdk-test
.PHONY: session-metrics
.PHONY: taker-down
.PHONY: taker-up
.PHONY: teardown
.PHONY: test
.PHONY: test-no-teardown
.PHONY: test-parity
.PHONY: tools-tests
.PHONY: tui
.PHONY: tui-prebuild
.PHONY: wasm

all: lint test
# Nuke this worktree's heavy build artifacts to reclaim disk: the Rust
# target/ tree, every pnpm node_modules, and the Next.js .next build caches
# (all cheaply rebuilt). Leaves the committed generated trees (sdk/idl,
# sdk/ts, sdk/conformance) alone. Run at PR merge time (by review-pr) so a
# worktree that lingers before it is pruned doesn't keep its build tree.
clean:
	cargo clean
	rm -rf node_modules frontend/node_modules decks/node_modules \
		sdk/ts/node_modules sdk/codama/node_modules
	rm -rf frontend/.next decks/.next sdk/ts/dist

# Local dev-server port allocation (the "reservation table"). There is no
# runtime enforcement — the OS fails-loud when a port is taken — so this
# comment is the single source of truth; pin each server to its slot and
# add a row when a new one lands.
#   3000  frontend (make frontend) AND explorer (make explorer) — these
#         two collide if run together; keep only one up at a time
#   3100  (free)
#   3200  (free)
#   3300  decks (make decks)
#   8080  indexer /v1 API (make indexer-up)
#   8899  solana-test-validator RPC (validator, not a web port)

# === Toolchain & prerequisite checks ===

# Required toolchain: anchor-cli 2.x, the Solana SBF toolchain, and a
# solana-cli / solana-test-validator on the 3.1 minor — matching the SDK's
# solana-client 3.1 so its `fetch` RpcClient and the local validator agree
# on wire/RPC (see sdk/rs/Cargo.toml). One prerequisite per tool keeps each
# recipe body small enough to wrap under the Makefile linter's length cap.
check-toolchain: check-anchor check-sbf check-solana

check-anchor:
	@anchor --version | grep -q " 2\." \
		|| { echo "anchor-cli 2.x required"; exit 1; }

check-sbf:
	@command -v cargo-build-sbf >/dev/null \
		|| { echo "cargo build-sbf not found (install Solana toolchain)"; \
			exit 1; }

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
check-wasm:
	@command -v wasm-pack >/dev/null \
		|| { echo "wasm-pack not found (cargo install wasm-pack)"; \
			exit 1; }

check-docker:
	@command -v docker >/dev/null \
		|| { echo "docker not found (install Docker)"; exit 1; }
	@docker compose version >/dev/null 2>&1 \
		|| { echo "docker compose v2 plugin required"; exit 1; }

check-pnpm:
	@command -v pnpm >/dev/null \
		|| { echo "pnpm not found (npm i -g pnpm)"; exit 1; }

# === On-chain program: build & keypair ===

# Materialize the program keypair into the (git-ignored) build dir from
# its canonical home, keys/AAAA.json, so anchor's build-time program-ID
# check — and the litesvm tests in programs/dropset/tests/common/mod.rs
# that read the file — see keypair == declare_id!. keys/AAAA.json is the
# single committed source; target/deploy/ is a pure build artifact.
program-keypair:
	mkdir -p target/deploy
	cp keys/AAAA.json target/deploy/dropset-keypair.json

program: check-toolchain program-keypair
	anchor keys sync && anchor build

# Build the program .so WITHOUT `admin-teardown` (the shape of the final
# immutable deploy). `anchor build`'s trailing args are forwarded to
# `cargo build-sbf`, so this rebuilds `dropset.so` feature-off. Split out
# from `test-no-teardown` so CI can cache this .so and skip the rebuild on
# a cache hit (see .github/workflows/test.yml).
program-no-teardown: check-toolchain program-keypair
	anchor build -- --no-default-features

# Build BOTH artifacts the Rust↔ASM parity tests need: the reference
# (feature-off) build stashed as `dropset_ref.so`, then the default asm
# build left in `dropset.so`. The reference build runs first because
# `anchor build` always writes `dropset.so`; the trailing default build
# restores the asm artifact every other test deploys.
program-parity: check-toolchain program-keypair
	anchor build -- --no-default-features --features admin-teardown
	cp target/deploy/dropset.so target/deploy/dropset_ref.so
	anchor build

debugger: program
	anchor debugger

# === Program tests ===

test: program
	cargo test

# Feature-off coverage: build the program WITHOUT `admin-teardown` and
# assert every teardown instruction returns `TeardownDisabled` — only the
# feature-off-gated test target is run.
test-no-teardown: program-no-teardown
	cargo test --no-default-features --test teardown_disabled

# Rust↔ASM parity: deploy both artifacts and assert the assembly fast path
# (the default `dropset.so`) matches the reference kernel — identical stamp
# bytes and domain error codes.
test-parity: program-parity
	cargo test --test asm_parity

# === SDK & codegen ===

# Regenerate the checked-in IDL from the program. Pin anchor-cli to the
# same anchor-next rev as the program crate (see install-anchor-v2) so
# the IDL-diff baseline doesn't drift — interface.md § SDK, CI discipline.
# Depend on program-keypair (like program: does) so the canonical
# keys/AAAA.json is staged before the build — otherwise anchor syncs
# declare_id! and the IDL `address` to a throwaway build keypair.
idl: check-toolchain program-keypair
	anchor idl build -o sdk/idl/dropset.json

# Regenerate the TS + Rust clients from the checked-in IDL via Codama,
# then normalize the Rust output with `cargo fmt` so it lands in canonical
# form (clean under the rustfmt hook, reproducible by the SDK CI gate).
sdk: check-pnpm
	cd sdk/codama && pnpm install && pnpm generate
	cargo fmt -p dropset-sdk

# Build the WASM package for the TS client (requires wasm-pack:
# `cargo install wasm-pack`). Built over `dropset-interface`, whose `wasm`
# feature turns on `dropset-math-core`'s, so the one package exports both the
# `simulate_swap` binding and the `Price` codec bindings. Emits the glue
# straight into the TS SDK (sdk/ts/src/wasm) so `@dropset/sdk` can import it
# and the SDK CI type-checks against it; the `simulate` module wraps it.
wasm: check-wasm
	cd sdk/interface && wasm-pack build --target web \
		--out-dir ../ts/src/wasm --features wasm
	rm -f sdk/ts/src/wasm/.gitignore sdk/ts/src/wasm/package.json \
		sdk/ts/src/wasm/README.md sdk/ts/src/wasm/LICENSE

# Regenerate the checked-in conformance vectors from their generators.
# The `--write` flag makes each example write its canonical JSON straight
# to sdk/conformance/*.json (instead of stdout, avoiding a shell redirect),
# so the generators stay the single source of truth.
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
check-conformance-vectors: conformance-vectors
	git add -A -- sdk/conformance/
	git diff --cached --exit-code -- sdk/conformance/

# Run the SDK test suites: Rust (math-core + interface + dropset-sdk, incl.
# the conformance vectors) and the TS conformance check.
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
# and dies with `SectorOverflow`). Finally warm the local explorer's Docker
# image: the TUI brings the explorer up in the background at launch, and
# without a cached image that `docker compose up` builds it lazily — streaming
# a multi-minute image build into the TUI log, the same class of stall the
# program/bot prebuild kills. The build is guarded on a `docker` CLI so a
# no-Docker host (which falls back to the hosted explorer) is unaffected.
# After `make clean && make tui`, every in-TUI command runs without building.
tui-prebuild: check-toolchain program-keypair
	anchor keys sync && anchor build --no-idl
	cargo build -p dropset-maker-bot -p dropset-taker-bot
	@if command -v docker >/dev/null 2>&1; then \
		docker compose -f infra/localnet/docker-compose.yml \
			build explorer; \
	else \
		echo "docker not found — skipping explorer image prebuild"; \
	fi

# Localnet control-plane TUI. Spawns its own
# solana-test-validator (ledger in a temp dir), so it needs no running
# validator first — `tui-prebuild` handles the toolchain gate and warms every
# build the panel will need. Named `tui` (not `localnet`) because the same
# panel will later drive mainnet too.
tui: tui-prebuild
	cargo run -p dropset-tui

# Headless rent reclamation — the same teardown the TUI's "Teardown & reclaim"
# action runs, with no UI. Defaults to localnet; pass WALLET to override the
# admin keypair and ARGS for the rest (e.g. a real cluster, which prompts for
# confirmation — add --yes to skip that prompt in automation):
# `make teardown WALLET=~/admin.json ARGS="--rpc-url <url> --yes"`.
teardown:
	cargo run -p dropset-tui --bin dropset-teardown -- \
		$(if $(WALLET),--wallet $(WALLET)) $(ARGS)

# Run next dev and open the browser once it's accepting connections.
frontend: check-pnpm
	cd frontend && pnpm install
	@( until nc -z localhost 3000 2>/dev/null; do sleep 0.2; done; \
		opener=$$(command -v open || command -v xdg-open) \
			&& $$opener http://localhost:3000 ) &
	cd frontend && pnpm dev

# Run the frontend against a local validator (open http://localhost:3000): the
# localnet cluster + local RPC/WS, overriding the mainnet endpoints in
# .env.local (a process env var wins over .env files in Next). Assumes a
# validator is up with the markets seeded, which the `tui` control plane does;
# run `make tui` alongside this, or use `make localnet` to launch both.
frontend-localnet: check-pnpm
	cd frontend && pnpm install
	cd frontend && NEXT_PUBLIC_CLUSTER=localnet \
		NEXT_PUBLIC_RPC_URL=http://127.0.0.1:8899 \
		NEXT_PUBLIC_WS_URL=ws://127.0.0.1:8900 pnpm dev

# Run the decks deck dev server (port 3300, set in the dev script) and
# open the browser once it's accepting connections.
decks: check-pnpm
	cd decks && pnpm install
	@( until nc -z localhost 3300 2>/dev/null; do sleep 0.2; done; \
		opener=$$(command -v open || command -v xdg-open) \
			&& $$opener http://localhost:3300 ) &
	cd decks && pnpm dev

# The whole localnet demo in one command: the TUI control plane in the
# foreground (it spawns the validator and seeds the markets) plus the
# localnet frontend in the background, pointed at that validator. Quitting the
# TUI stops the frontend too; the frontend retries until the validator is up,
# so start order doesn't matter. Cleanup runs a broad `pkill -f "next dev"`,
# so it also stops any unrelated next dev you have running (dev-only target).
# A third background job opens the browser on the frontend once its port is
# accepting connections — `frontend-localnet` (unlike `frontend`) doesn't, so
# the demo would otherwise leave the operator to open localhost:3000 by hand.
localnet:
	@( until nc -z localhost 3000 2>/dev/null; do sleep 0.2; done; \
		opener=$$(command -v open || command -v xdg-open) \
			&& $$opener http://localhost:3000 ) &
	@$(MAKE) --no-print-directory frontend-localnet & \
	trap 'kill %1 2>/dev/null; pkill -f "next dev"' INT TERM EXIT; \
	$(MAKE) --no-print-directory tui

# === Localnet Docker stacks ===

# Localnet Docker stack: the local Solana Explorer (infra/localnet). The
# dropset-tui control plane manages this automatically; these targets drive
# it by hand. First `explorer` run builds the image from source (a few
# minutes); later runs reuse the cache. Set DROPSET_EXPLORER_REF to pin the
# explorer version (branch, tag, or commit SHA).
explorer: check-docker
	docker compose -f infra/localnet/docker-compose.yml up -d explorer
explorer-down: check-docker
	docker compose -f infra/localnet/docker-compose.yml down

# Nuke the localnet Docker state for a fully cold start: stop and remove every
# stack container (explorer, indexer, bots, postgres — the `taker` profile
# included), drop its volumes and any orphans, remove the images compose built
# locally, and prune the build cache. The container removal takes each
# container's logs with it. Use it to validate a cold `make tui`: the next
# launch rebuilds the explorer image from scratch, and `tui-prebuild` warms it
# again. Pulled base images (e.g. postgres) survive `--rmi local`; run
# `docker system prune` by hand if you want those gone too.
clean-docker: check-docker
	docker compose -f infra/localnet/docker-compose.yml \
		--profile taker down --rmi local -v --remove-orphans
	docker builder prune -f

# Localnet indexer stack: Postgres + the event indexer worker + the /v1 API
# (infra/localnet, docs/indexer.md §8). Needs a running validator (the tui or
# a host-run solana-test-validator) as the live event source. First run builds
# the Rust image (slow); later runs reuse the cargo-chef dependency cache. The
# /v1 surface comes up on http://localhost:8080.
indexer-up: check-docker
	docker compose -f infra/localnet/docker-compose.yml \
		up -d postgres indexer indexer-api
indexer-down: check-docker
	docker compose -f infra/localnet/docker-compose.yml \
		rm -sf postgres indexer indexer-api

# Localnet bot stack: the maker bot (infra/localnet). It signs with the repo
# keys/ keypairs and reaches the host-run validator at
# host.docker.internal:8899. Needs a running validator with the market
# bootstrapped and seeded (the tui control plane). First run builds the Rust
# image (slow); later runs reuse the cargo-chef dependency cache. The taker is
# opt-in (`taker-up`), never started here — the demo market stays quiet until
# an operator asks for organic flow.
bots-up: check-docker
	docker compose -f infra/localnet/docker-compose.yml \
		up -d maker-bot
bots-down: check-docker
	docker compose -f infra/localnet/docker-compose.yml \
		rm -sf maker-bot taker-bot

# Opt-in localnet flow: start / stop the benign stochastic taker so the seeded
# books move and the maker takes fills. Off by default (gated behind the compose
# `taker` profile); flip it on for a walkthrough, off to leave the market quiet.
taker-up: check-docker
	docker compose -f infra/localnet/docker-compose.yml \
		--profile taker up -d taker-bot
taker-down: check-docker
	docker compose -f infra/localnet/docker-compose.yml \
		rm -sf taker-bot

# === Repo tooling ===

lint:
	pre-commit run --config cfg/pre-commit-lint.yml --all-files

# Account for where a session's tokens went (the deterministic core of the
# session-metrics skill). A stdlib-only Python skill-tool under .claude/tools/
# (not a Cargo workspace member — see CLAUDE.md "Skill tooling"). Resolves the
# transcript itself from the Claude home (CLAUDE_CONFIG_DIR or ~/.claude) and
# the working-directory project slug, reads it in its own process, and prints
# a compact ranked-sink summary. Pass the session id:
# `make session-metrics SESSION=<uuid>` (add ARGS=--json for JSON).
session-metrics:
	python3 .claude/tools/session_metrics.py --session-id $(SESSION) $(ARGS)

# Run every Python skill-tool's unit tests (stdlib `unittest`, no third-party
# dep). Covers the `.claude/tools/` skill helpers (tests live under
# `.claude/tools/tests/`) plus the Python under `.claude/scripts/` (the iTerm
# tab-ordering logic). The tools tests import their modules bare (`import
# firm_core`), so discovery runs with the tests dir as start and the tool home
# as top-level (`-t`) to keep those imports resolving. Run in CI's lint job.
tools-tests:
	python3 -m unittest discover \
		-s .claude/tools/tests -t .claude/tools -p 'test_*.py'
	python3 -m unittest discover -s .claude/scripts -p 'test_*.py'

# https://github.com/solana-foundation/anchor/tree/anchor-next/lang-v2
install-anchor-v2:
	CARGO_PROFILE_RELEASE_LTO=off cargo install \
		--git https://github.com/solana-foundation/anchor.git \
		--branch anchor-next \
		anchor-cli --force
