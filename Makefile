.PHONY: all
.PHONY: check-anchor
.PHONY: check-conformance-vectors
.PHONY: check-sbf
.PHONY: check-solana
.PHONY: check-toolchain
.PHONY: clean
.PHONY: conformance-vectors
.PHONY: explorer
.PHONY: explorer-down
.PHONY: frontend
.PHONY: idl
.PHONY: indexer-down
.PHONY: indexer-up
.PHONY: install-anchor-v2
.PHONY: lint
.PHONY: sdk
.PHONY: sdk-test
.PHONY: session-metrics
.PHONY: teardown
.PHONY: test
.PHONY: test-no-teardown
.PHONY: tools-tests
.PHONY: tui
.PHONY: wasm

all: lint test
clean:

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
sdk:
	cd sdk/codama && pnpm install && pnpm generate
	cargo fmt -p dropset-sdk

# Build the WASM package for the TS client (requires wasm-pack:
# `cargo install wasm-pack`). Built over `dropset-interface`, whose `wasm`
# feature turns on `dropset-math-core`'s, so the one package exports both the
# `simulate_swap` binding and the `Price` codec bindings. Outputs
# sdk/interface/pkg.
wasm:
	cd sdk/interface && wasm-pack build --target web --features wasm

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
sdk-test:
	cargo test -p dropset-math-core -p dropset-interface -p dropset-sdk
	cd sdk/ts && pnpm test

debugger: program
	anchor debugger

# Localnet control-plane TUI. Spawns its own
# solana-test-validator (ledger in a temp dir), so it needs no running
# validator first — just the toolchain check-toolchain gates. Named `tui`
# (not `localnet`) because the same panel will later drive mainnet too.
tui:
	cargo run -p dropset-tui

# Headless rent reclamation — the same teardown the TUI's "Teardown & reclaim"
# action runs, with no UI. Defaults to localnet; pass WALLET to override the
# admin keypair and ARGS for the rest (e.g. a real cluster, which prompts for
# confirmation — add --yes to skip that prompt in automation):
# `make teardown WALLET=~/admin.json ARGS="--rpc-url <url> --yes"`.
teardown:
	cargo run -p dropset-tui --bin dropset-teardown -- \
		$(if $(WALLET),--wallet $(WALLET)) $(ARGS)

# Localnet Docker stack: the local Solana Explorer (infra/localnet). The
# dropset-tui control plane manages this automatically; these targets drive
# it by hand. First `explorer` run builds the image from source (a few
# minutes); later runs reuse the cache. Set DROPSET_EXPLORER_REF to pin the
# explorer version (branch, tag, or commit SHA).
explorer:
	docker compose -f infra/localnet/docker-compose.yml up -d explorer
explorer-down:
	docker compose -f infra/localnet/docker-compose.yml down

# Localnet indexer stack: Postgres + the event indexer worker + the /v1 API
# (infra/localnet, docs/indexer.md §8). Needs a running validator (the tui or
# a host-run solana-test-validator) as the live event source. First run builds
# the Rust image (slow); later runs reuse the cargo-chef dependency cache. The
# /v1 surface comes up on http://localhost:8080.
indexer-up:
	docker compose -f infra/localnet/docker-compose.yml \
		up -d postgres indexer indexer-api
indexer-down:
	docker compose -f infra/localnet/docker-compose.yml \
		rm -sf postgres indexer indexer-api

# Run next dev and open the browser once it's accepting connections.
frontend:
	cd frontend && pnpm install
	@( until nc -z localhost 3000 2>/dev/null; do sleep 0.2; done; \
		opener=$$(command -v open || command -v xdg-open) \
			&& $$opener http://localhost:3000 ) &
	cd frontend && pnpm dev

# https://github.com/solana-foundation/anchor/tree/anchor-next/lang-v2
install-anchor-v2:
	CARGO_PROFILE_RELEASE_LTO=off cargo install \
		--git https://github.com/solana-foundation/anchor.git \
		--branch anchor-next \
		anchor-cli --force

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
# dep). Covers both tool homes: the `tools/` deterministic skill cores and the
# `.claude/tools/` skill helpers. Each tool dir is its own discovery root
# because `tools/stage-backlog` is a hyphenated, non-package directory that a
# single top-level `discover -s tools` can't import. Run in CI's lint job.
tools-tests:
	python3 -m unittest discover -s tools/stage-backlog -p 'test_*.py'
	python3 -m unittest discover -s .claude/tools -p 'test_*.py'

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

test: program
	cargo test

# Build the program .so WITHOUT `admin-teardown` (the shape of the final
# immutable deploy). `anchor build`'s trailing args are forwarded to
# `cargo build-sbf`, so this rebuilds `dropset.so` feature-off. Split out
# from `test-no-teardown` so CI can cache this .so and skip the rebuild on
# a cache hit (see .github/workflows/test.yml).
program-no-teardown: check-toolchain program-keypair
	anchor build -- --no-default-features

# Feature-off coverage: build the program WITHOUT `admin-teardown` and
# assert every teardown instruction returns `TeardownDisabled` — only the
# feature-off-gated test target is run.
test-no-teardown: program-no-teardown
	cargo test --no-default-features --test teardown_disabled
