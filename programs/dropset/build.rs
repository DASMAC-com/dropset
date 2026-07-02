//! Expand the hand-written sBPF assembly under `src/asm/` into
//! `$OUT_DIR/combined.s`, which `lib.rs` links via
//! `anchor_asm_v2_runtime::include_asm!()` under the `asm-entrypoint`
//! feature. Runs on every build; the output is only linked when that
//! feature is on, so the default (reference) build is unaffected.
fn main() {
    anchor_asm_v2::build("src/asm");
}
