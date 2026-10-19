//! [`Vault::name`] encoding — the one rule set every name writer
//! (`create_vault`, `set_vault_name`, `override_vault_name`) shares. See
//! the spec's **Vault** and **SetVaultName**.
//!
//! The rules are deliberately few: printable ASCII (`0x20..=0x7E`), at
//! most [`VAULT_NAME_LEN`] bytes, stored zero-padded, with the empty
//! string meaning unset. Uniqueness is the market-wide walk in
//! [`super::VaultAccess::name_taken`]. Nothing else — no URL grammar, no
//! whitespace rules; the creation fee and the admin override bound abuse.

use anchor_lang::prelude::*;

use crate::errors::DropsetError;

use super::{Vault, VAULT_NAME_LEN};

/// Validate `name` and pack it into the zero-padded on-chain field.
/// The empty string packs to all-zero, the unset name.
pub fn encode_vault_name(name: &str) -> Result<[u8; VAULT_NAME_LEN]> {
    let bytes = name.as_bytes();
    require!(
        bytes.len() <= VAULT_NAME_LEN && bytes.iter().all(|b| (0x20..=0x7E).contains(b)),
        DropsetError::InvalidVaultName
    );
    let mut out = [0u8; VAULT_NAME_LEN];
    out[..bytes.len()].copy_from_slice(bytes);
    Ok(out)
}

/// The stored name as text, padding stripped; empty when unset. Only
/// [`encode_vault_name`] writes the field, so the bytes are ASCII.
pub fn decode_vault_name(name: &[u8; VAULT_NAME_LEN]) -> String {
    let len = name.iter().position(|&b| b == 0).unwrap_or(VAULT_NAME_LEN);
    String::from_utf8_lossy(&name[..len]).into_owned()
}

/// True when two packed names clash under the uniqueness rule:
/// ASCII-case-insensitive equality, with an unset name never clashing.
pub fn names_collide(a: &[u8; VAULT_NAME_LEN], b: &[u8; VAULT_NAME_LEN]) -> bool {
    a[0] != 0 && a.eq_ignore_ascii_case(b)
}

impl Vault {
    /// Blank the name and set or clear the lock in one step — the shape
    /// `freeze_vault` (lock) and `close_vault` (unlock) share. Returns the
    /// name it blanked, for the event.
    pub fn blank_name(&mut self, lock: bool) -> String {
        let old = decode_vault_name(&self.name);
        self.name = [0; VAULT_NAME_LEN];
        self.name_locked = lock.into();
        old
    }
}

#[cfg(test)]
mod tests {
    use anchor_lang::bytemuck::Zeroable;

    use super::*;

    #[test]
    fn encode_pads_and_round_trips() {
        let packed = encode_vault_name("AUDD Desk").unwrap();
        assert_eq!(&packed[..9], b"AUDD Desk");
        assert!(packed[9..].iter().all(|&b| b == 0));
        assert_eq!(decode_vault_name(&packed), "AUDD Desk");
    }

    #[test]
    fn empty_is_unset() {
        assert_eq!(encode_vault_name("").unwrap(), [0; VAULT_NAME_LEN]);
        assert_eq!(decode_vault_name(&[0; VAULT_NAME_LEN]), "");
    }

    #[test]
    fn full_width_accepted_one_more_rejected() {
        let full = "x".repeat(VAULT_NAME_LEN);
        assert_eq!(decode_vault_name(&encode_vault_name(&full).unwrap()), full);
        assert!(encode_vault_name(&"x".repeat(VAULT_NAME_LEN + 1)).is_err());
    }

    #[test]
    fn printable_bounds() {
        assert!(encode_vault_name(" ~").is_ok());
        for bad in ["a\u{1f}", "a\u{7f}", "a\0b", "café", "tab\t"] {
            assert!(encode_vault_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn collision_folds_case_and_ignores_unset() {
        let a = encode_vault_name("Alpha").unwrap();
        let b = encode_vault_name("aLPHA").unwrap();
        let c = encode_vault_name("Alpha2").unwrap();
        let unset = [0; VAULT_NAME_LEN];
        assert!(names_collide(&a, &b));
        assert!(!names_collide(&a, &c));
        assert!(!names_collide(&unset, &unset));
        assert!(!names_collide(&unset, &a));
    }

    #[test]
    fn blank_name_sets_lock_and_returns_old() {
        let mut v = Vault::zeroed();
        v.name = encode_vault_name("Old").unwrap();
        assert_eq!(v.blank_name(true), "Old");
        assert_eq!(v.name, [0; VAULT_NAME_LEN]);
        assert!(v.name_locked.get());
        v.blank_name(false);
        assert!(!v.name_locked.get());
    }
}
