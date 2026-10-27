//! Token-2022 extension policy, against the deployed `.so` (interface.md
//! § 3, **Mint qualification — the listing checklist**).
//!
//! * `create_market` admits **any** extension set — the frontend curates
//!   which currencies are listed, so the program gates nothing. One
//!   positive test per extension the checklist names that a mint can carry
//!   on its own (`TokenMetadata` and the group/member records are
//!   represented by their pointer extensions).
//! * Every inbound leg transfer (`deposit`, `deposit_leader`, `swap`'s
//!   input leg), on either leg, fails `TransferShortfall` unless the
//!   treasury receives the full amount, so a transfer-fee mint can never
//!   leave `treasury.amount()` below the vault legs it backs.
//! * The checklist's dead-market rows hold: a hooked or non-transferable
//!   leg cannot be seeded.
//! * The issuer levers are characterized, not defended against: a frozen
//!   treasury, or a paused mint, halts withdraws until the issuer relents.

mod common;

use anchor_v2_testing::{Keypair, Signer};
use common::fixture::{simple_profile, Fixture};
use common::{
    assert_program_error, create_token2022_mint_with, send_signed, set_token_account_frozen,
    TOKEN_2022_PROGRAM_ID,
};
use dropset::{DropsetError, Price};
use solana_clock::Clock;
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;
use spl_token_2022_interface::{
    error::TokenError,
    extension::{
        confidential_transfer, default_account_state, group_pointer, interest_bearing_mint,
        metadata_pointer, pausable, scaled_ui_amount, transfer_fee, transfer_hook, ExtensionType,
    },
    instruction,
    state::AccountState,
};

const SEED_BASE: u64 = 1_000_000;
const SEED_QUOTE: u64 = 1_085_000;
/// 1% transfer fee, uncapped.
const FEE_BPS: u16 = 100;

/// A fixture whose quote leg is a fresh Token-2022 mint carrying
/// `extensions` (initialized by `init`), with the fixture authority as its
/// freeze authority when `freezable`. `Fixture::bootstrap_with_quote`
/// runs `create_market` and panics if it is rejected, so building the
/// fixture *is* the admission check.
fn t22_quote_fixture(
    extensions: &[ExtensionType],
    freezable: bool,
    init: impl FnOnce(&Pubkey, &Pubkey) -> Vec<Instruction>,
) -> Fixture {
    Fixture::bootstrap_with_quote(TOKEN_2022_PROGRAM_ID, |svm, auth| {
        let freeze = auth.pubkey();
        create_token2022_mint_with(svm, auth, extensions, freezable.then_some(&freeze), init)
    })
}

fn assert_admitted(
    extensions: &[ExtensionType],
    freezable: bool,
    init: impl FnOnce(&Pubkey, &Pubkey) -> Vec<Instruction>,
) {
    let f = t22_quote_fixture(extensions, freezable, init);
    assert_eq!(f.market_header().quote_mint, f.quote_mint.to_bytes().into());
    assert!(
        f.svm.get_account(&f.quote_treasury).is_some(),
        "quote treasury created under Token-2022"
    );
}

/// A transfer-fee config at `bps`, uncapped. Pass `0` to seed the vault
/// before [`activate_fee`] makes it bite.
fn transfer_fee_ix(mint: &Pubkey, auth: &Pubkey, bps: u16) -> Instruction {
    transfer_fee::instruction::initialize_transfer_fee_config(
        &TOKEN_2022_PROGRAM_ID,
        mint,
        Some(auth),
        Some(auth),
        bps,
        u64::MAX,
    )
    .unwrap()
}

/// Raise `mint`'s transfer fee to [`FEE_BPS`] and advance two epochs —
/// Token-2022 schedules a new fee for the epoch after next.
fn activate_fee(f: &mut Fixture, mint: Pubkey) {
    let auth = f.authority.insecure_clone();
    let ix = transfer_fee::instruction::set_transfer_fee(
        &TOKEN_2022_PROGRAM_ID,
        &mint,
        &auth.pubkey(),
        &[],
        FEE_BPS,
        u64::MAX,
    )
    .unwrap();
    send_signed(&mut f.svm, &[&auth], &[ix]);
    let mut clock = f.svm.get_sysvar::<Clock>();
    clock.epoch += 2;
    f.svm.set_sysvar::<Clock>(&clock);
}

