mod common;

use anchor_lang_v2::{programs::System, Id, InstructionData};
use anchor_v2_testing::{Keypair, LiteSVM, Signer};
use common::{
    decode_slab, deploy_with_authority, send_ixn, PROGRAM_ID, SIGNER_FUNDING_LAMPORTS,
};
use dropset::{
    instruction::{AddAdmin as AddAdminIx, Init as InitIx, RemoveAdmin as RemoveAdminIx},
    Registry, RegistryHeader,
};
use solana_instruction::{AccountMeta, Instruction};
use solana_loader_v3_interface::get_program_data_address;
use solana_pubkey::Pubkey;

fn registry_pda() -> Pubkey {
    Pubkey::find_program_address(&[b"registry"], &PROGRAM_ID).0
}

fn init_ixn(payer: Pubkey, genesis_admin: Pubkey) -> Instruction {
    Instruction::new_with_bytes(
        PROGRAM_ID,
        &InitIx { genesis_admin }.data(),
        vec![
            AccountMeta::new(payer, true),
            AccountMeta::new(registry_pda(), false),
            AccountMeta::new_readonly(System::id(), false),
            AccountMeta::new_readonly(get_program_data_address(&PROGRAM_ID), false),
        ],
    )
}

fn add_ixn(admin: Pubkey, new_admin: Pubkey) -> Instruction {
    Instruction::new_with_bytes(
        PROGRAM_ID,
        &AddAdminIx { new_admin }.data(),
        vec![
            AccountMeta::new(admin, true),
            AccountMeta::new(registry_pda(), false),
            AccountMeta::new_readonly(System::id(), false),
        ],
    )
}

fn remove_ixn(admin: Pubkey, target: Pubkey) -> Instruction {
    Instruction::new_with_bytes(
        PROGRAM_ID,
        &RemoveAdminIx { target }.data(),
        vec![
            AccountMeta::new(admin, true),
            AccountMeta::new(registry_pda(), false),
        ],
    )
}

/// Deploy and `init` the registry with `authority` as both the upgrade
/// authority and the genesis admin (so it can sign add/remove).
fn setup() -> (LiteSVM, Keypair) {
    let authority = Keypair::new();
    let mut svm = deploy_with_authority(&authority);
    send_ixn(
        &mut svm,
        &authority,
        init_ixn(authority.pubkey(), authority.pubkey()),
    )
    .expect("init should succeed");
    (svm, authority)
}

/// The admin set (as raw 32-byte keys) currently stored in the registry.
fn admins(svm: &LiteSVM) -> Vec<[u8; 32]> {
    let account = svm.get_account(&registry_pda()).expect("registry exists");
    let (_, admins) = decode_slab::<RegistryHeader, [u8; 32]>(&account.data);
    admins.to_vec()
}

/// Rent-exempt lamports for a registry holding `n` admins.
fn rent_for(svm: &LiteSVM, n: usize) -> u64 {
    svm.minimum_balance_for_rent_exemption(Registry::space_for(n))
}

#[test]
fn add_admin_grows_and_funds_rent() {
    let (mut svm, authority) = setup();
    let pda = registry_pda();

    let before = svm.get_account(&pda).unwrap();
    assert_eq!(before.data.len(), Registry::space_for(1));
    assert_eq!(before.lamports, rent_for(&svm, 1));

    let new_admin = Pubkey::new_unique();
    send_ixn(&mut svm, &authority, add_ixn(authority.pubkey(), new_admin)).expect("add");

    let after = svm.get_account(&pda).unwrap();
    let set = admins(&svm);
    assert_eq!(set.len(), 2);
    assert!(set.contains(&authority.pubkey().to_bytes()));
    assert!(set.contains(&new_admin.to_bytes()));
    // Grown to fit two admins and topped up to the new rent floor.
    assert_eq!(after.data.len(), Registry::space_for(2));
    assert_eq!(after.lamports, rent_for(&svm, 2));
    assert!(after.lamports > before.lamports);
}

