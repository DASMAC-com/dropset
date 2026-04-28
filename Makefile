.PHONY: all
.PHONY: check-toolchain
.PHONY: clean
.PHONY: install-anchor-v2
.PHONY: lint
.PHONY: test

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