fn seed(f: &mut Fixture) -> Result<(), String> {
    f.seed_vault(
        SEED_BASE,
        SEED_QUOTE,
        simple_profile(5_000, 10_000, u32::MAX),
    )
}

// ── create_market admits every extension ─────────────────────────────

#[test]
fn admits_transfer_fee() {
    assert_admitted(&[ExtensionType::TransferFeeConfig], false, |m, a| {
        vec![transfer_fee_ix(m, a, FEE_BPS)]
    });
}

#[test]
fn admits_transfer_hook() {
    // A hook program set on the mint (here one that does not exist): leg
    // transfers cannot invoke it, so every one fails — see
    // `a_hooked_leg_cannot_be_seeded` — but listing is not refused.
    let hook = Pubkey::new_unique();
    assert_admitted(&[ExtensionType::TransferHook], false, |m, a| {
        vec![transfer_hook::instruction::initialize(
            &TOKEN_2022_PROGRAM_ID,
            m,
            Some(*a),
            Some(hook),
        )
        .unwrap()]
    });
}

#[test]
fn admits_permanent_delegate() {
    assert_admitted(&[ExtensionType::PermanentDelegate], false, |m, a| {
        vec![instruction::initialize_permanent_delegate(&TOKEN_2022_PROGRAM_ID, m, a).unwrap()]
    });
}

#[test]
fn admits_non_transferable() {
    assert_admitted(&[ExtensionType::NonTransferable], false, |m, _| {
        vec![instruction::initialize_non_transferable_mint(&TOKEN_2022_PROGRAM_ID, m).unwrap()]
    });
}

#[test]
fn admits_default_account_state_frozen() {
    // DefaultAccountState=Frozen needs a freeze authority; the treasury
    // ATAs are then created frozen.
    let f = t22_quote_fixture(&[ExtensionType::DefaultAccountState], true, |m, _| {
        vec![
            default_account_state::instruction::initialize_default_account_state(
                &TOKEN_2022_PROGRAM_ID,
                m,
                &AccountState::Frozen,
            )
            .unwrap(),
        ]
    });
    let treasury = f.svm.get_account(&f.quote_treasury).expect("treasury");
    // SPL account layout: `state` at offset 108; 2 = Frozen.
    assert_eq!(treasury.data[108], AccountState::Frozen as u8);
}

#[test]
fn admits_pausable() {
    assert_admitted(&[ExtensionType::Pausable], false, |m, a| {
        vec![pausable::instruction::initialize(&TOKEN_2022_PROGRAM_ID, m, a).unwrap()]
    });
}

#[test]
fn admits_confidential_transfer() {
    assert_admitted(&[ExtensionType::ConfidentialTransferMint], false, |m, a| {
        vec![confidential_transfer::instruction::initialize_mint(
            &TOKEN_2022_PROGRAM_ID,
            m,
            Some(*a),
            false,
            None,
        )
        .unwrap()]
    });
}

#[test]
fn admits_interest_bearing() {
    assert_admitted(&[ExtensionType::InterestBearingConfig], false, |m, a| {
        vec![interest_bearing_mint::instruction::initialize(
            &TOKEN_2022_PROGRAM_ID,
            m,
            Some(*a),
            500,
        )
        .unwrap()]
    });
}

#[test]
fn admits_scaled_ui_amount() {
    assert_admitted(&[ExtensionType::ScaledUiAmount], false, |m, a| {
        vec![
            scaled_ui_amount::instruction::initialize(&TOKEN_2022_PROGRAM_ID, m, Some(*a), 2.0)
                .unwrap(),
        ]
    });
}

