//! `sweep_residual` integration tests — the admin recovery path for a
//! treasury balance that neither the vaults' depositors nor the accrued
//! protocol fee has a claim on.
//!
//! The invariant under test throughout is
//! `treasury.amount == Σ vault.<leg>_atoms + accrued_<leg>_fee`, so the
//! residual the instruction pays out is zero on every honest path. The two
//! interesting cases are therefore an **unsolicited transfer** straight to
//! the treasury ATA (recovered) and a **fee'd swap** (accrued revenue,
//! deliberately *not* recovered).

mod common;

use anchor_v2_testing::Signer;
use common::fixture::Fixture;
use dropset::Price;

const SEED_BASE: u64 = 1_000_000;
const SEED_QUOTE: u64 = 1_085_000;

#[test]
fn zero_residual_sweeps_nothing_and_reads_out_the_invariant() {
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let admin = f.authority.insecure_clone();
    // A recipient with live ATAs to point the sweep at.
    let recipient = f.funded_depositor(0, 0);
    let dest = f.base_ata(&recipient.pubkey());
    let dest_before = f.token_balance(&dest);

    let base_treasury = f.base_treasury;
    let base_mint = f.base_mint;
    let meta = f
        .sweep_residual_meta(&admin, &base_mint, &base_treasury, &dest)
        .expect("sweep succeeds on a healthy market");

    // Nothing moved — the treasury is fully claimed by the seeded vault.
    assert_eq!(f.token_balance(&dest), dest_before, "no residual to sweep");
    assert_eq!(f.token_balance(&base_treasury), SEED_BASE);

    // The event still fires, so the instruction doubles as an on-chain
    // read-out of the invariant's three terms.
    let ev = common::events::sweep_residual(&meta);
    assert_eq!(ev.market, f.market.to_bytes());
    assert_eq!(ev.mint, base_mint.to_bytes());
    assert_eq!(ev.treasury_amount, SEED_BASE);
    assert_eq!(ev.vault_sum, SEED_BASE);
    assert_eq!(ev.accrued_fee, 0);
    assert_eq!(ev.swept, 0);
}

#[test]
fn unsolicited_transfer_is_recovered() {
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let admin = f.authority.insecure_clone();
    let recipient = f.funded_depositor(0, 0);
    let dest = f.base_ata(&recipient.pubkey());

    // Someone mints straight into the treasury ATA. No vault has a claim
    // on these atoms, so no `Withdraw` could ever move them.
    let base_mint = f.base_mint;
    let base_treasury = f.base_treasury;
    f.donate(&base_mint, &base_treasury, 7_777);
    assert_eq!(f.token_balance(&base_treasury), SEED_BASE + 7_777);

    let meta = f
        .sweep_residual_meta(&admin, &base_mint, &base_treasury, &dest)
        .expect("sweep recovers the donation");

    let ev = common::events::sweep_residual(&meta);
    assert_eq!(ev.treasury_amount, SEED_BASE + 7_777);
    assert_eq!(ev.vault_sum, SEED_BASE);
    assert_eq!(ev.accrued_fee, 0);
    assert_eq!(ev.swept, 7_777);
    assert_eq!(f.token_balance(&dest), 7_777, "residual paid out in full");
    // Invariant restored exactly: treasury back to the vault's claim.
    assert_eq!(f.token_balance(&base_treasury), SEED_BASE);
    assert_eq!(f.vault(0).base_atoms.get(), SEED_BASE);

    // Idempotent — a second sweep finds nothing. Pointed at a *different*
    // destination so the transaction isn't byte-identical to the first
    // (LiteSVM rejects a repeated signature as `AlreadyProcessed`).
    let second = f.funded_depositor(0, 0);
    let dest2 = f.base_ata(&second.pubkey());
    let meta = f
        .sweep_residual_meta(&admin, &base_mint, &base_treasury, &dest2)
        .expect("second sweep is a no-op");
    assert_eq!(common::events::sweep_residual(&meta).swept, 0);
    assert_eq!(f.token_balance(&dest2), 0);
    assert_eq!(f.token_balance(&dest), 7_777);
}

#[test]
fn accrued_taker_fee_is_not_swept() {
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let admin = f.authority.insecure_clone();
    f.set_taker_fee(&admin, 10_000).expect("1% taker fee");

    let taker = f.funded_depositor(0, 200_000);
    f.swap(&taker, 0, 100_000, Price::INFINITY.as_u32(), 1)
        .expect("fee'd buy");

    let accrued = f.market_header().accrued_base_fee.get();
    assert!(accrued > 0, "the buy accrued a base-leg protocol fee");

    // The fee is real revenue sitting in the treasury — the sweep must
    // subtract it, not pay it out. A harvest is a separate, future lever.
    let dest = f.base_ata(&taker.pubkey());
    let taker_base = f.token_balance(&dest);
    let base_mint = f.base_mint;
    let base_treasury = f.base_treasury;
    let meta = f
        .sweep_residual_meta(&admin, &base_mint, &base_treasury, &dest)
        .expect("sweep after a fee'd swap");

    let ev = common::events::sweep_residual(&meta);
    assert_eq!(ev.accrued_fee, accrued);
    assert_eq!(
        ev.vault_sum + ev.accrued_fee,
        ev.treasury_amount,
        "custody invariant holds after a fee'd swap"
    );
    assert_eq!(ev.swept, 0, "accrued revenue is not a residual");
    assert_eq!(f.token_balance(&dest), taker_base, "nothing paid out");
    assert_eq!(
        f.market_header().accrued_base_fee.get(),
        accrued,
        "the counter is subtracted, never touched"
    );
}

#[test]
fn non_admin_cannot_sweep() {
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let outsider = f.funded_depositor(0, 0);
    let dest = f.base_ata(&outsider.pubkey());
    let base_mint = f.base_mint;
    let base_treasury = f.base_treasury;
    f.donate(&base_mint, &base_treasury, 500);

    let err = f
        .sweep_residual(&outsider, &base_mint, &base_treasury, &dest)
        .expect_err("a non-admin signer must be rejected");
    common::assert_program_error(&err, dropset::DropsetError::Unauthorized);
    assert_eq!(f.token_balance(&dest), 0, "no payout on the rejected call");
}

/// A market-owned ATA whose mint is neither market leg has no leg to
/// compute a residual against — there is no inventory field and no accrued
/// counter to subtract, so its whole balance would read as residual. The
/// `associated_token` constraint resolves (the account really is
/// `ata(market, mint)`), so the handler's explicit leg check is what
/// rejects it — mirroring `close_market_treasury`.
#[test]
fn non_leg_mint_rejected() {
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let admin = f.authority.insecure_clone();
    let recipient = f.funded_depositor(0, 0);
    let dest = f.base_ata(&recipient.pubkey());
    let market = f.market;
    let other_mint = common::create_spl_mint(&mut f.svm, &admin);
    let other_treasury = common::create_associated_token_account(
        &mut f.svm,
        &admin,
        &market,
        &other_mint,
        &common::SPL_TOKEN_PROGRAM_ID,
    );
    let err = f
        .sweep_residual(&admin, &other_mint, &other_treasury, &dest)
        .expect_err("a non-leg market-owned ATA must not be swept");
    common::assert_program_error(&err, dropset::DropsetError::NotAMarketTreasury);
}
