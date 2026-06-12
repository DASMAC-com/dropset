.PHONY: all
.PHONY: check-toolchain
.PHONY: clean
.PHONY: frontend
.PHONY: install-anchor-v2
.PHONY: lint
.PHONY: test
.PHONY: test-no-teardown

all: lint test
clean:

check-toolchain:
	@anchor --version | grep -q " 2\." \
		|| { echo "anchor-cli 2.x required"; exit 1; }
	@command -v cargo-build-sbf >/dev/null \
		|| { echo "cargo build-sbf not found (install Solana toolchain)"; \
			exit 1; }

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