#[test]
fn admits_metadata_and_group_pointers() {
    assert_admitted(
        &[ExtensionType::MetadataPointer, ExtensionType::GroupPointer],
        false,
        |m, a| {
            vec![
                metadata_pointer::instruction::initialize(
                    &TOKEN_2022_PROGRAM_ID,
                    m,
                    Some(*a),
                    Some(*m),
                )
                .unwrap(),
                group_pointer::instruction::initialize(
                    &TOKEN_2022_PROGRAM_ID,
                    m,
                    Some(*a),
                    Some(*m),
                )
                .unwrap(),
            ]
        },
    );
}

#[test]
fn admits_mint_close_authority() {
    assert_admitted(&[ExtensionType::MintCloseAuthority], false, |m, a| {
        vec![
            instruction::initialize_mint_close_authority(&TOKEN_2022_PROGRAM_ID, m, Some(a))
                .unwrap(),
        ]
    });
}

// ── received-amount check on every inbound leg ───────────────────────

#[test]
fn deposit_leader_on_a_transfer_fee_leg_fails_typed() {
    let mut f = t22_quote_fixture(&[ExtensionType::TransferFeeConfig], false, |m, a| {
        vec![transfer_fee_ix(m, a, FEE_BPS)]
    });
    let err = seed(&mut f).unwrap_err();
    assert_program_error(&err, DropsetError::TransferShortfall);
    // The whole transaction reverted: nothing booked, nothing held.
    let v = f.vault(0);
    assert_eq!(v.base_atoms.get(), 0);
    assert_eq!(v.quote_atoms.get(), 0);
    assert_eq!(f.token_balance(&f.quote_treasury), 0);
}

#[test]
fn deposit_and_swap_in_on_a_transfer_fee_leg_fail_typed() {
    // Seed and take one outside deposit while the fee is zero, then let it
    // bite: the next outside deposit and a quote-in swap must both reject
    // rather than book atoms the treasury never received.
    let mut f = t22_quote_fixture(&[ExtensionType::TransferFeeConfig], false, |m, a| {
        vec![transfer_fee_ix(m, a, 0)]
    });
    let (_leader, alice) = f.with_outside_depositor();
    f.assert_treasury_invariant();
    let mint = f.quote_mint;
    activate_fee(&mut f, mint);

    let err = f
        .deposit(&alice, 0, 0, 10_000, 200_000, 200_000)
        .unwrap_err();
    assert_program_error(&err, DropsetError::TransferShortfall);

    // Buy pays the fee-bearing quote leg in.
    let taker = f.funded_depositor(100_000, 100_000);
    let err = f
        .swap(&taker, 0, 10_000, Price::INFINITY.as_u32(), 1)
        .unwrap_err();
    assert_program_error(&err, DropsetError::TransferShortfall);

    // Sell pays the plain SPL base leg in, so it still settles; its quote
    // payout is short by the fee at the taker's end, not the treasury's.
    f.swap(&taker, 1, 10_000, Price::ZERO.as_u32(), 1)
        .expect("Sell pays the SPL leg in");
    // The custody inequality on both legs (`treasury_residual` asserts
    // it); the exact-in Sell may leave unpriced input as residual.
    f.treasury_residual();
}

#[test]
fn every_inbound_site_on_a_transfer_fee_base_leg_fails_typed() {
    // The base-leg mirror, so each call site of `transfer_in_leg` — not
    // only the quote-leg ones — is held to the check.
    let fee_base = |bps: u16| {
        Fixture::bootstrap_with_base(TOKEN_2022_PROGRAM_ID, move |svm, auth| {
            create_token2022_mint_with(
                svm,
                auth,
                &[ExtensionType::TransferFeeConfig],
                None,
                |m, a| vec![transfer_fee_ix(m, a, bps)],
            )
        })
    };

    // `deposit_leader`'s base leg.
    let mut f = fee_base(FEE_BPS);
    let err = seed(&mut f).unwrap_err();
    assert_program_error(&err, DropsetError::TransferShortfall);

    // `deposit`'s base leg and a Sell (base in), once the fee bites.
    let mut f = fee_base(0);
    let (_leader, alice) = f.with_outside_depositor();
    let mint = f.base_mint;
    activate_fee(&mut f, mint);
    let err = f
        .deposit(&alice, 0, 10_000, 0, 200_000, 200_000)
        .unwrap_err();
    assert_program_error(&err, DropsetError::TransferShortfall);
    let taker = f.funded_depositor(100_000, 100_000);
    let err = f
        .swap(&taker, 1, 10_000, Price::ZERO.as_u32(), 1)
        .unwrap_err();
    assert_program_error(&err, DropsetError::TransferShortfall);
}

