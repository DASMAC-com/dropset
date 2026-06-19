.PHONY: all
.PHONY: check-anchor
.PHONY: check-conformance-vectors
.PHONY: check-sbf
.PHONY: check-solana
.PHONY: check-toolchain
.PHONY: clean
.PHONY: conformance-vectors
.PHONY: frontend
.PHONY: idl
.PHONY: install-anchor-v2
.PHONY: lint
.PHONY: sdk
.PHONY: sdk-test
.PHONY: test
.PHONY: test-no-teardown
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
idl: check-toolchain
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

program: check-toolchain
	anchor keys sync && anchor build

test: program
	cargo test

# Feature-off coverage: build the program WITHOUT `admin-teardown`
# (the shape of the final immutable deploy) and assert every teardown
# instruction returns `TeardownDisabled`. `anchor build`'s trailing args
# are forwarded to `cargo build-sbf`, so this rebuilds `dropset.so`
# feature-off; we then run only the feature-off-gated test target.
test-no-teardown: check-toolchain
	anchor build -- --no-default-features
	cargo test --no-default-features --test teardown_disabled
