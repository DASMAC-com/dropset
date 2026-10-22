//! Pins the committed copies of each program id together: the program's
//! per-cluster `declare_id!`s, the SDK constants, and (for mainnet) the
//! public key file `keys/dropset-mainnet.pubkey`. The localnet keypair
//! `keys/AAAA.json` is held to its `declare_id!` by anchor's own id check.

use std::fs;
use std::path::PathBuf;

use dropset_sdk::{DROPSET_ID, DROPSET_MAINNET_ID};

fn repo_file(relative: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative);
    fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

/// The `declare_id!` literal directly under the given `#[cfg(...)]` line.
fn declared_under(lib_rs: &str, cfg: &str) -> String {
    let mut lines = lib_rs.lines();
    lines
        .by_ref()
        .find(|line| line.trim() == cfg)
        .unwrap_or_else(|| panic!("no `{cfg}` in the program's lib.rs"));
    let declaration = lines.next().expect("a declare_id! after the cfg");
    declaration
        .trim()
        .strip_prefix("declare_id!(\"")
        .and_then(|rest| rest.strip_suffix("\");"))
        .unwrap_or_else(|| panic!("`{cfg}` is not followed by a declare_id!"))
        .to_owned()
}

#[test]
fn mainnet_id_matches_the_public_key_file_and_the_program() {
    let file = repo_file("keys/dropset-mainnet.pubkey");
    assert_eq!(file.trim(), DROPSET_MAINNET_ID.to_string());
    // Pin the operator-ground vanity prefix, so all copies swapped together
    // to some other key cannot pass.
    assert!(DROPSET_MAINNET_ID.to_string().starts_with("eBook"));

    let lib_rs = repo_file("programs/dropset/src/lib.rs");
    assert_eq!(
        declared_under(&lib_rs, "#[cfg(feature = \"mainnet\")]"),
        DROPSET_MAINNET_ID.to_string(),
    );
}

#[test]
fn localnet_id_is_declared_first() {
    // `anchor keys sync` and anchor's pre-build check read only the first
    // `declare_id!`, so the localnet one has to lead.
    let lib_rs = repo_file("programs/dropset/src/lib.rs");
    assert_eq!(
        declared_under(&lib_rs, "#[cfg(not(feature = \"mainnet\"))]"),
        DROPSET_ID.to_string(),
    );
    // Mirror anchor's line-anchored match, so a comment quoting a
    // declaration cannot count.
    let declarations: Vec<&str> = lib_rs
        .lines()
        .filter_map(|line| line.strip_prefix("declare_id!(\""))
        .collect();
    assert_eq!(declarations.len(), 2, "one declare_id! per cluster");
    assert!(declarations[0].starts_with(&DROPSET_ID.to_string()));
}