// ── dead-market rows of the checklist ────────────────────────────────

#[test]
fn a_hooked_leg_cannot_be_seeded() {
    let hook = Pubkey::new_unique();
    let mut f = t22_quote_fixture(&[ExtensionType::TransferHook], false, |m, a| {
        vec![transfer_hook::instruction::initialize(
            &TOKEN_2022_PROGRAM_ID,
            m,
            Some(*a),
            Some(hook),
        )
        .unwrap()]
    });
    // Token-2022 fails the leg transfer itself, before the received-amount
    // check runs: the hook program cannot be invoked from the market's CPI.
    seed(&mut f).unwrap_err();
    assert_eq!(f.vault(0).quote_atoms.get(), 0);
    assert_eq!(f.token_balance(&f.quote_treasury), 0);
}

#[test]
fn a_non_transferable_leg_cannot_be_seeded() {
    let mut f = t22_quote_fixture(&[ExtensionType::NonTransferable], false, |m, _| {
        vec![instruction::initialize_non_transferable_mint(&TOKEN_2022_PROGRAM_ID, m).unwrap()]
    });
    let err = seed(&mut f).unwrap_err();
    let code = format!("Custom({})", TokenError::NonTransferable as u32);
    assert!(err.contains(&code), "expected {code}, got: {err}");
}

// ── issuer levers: characterized, not defended against ───────────────

#[test]
fn withdraw_fails_while_the_mint_is_paused() {
    let mut f = t22_quote_fixture(&[ExtensionType::Pausable], false, |m, a| {
        vec![pausable::instruction::initialize(&TOKEN_2022_PROGRAM_ID, m, a).unwrap()]
    });
    seed(&mut f).expect("seed");
    let shares = f.vault(0).leader_shares.get();
    let auth = f.authority.insecure_clone();
    let pause =
        pausable::instruction::pause(&TOKEN_2022_PROGRAM_ID, &f.quote_mint, &auth.pubkey(), &[])
            .unwrap();
    send_signed(&mut f.svm, &[&auth], &[pause]);

    let err = f.withdraw_leader(0, shares / 2, 0, 0).unwrap_err();
    let code = format!("Custom({})", TokenError::MintPaused as u32);
    assert!(err.contains(&code), "expected {code}, got: {err}");
}

#[test]
fn withdraw_fails_while_treasury_frozen() {
    let mut f = t22_quote_fixture(&[], true, |_, _| vec![]);
    seed(&mut f).expect("seed");
    let shares = f.vault(0).leader_shares.get();
    let auth: Keypair = f.authority.insecure_clone();
    let (treasury, mint) = (f.quote_treasury, f.quote_mint);

    set_token_account_frozen(
        &mut f.svm,
        &auth,
        &TOKEN_2022_PROGRAM_ID,
        &treasury,
        &mint,
        true,
    );
    let err = f.withdraw_leader(0, shares / 2, 0, 0).unwrap_err();
    let frozen = format!("Custom({})", TokenError::AccountFrozen as u32);
    assert!(err.contains(&frozen), "expected {frozen}, got: {err}");

    // Nothing the program can do moves it; only the issuer's thaw does.
    set_token_account_frozen(
        &mut f.svm,
        &auth,
        &TOKEN_2022_PROGRAM_ID,
        &treasury,
        &mint,
        false,
    );
    // The retry is byte-identical to the rejected withdraw.
    f.svm.expire_blockhash();
    f.withdraw_leader(0, shares / 2, 0, 0)
        .expect("withdraw after thaw");
    f.assert_treasury_invariant();
}
