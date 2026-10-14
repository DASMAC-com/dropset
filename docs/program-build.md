<!-- cspell:word objdump -->

<!-- cspell:word readelf -->

<!-- cspell:word sbpfv -->

# Program build: toolchain facts

How the program is built, and which pins move together, is stated once in
[interface.md § SDK](interface.md#6-sdk) ("How the program is built"). This
page records the measured toolchain facts behind that statement and the probe
recipe that produced them, so nobody has to re-measure them.

## The toolchain

- **anchor-cli 1.2.1** (crates.io) defaults to **platform-tools v1.57** and
  **arch v3**, overridable with `--arch` (or the `ANCHOR_BUILD_SBF_ARCH`
  environment variable) and `--tools-version`. It recognizes the
  `anchor-lang-v2` / `anchor-spl-v2` crate pair in its manifest detection.
- **The build is held at v0** by the Makefile's exported
  `ANCHOR_BUILD_SBF_ARCH`. The tests' litesvm and the local validator run
  the Agave 3.1 runtime, whose sBPF v3 (SIMD-0178, -0179, -0189) predates the
  SIMD-0377 encoding that platform-tools v1.57 emits. A v3 ELF built today
  fails to deploy with `Failed to parse ELF file: invalid file header`, even
  with every feature enabled. The flip to v3 waits for the Agave 4.x runtime.
- **The gate that flip satisfies** is SIMD-0500: a validator refuses to
  deploy, upgrade or finalize any ELF older than sBPF v3. It is listed for
  Agave 4.4.
- **Before this pin**, CI built the CLI from anchor-next at rev `2a191379`
  (June 3). Both anchor-next and the 2.0 branch at rc.2 hard-code
  `--tools-version v1.52` and never pass `--arch`, so they can only build v0.
  The Anchor maintainer confirmed by 2026-10-13 that the stable v1 CLI
  releases are the ones to use, v2 programs included. The program crate is
  unaffected: it stays on `anchor-lang-v2` from anchor-next, which the
  lockfile pins at `e056cc17` (June 23).

## Reading an ELF

- **Version:** `llvm-readelf -h target/deploy/dropset.so` and read the `Flags`
  line (the 4-byte `e_flags` at offset `0x30`): `0x0` is v0, `0x3` is v3.
- **Relocations:** `llvm-readelf -r`. A v3 ELF must report none.
- **Encodings:** `llvm-objdump -d`. Opcode `0x85` is `call`. Source field 0
  (`85 00 …`) with a murmur3 hash immediate is a static syscall. Source field
  1 (`85 10 …`) is an internal call with a pc-relative immediate.

## The entrypoint's syscall spelling

The hand-written assembly entrypoint calls `sol_memcpy_`, and how that call
is spelled decides what it encodes as:

- **`call sol_memcpy_`** (by symbol) encodes an internal call (source field 1)
  plus a relocation. A v0 loader resolves the relocation to the syscall.
  Under v3 it is left unresolved, at immediate `-1`.
- **`call` on a constant** equal to the name's murmur3 hash (`0x717cc4a3` for
  `sol_memcpy_`) encodes the static form, `85 00 00 00 a3 c4 7c 71`. This is
  what v3 dispatches, and what every Rust-side syscall in the same ELF lowers
  to. It is **not** version-agnostic: a v0 load on Agave 3.1 rejects it with
  `Relative jump out of bounds` at that instruction. So the rewrite ships
  together with the v3 flip, never before it.
- The constant must be a plain `.equ`: `build.rs` deliberately fails closed
  on `.equiv` and `.set`.

### Probe recipe

To settle a spelling without building the program:

1. Write a five-line `.s` that makes the call both ways.
1. Assemble it with the platform-tools clang:
   `clang -target sbpfv3-solana-solana -c probe.s`.
1. Inspect the result with `llvm-objdump -d -r` and `llvm-readelf -r`.

`clang -S -O2` on a C call through a constant function pointer prints
LLVM's own spelling: `call 1904002211` for `sol_memcpy_`. When a probe runs
`anchor build` in a worktree whose program keypair differs from the canonical
one, pass `--ignore-keys`, or the build syncs and rewrites `declare_id!`.

## Local hazard: one sbpf toolchain at a time

cargo-build-sbf 3.1.x links one `*-sbpf-solana-*` rustup toolchain at a time,
replacing whichever is linked. A concurrent build that needs a different
platform-tools release (for example, a worktree still on a v1.52 CLI)
unlinks yours mid-build. The failure reads
`override toolchain '…-sbpf-solana-v1.57' is not installed`, followed by
proc-macro panics in `borsh`. Retrying fixes it. CI jobs are isolated and
never see it.
