//! `set_quote_authority` integration tests — leader-only quote-authority
//! rotation. Covers the happy rotation, the functional hand-off (the new
//! authority gains the quoting hot path and the old one loses it), the
//! self-rotation that revokes delegation, and the authority / zero-address
//! / sector gates.

mod common;

use anchor_v2_testing::Signer;
use common::fixture::Fixture;
use solana_pubkey::Pubkey;

/// Bootstrap + one admin vault (sector 0) with `authority` as both leader
/// and quote authority.
fn fixture_with_vault() -> Fixture {
    let mut f = Fixture::bootstrap();
    f.create_vault(0, f.authority.pubkey(), false, Pubkey::default())
        .expect("create_vault");
    f
}

fn valid_price() -> u32 {
    dropset::Price::encode(10_850_000, 0).unwrap().as_u32()
}

#[test]
fn leader_rotates_quote_authority() {
    let mut f = fixture_with_vault();
    let leader = f.authority.insecure_clone();
    let new_authority = Pubkey::new_unique();
    // Sanity: the vault opens with the leader as its own quote authority.
    assert_eq!(
        f.vault(0).quote_authority,
        leader.pubkey().to_bytes().into()
    );

    f.set_quote_authority(&leader, 0, new_authority)
        .expect("leader may rotate quote authority");
    assert_eq!(f.vault(0).quote_authority, new_authority.to_bytes().into());
}

#[test]
fn rotation_hands_off_the_quoting_hot_path() {
    let mut f = fixture_with_vault();
    let leader = f.authority.insecure_clone();
    // Delegate quoting to a fresh, funded key so it can sign its own tx.
    let delegate = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS);
    f.set_quote_authority(&leader, 0, delegate.pubkey())
        .expect("leader may delegate quoting");

    // The new authority can now drive the quoting hot path.
    f.set_reference_price(&delegate, 0, valid_price(), 0)
        .expect("rotated quote authority may set the reference price");

    // And the former authority (the leader) is now locked out of it.
    let err = f
        .set_reference_price(&leader, 0, valid_price(), 0)
        .expect_err("the superseded authority must lose the quoting path");
    common::assert_program_error(&err, dropset::DropsetError::Unauthorized);
}

#[test]
fn leader_may_revoke_delegation_to_self() {
    let mut f = fixture_with_vault();
    let leader = f.authority.insecure_clone();
    let delegate = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS);
    f.set_quote_authority(&leader, 0, delegate.pubkey())
        .expect("delegate quoting");

    // Rotating back to the leader's own pubkey revokes the delegation.
    f.set_quote_authority(&leader, 0, leader.pubkey())
        .expect("leader may rotate quoting back to itself");
    assert_eq!(
        f.vault(0).quote_authority,
        leader.pubkey().to_bytes().into()
    );

    let err = f
        .set_reference_price(&delegate, 0, valid_price(), 0)
        .expect_err("the revoked delegate must lose the quoting path");
    common::assert_program_error(&err, dropset::DropsetError::Unauthorized);
}

#[test]
fn leader_may_rotate_on_a_frozen_vault() {
    // SetQuoteAuthority is leader-only (architecture.md § Caller mechanics →
    // Authority), NOT a quote-mutating ix, so — unlike SetReferencePrice /
    // SetLiquidityProfile — it is deliberately NOT gated on `!frozen`.
    // Freezing must not trap a leader from rotating away a compromised quote
    // authority; the rotated-in key still can't quote (those paths reject a
    // frozen vault), so the rotation is harmless. This pins that contract
    // against a future change that might wrongly add a frozen check.
    let mut f = fixture_with_vault();
    let leader = f.authority.insecure_clone();
    f.freeze_vault(&leader, 0).expect("admin freezes vault");

    let new_authority = Pubkey::new_unique();
    f.set_quote_authority(&leader, 0, new_authority)
        .expect("leader may rotate quote authority even on a frozen vault");
    assert_eq!(f.vault(0).quote_authority, new_authority.to_bytes().into());
}

#[test]
fn rejects_non_leader() {
    let mut f = fixture_with_vault();
    // The incumbent quote authority is the leader here; prove that even a
    // distinct funded stranger cannot rotate it — only the leader may.
    let stranger = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS);
    let err = f
        .set_quote_authority(&stranger, 0, stranger.pubkey())
        .expect_err("non-leader must not rotate the quote authority");
    common::assert_program_error(&err, dropset::DropsetError::Unauthorized);
    // Unchanged.
    let leader = f.authority.insecure_clone();
    assert_eq!(
        f.vault(0).quote_authority,
        leader.pubkey().to_bytes().into()
    );
}

#[test]
fn rejects_zero_address() {
    let mut f = fixture_with_vault();
    let leader = f.authority.insecure_clone();
    // The zero pubkey is the free-list marker and has no private key —
    // stamping it would quote-brick the vault, so it is rejected.
    let err = f
        .set_quote_authority(&leader, 0, Pubkey::default())
        .expect_err("the zero address must be rejected");
    common::assert_program_error(&err, dropset::DropsetError::Unauthorized);
    assert_eq!(
        f.vault(0).quote_authority,
        leader.pubkey().to_bytes().into()
    );
}

#[test]
fn rejects_invalid_idx() {
    let mut f = fixture_with_vault();
    let leader = f.authority.insecure_clone();
    let err = f
        .set_quote_authority(&leader, 99, leader.pubkey())
        .expect_err("out-of-range vault_idx must reject");
    common::assert_program_error(&err, dropset::DropsetError::InvalidSectorIndex);
}

#[test]
fn rejects_empty_sector() {
    let mut f = fixture_with_vault();
    // Vacate the (in-range) sector by zeroing its leader — the free-list
    // emptiness marker. The VaultEmpty guard must fire before the
    // leader-authorization check.
    f.poke_leader_empty(0);
    let leader = f.authority.insecure_clone();
    let err = f
        .set_quote_authority(&leader, 0, leader.pubkey())
        .expect_err("an empty sector must reject");
    common::assert_program_error(&err, dropset::DropsetError::VaultEmpty);
}