#[test]
fn add_admin_is_idempotent() {
    let (mut svm, authority) = setup();
    send_ixn(
        &mut svm,
        &authority,
        add_ixn(authority.pubkey(), authority.pubkey()),
    )
    .expect("re-adding an existing admin is a no-op");
    assert_eq!(admins(&svm).len(), 1);
    assert_eq!(svm.get_account(&registry_pda()).unwrap().lamports, rent_for(&svm, 1));
}

#[test]
fn add_admin_rejects_non_admin_signer() {
    let (mut svm, _authority) = setup();
    let imposter = Keypair::new();
    svm.airdrop(&imposter.pubkey(), SIGNER_FUNDING_LAMPORTS).unwrap();

    let err = send_ixn(
        &mut svm,
        &imposter,
        add_ixn(imposter.pubkey(), Pubkey::new_unique()),
    )
    .expect_err("non-admin must be rejected");
    assert!(err.contains("Custom"), "expected Unauthorized, got {err}");
    assert_eq!(admins(&svm).len(), 1);
}

#[test]
fn remove_admin_compacts_and_refunds_rent() {
    let (mut svm, authority) = setup();
    let pda = registry_pda();
    let new_admin = Pubkey::new_unique();
    send_ixn(&mut svm, &authority, add_ixn(authority.pubkey(), new_admin)).expect("add");

    let reg_before = svm.get_account(&pda).unwrap().lamports;
    assert_eq!(reg_before, rent_for(&svm, 2));
    let authority_before = svm.get_account(&authority.pubkey()).unwrap().lamports;

    send_ixn(&mut svm, &authority, remove_ixn(authority.pubkey(), new_admin)).expect("remove");

    let after = svm.get_account(&pda).unwrap();
    // Only the genesis admin remains.
    assert_eq!(admins(&svm), vec![authority.pubkey().to_bytes()]);
    // Shrunk to one admin and the freed rent left the account.
    assert_eq!(after.data.len(), Registry::space_for(1));
    assert_eq!(after.lamports, rent_for(&svm, 1));
    assert_eq!(reg_before - after.lamports, rent_for(&svm, 2) - rent_for(&svm, 1));
    // The signer received the refund (rent delta dwarfs any tx fee).
    let authority_after = svm.get_account(&authority.pubkey()).unwrap().lamports;
    assert!(authority_after > authority_before);
}

#[test]
fn remove_admin_rejects_last_admin() {
    let (mut svm, authority) = setup();
    let err = send_ixn(
        &mut svm,
        &authority,
        remove_ixn(authority.pubkey(), authority.pubkey()),
    )
    .expect_err("removing the last admin must be rejected");
    assert!(err.contains("Custom"), "expected CannotRemoveLastAdmin, got {err}");
    assert_eq!(admins(&svm).len(), 1);
}

#[test]
fn remove_admin_rejects_unknown() {
    let (mut svm, authority) = setup();
    // A second admin so the last-admin guard isn't what trips first.
    let new_admin = Pubkey::new_unique();
    send_ixn(&mut svm, &authority, add_ixn(authority.pubkey(), new_admin)).expect("add");

    let err = send_ixn(
        &mut svm,
        &authority,
        remove_ixn(authority.pubkey(), Pubkey::new_unique()),
    )
    .expect_err("removing a non-admin must be rejected");
    assert!(err.contains("Custom"), "expected AdminNotFound, got {err}");
    assert_eq!(admins(&svm).len(), 2);
}

#[test]
fn remove_admin_rejects_non_admin_signer() {
    let (mut svm, authority) = setup();
    let new_admin = Pubkey::new_unique();
    send_ixn(&mut svm, &authority, add_ixn(authority.pubkey(), new_admin)).expect("add");

    let imposter = Keypair::new();
    svm.airdrop(&imposter.pubkey(), SIGNER_FUNDING_LAMPORTS).unwrap();
    let err = send_ixn(&mut svm, &imposter, remove_ixn(imposter.pubkey(), new_admin))
        .expect_err("non-admin signer must be rejected");
    assert!(err.contains("Custom"), "expected Unauthorized, got {err}");
    assert_eq!(admins(&svm).len(), 2);
}
