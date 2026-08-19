// cspell:word unfillable
//! Swap integration tests — the multi-vault heap matcher and `min_out`
//! soft-revert, end-to-end against the deployed `.so`. All built on the
//! shared [`Fixture`]: `Fixture::seeded` for the single-vault cases and
//! `Fixture::seeded_two_vaults` for cross-vault price-time priority.

mod common;

use anchor_lang_v2::bytemuck;
use anchor_v2_testing::Signer;
use common::fixture::{dual_profile, ladder_profile, simple_profile, Fixture};
use dropset::{Price, FLUSH_BIT};
use solana_pubkey::Pubkey;

/// Default seed used across the swap tests.
const SEED_BASE: u64 = 1_000_000;
const SEED_QUOTE: u64 = 1_085_000;

#[test]
fn buy_fills_against_seeded_vault() {
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    // Taker buys base, pays quote. Funded with quote only.
    let taker = f.funded_depositor(0, 200_000);
    let quote_ata = f.quote_ata(&taker.pubkey());
    let q_before = f.token_balance(&quote_ata);

    // Buy with INFINITY limit (no upper bound), spend 100_000 quote,
    // min_out = 1 (any non-zero fill is acceptable).
    f.swap(&taker, 0, 100_000, Price::INFINITY.as_u32(), 1)
        .expect("swap Buy");

    assert!(
        f.token_balance(&f.base_ata(&taker.pubkey())) > 0,
        "taker received base"
    );
    assert!(f.token_balance(&quote_ata) < q_before, "taker spent quote");
    let v = f.vault(0);
    assert!(
        v.base_atoms.get() < SEED_BASE,
        "vault base inventory decreased"
    );
    assert!(
        v.quote_atoms.get() > SEED_QUOTE,
        "vault quote inventory increased"
    );
}

#[test]
fn min_out_soft_reverts_when_unattainable() {
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let taker = f.funded_depositor(0, 200_000);
    let quote_ata = f.quote_ata(&taker.pubkey());
    let q_before = f.token_balance(&quote_ata);
    let nonce_before = f.market_header().nonce.get();
    let vault_before = f.vault(0);

    // min_out is unattainable — the matcher must roll back every
    // mutation and still return Ok so the surrounding tx survives.
    f.swap(&taker, 0, 100_000, Price::INFINITY.as_u32(), u64::MAX)
        .expect("soft-revert swap should still succeed");

    // Taker balances unchanged — no transfers fired.
    assert_eq!(f.token_balance(&quote_ata), q_before);
    assert_eq!(f.token_balance(&f.base_ata(&taker.pubkey())), 0);

    // Vault inventory + market nonce restored to pre-swap.
    let vault_after = f.vault(0);
    assert_eq!(vault_before.base_atoms.get(), vault_after.base_atoms.get());
    assert_eq!(
        vault_before.quote_atoms.get(),
        vault_after.quote_atoms.get()
    );
    assert_eq!(nonce_before, f.market_header().nonce.get());

    // Treasury invariant holds with zero slack — a swap that never
    // committed accrues no fee and, since it transfers nothing at all,
    // leaves no exact-in residue either.
    let h = f.market_header();
    assert_eq!(h.accrued_base_fee_atoms.get(), 0);
    assert_eq!(h.accrued_quote_fee_atoms.get(), 0);
    assert_eq!(
        f.token_balance(&f.base_treasury),
        vault_after.base_atoms.get()
    );
    assert_eq!(
        f.token_balance(&f.quote_treasury),
        vault_after.quote_atoms.get()
    );
}

#[test]
fn soft_revert_unwinds_the_accrued_fee() {
    // With a live taker fee, a soft-reverted swap must leave the market's
    // protocol-fee accumulators exactly where it found them. Otherwise a
    // failed-`min_out` taker mints phantom revenue: the counters would
    // claim treasury atoms that were never charged, so the residual sweep
    // would read the gap as a bug and a future harvest would have a
    // ceiling above the real revenue. The default taker fee is zero, so
    // this is the case the other revert tests can't reach.
    //
    // Every assertion here is `== 0`, which an implementation that never
    // accrued at all would also satisfy — `taker_fee_accrues_to_the_market_not_the_vault`
    // below is the positive control: same fixture, same swap args but a
    // committing `min_out`, asserting the accrual is non-zero. The pair is
    // the witness; neither half proves the unwind alone.
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let admin = f.authority.insecure_clone();
    f.set_taker_fee(&admin, 10_000).expect("1% taker fee");
    let taker = f.funded_depositor(0, 200_000);
    assert_eq!(
        f.market_header().accrued_base_fee_atoms.get(),
        0,
        "clean slate"
    );

    f.swap(&taker, 0, 100_000, Price::INFINITY.as_u32(), u64::MAX)
        .expect("soft-revert swap should still succeed");

    let h = f.market_header();
    assert_eq!(h.accrued_base_fee_atoms.get(), 0, "base accrual unwound");
    assert_eq!(
        h.accrued_quote_fee_atoms.get(),
        0,
        "quote accrual untouched"
    );
    let v = f.vault(0);
    assert_eq!(v.base_atoms.get(), SEED_BASE, "vault base restored");
    assert_eq!(f.token_balance(&f.base_treasury), v.base_atoms.get());
    assert_eq!(f.token_balance(&f.quote_treasury), v.quote_atoms.get());
}

#[test]
fn invalid_side_byte_rejects() {
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let taker = f.funded_depositor(0, 200_000);
    // Side byte 2 — neither Buy (0) nor Sell (1).
    let err = f
        .swap(&taker, 2, 100_000, Price::INFINITY.as_u32(), 0)
        .expect_err("swap with side=2 must reject as InvalidSwapSide");
    common::assert_program_error(&err, dropset::DropsetError::InvalidSwapSide);
}

#[test]
fn sell_side_fills_against_bids() {
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    // Taker sells base, receives quote — exercises the bid-side heap
    // key + sort. Funded with base only.
    let taker = f.funded_depositor(100_000, 0);
    let base_ata = f.base_ata(&taker.pubkey());
    let quote_ata = f.quote_ata(&taker.pubkey());
    let base_before = f.token_balance(&base_ata);

    // Sell with ZERO limit (no lower bound on bid price), min_out = 1.
    f.swap(&taker, 1, 100_000, Price::ZERO.as_u32(), 1)
        .expect("sell fills");

    assert!(
        f.token_balance(&quote_ata) > 0,
        "taker received quote for the base sold"
    );
    assert!(f.token_balance(&base_ata) < base_before, "taker spent base");
    let v = f.vault(0);
    assert!(
        v.base_atoms.get() > SEED_BASE,
        "vault base grew on the buy-from-taker"
    );
    assert!(v.quote_atoms.get() < SEED_QUOTE, "vault quote shrank");
}

/// A dust Sell whose input leg truncates to zero must not fill at all.
///
/// The live localnet case: at any bid above 1, one base atom converts to
/// a single quote atom, and that atom reverse-converts back to **zero**
/// base. Before the WARNING 1f guard the vault paid out the quote atom
/// against an input of nothing — a real transfer, confirmed at the token
/// level, not just a mis-reported `FillEvent`.
#[test]
fn dust_sell_never_pays_out_against_a_zero_input_leg() {
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    // Exactly one base atom — the remainder a sweep leaves behind.
    let taker = f.funded_depositor(1, 0);
    let base_ata = f.base_ata(&taker.pubkey());
    let quote_ata = f.quote_ata(&taker.pubkey());
    let v_before = f.vault(0);

    // `min_out = 0`, so a zero fill is the matcher's own decision rather
    // than the `min_out` soft-revert rolling a bad leg back for us.
    f.swap(&taker, 1, 1, Price::ZERO.as_u32(), 0)
        .expect("dust sell should succeed as a no-op");

    assert_eq!(
        f.token_balance(&quote_ata),
        0,
        "vault must not pay quote against zero base"
    );
    assert_eq!(
        f.token_balance(&base_ata),
        1,
        "the taker's unfillable dust atom stays unspent"
    );
    let v_after = f.vault(0);
    assert_eq!(v_before.base_atoms.get(), v_after.base_atoms.get());
    assert_eq!(v_before.quote_atoms.get(), v_after.quote_atoms.get());
    // Treasury invariant still ties out.
    assert_eq!(
        f.token_balance(&f.quote_treasury),
        v_after.quote_atoms.get()
    );
}

/// The mirror-image half, latent until a market quotes **below** 1: there
/// the free leg is *base*, and the round-trip magnifies it — one quote
/// atom converts to ~16.5k base atoms whose reverse conversion floors
/// back to zero quote. The multi-market FX demo's IDR- and MXN-scale
/// markets are the ones that reach this arm.
#[test]
fn dust_buy_below_price_one_never_hands_out_free_base() {
    let mut f = Fixture::bootstrap();
    let auth = f.authority.insecure_clone();
    f.create_vault(0, f.authority.pubkey(), false, Pubkey::default())
        .expect("create_vault");
    // 0.00006 quote per base — an IDR-scale rate, well below 1.
    let ref_price = Price::encode(60_000_000, -5).unwrap();
    assert_eq!(ref_price.base_for_quote(1), 16_666);
    assert_eq!(ref_price.quote_for_base(16_666), 0);
    f.set_reference_price(&auth, 0, ref_price.as_u32(), 0)
        .expect("set_reference_price");
    f.set_liquidity_profile(&auth, 0, simple_profile(5_000, 10_000, u32::MAX))
        .expect("set_liquidity_profile");
    f.deposit_leader(0, 1_000_000_000, 60_000, 1_000_000_000, 60_000)
        .expect("seed deposit_leader");

    // One quote atom — enough to price a base leg, not enough to pay for
    // it once the reverse conversion truncates.
    let taker = f.funded_depositor(0, 1);
    let base_ata = f.base_ata(&taker.pubkey());
    let quote_ata = f.quote_ata(&taker.pubkey());
    let v_before = f.vault(0);

    f.swap(&taker, 0, 1, Price::INFINITY.as_u32(), 0)
        .expect("dust buy should succeed as a no-op");

    assert_eq!(
        f.token_balance(&base_ata),
        0,
        "vault must not pay base against zero quote"
    );
    assert_eq!(
        f.token_balance(&quote_ata),
        1,
        "the taker's unfillable dust atom stays unspent"
    );
    let v_after = f.vault(0);
    assert_eq!(v_before.base_atoms.get(), v_after.base_atoms.get());
    assert_eq!(v_before.quote_atoms.get(), v_after.quote_atoms.get());
    assert_eq!(f.token_balance(&f.base_treasury), v_after.base_atoms.get());
}

#[test]
fn limit_price_stops_before_level() {
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let taker = f.funded_depositor(0, 200_000);
    let quote_ata = f.quote_ata(&taker.pubkey());
    let q_before = f.token_balance(&quote_ata);

    // Ask sits at ~1.0904 (1.0850 × 1.005). A Buy limit of 1.08 is
    // strictly tighter, so the best (only) level crosses and nothing
    // fills — with min_out = 0 the handler returns Ok with no transfer.
    let tight_limit = Price::encode(10_800_000, 0).unwrap();
    f.swap(&taker, 0, 100_000, tight_limit.as_u32(), 0)
        .expect("swap returns Ok with no fill");

    assert_eq!(f.token_balance(&quote_ata), q_before, "no quote spent");
    assert_eq!(
        f.token_balance(&f.base_ata(&taker.pubkey())),
        0,
        "no base received"
    );
}

#[test]
fn frozen_vault_skipped_from_matching() {
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    // Freeze the only vault — it must drop out of the matching set even
    // though its levels would otherwise be the best (only) price.
    let admin = f.authority.insecure_clone();
    f.freeze_vault(&admin, 0).expect("admin freezes vault");
    let taker = f.funded_depositor(0, 200_000);
    let quote_ata = f.quote_ata(&taker.pubkey());
    let q_before = f.token_balance(&quote_ata);

    f.swap(&taker, 0, 100_000, Price::INFINITY.as_u32(), 0)
        .expect("ok, no fill against a frozen vault");

    assert_eq!(f.token_balance(&quote_ata), q_before, "no quote spent");
    let v = f.vault(0);
    assert_eq!(
        v.base_atoms.get(),
        SEED_BASE,
        "frozen vault inventory untouched"
    );
    assert_eq!(
        v.quote_atoms.get(),
        SEED_QUOTE,
        "frozen vault inventory untouched"
    );
}

/// Stamping the zero sentinel over a live reference price takes the whole
/// vault out of the matching set without touching its `LiquidityProfile`, and
/// the next real price puts the same book straight back.
///
/// This is what makes the maker bot's stale-quote invalidation possible
/// (`bots/maker-bot` → `model::invalidate`): `FreezeVault` is admin-only, so a
/// bot that comes back to find its own quotes resting at a stale price kills
/// them by stamping zero through the ordinary quote-authority hot path. Both
/// halves matter — the kill has to be total, and it has to be reversible by a
/// plain re-quote rather than needing the ladder re-armed.
#[test]
fn zero_reference_price_skips_the_vault_and_a_fresh_price_re_arms_it() {
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let authority = f.authority.insecure_clone();
    let profile_before = bytemuck::bytes_of(&f.vault(0).profile).to_vec();

    // Kill the book: stamp the zero sentinel in place of the 1.0850 anchor.
    f.set_reference_price(&authority, 0, Price::ZERO.as_u32(), 0)
        .expect("quote authority stamps the kill price");

    let taker = f.funded_depositor(0, 200_000);
    let quote_ata = f.quote_ata(&taker.pubkey());
    let q_before = f.token_balance(&quote_ata);
    f.swap(&taker, 0, 100_000, Price::INFINITY.as_u32(), 0)
        .expect("ok, no fill against an invalid reference price");
    assert_eq!(f.token_balance(&quote_ata), q_before, "no quote spent");
    assert_eq!(
        f.token_balance(&f.base_ata(&taker.pubkey())),
        0,
        "no base received"
    );
    let v = f.vault(0);
    assert_eq!(v.base_atoms.get(), SEED_BASE, "inventory untouched");
    assert_eq!(v.quote_atoms.get(), SEED_QUOTE, "inventory untouched");
    assert_eq!(
        bytemuck::bytes_of(&v.profile),
        profile_before.as_slice(),
        "the kill stamp leaves the ladder shape alone"
    );

    // Re-quote at (a hair off) the original anchor: the untouched profile
    // re-materializes and the same taker now fills. The price differs from the
    // fixture's opening stamp only so the transaction isn't a byte-for-byte
    // replay of it.
    let ref_price = Price::encode(10_850_001, 0).unwrap();
    f.set_reference_price(&authority, 0, ref_price.as_u32(), 0)
        .expect("quote authority re-arms the book");
    f.swap(&taker, 0, 100_000, Price::INFINITY.as_u32(), 1)
        .expect("swap fills once the reference is live again");
    assert!(
        f.token_balance(&f.base_ata(&taker.pubkey())) > 0,
        "taker received base after the re-quote"
    );
}

#[test]
fn min_out_boundary_commits_at_equal_and_reverts_one_over() {
    // Probe the achievable net output on a throwaway fixture.
    let achievable = {
        let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
        let taker = f.funded_depositor(0, 200_000);
        f.swap(&taker, 0, 100_000, Price::INFINITY.as_u32(), 1)
            .expect("probe swap");
        f.token_balance(&f.base_ata(&taker.pubkey()))
    };
    assert!(achievable > 0, "probe must fill something");

    // min_out exactly equal to achievable → commits.
    {
        let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
        let taker = f.funded_depositor(0, 200_000);
        f.swap(&taker, 0, 100_000, Price::INFINITY.as_u32(), achievable)
            .expect("min_out == achievable commits");
        assert_eq!(f.token_balance(&f.base_ata(&taker.pubkey())), achievable);
    }
    // min_out one atom over → soft-reverts (Ok, no transfer).
    {
        let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
        let taker = f.funded_depositor(0, 200_000);
        f.swap(&taker, 0, 100_000, Price::INFINITY.as_u32(), achievable + 1)
            .expect("min_out one over soft-reverts");
        assert_eq!(f.token_balance(&f.base_ata(&taker.pubkey())), 0);
    }
}

#[test]
fn nonce_overflow_hard_reverts_and_errors() {
    // The per-leg `market.nonce` bump uses `checked_add(1)` — at
    // `u64::MAX` the next fill can't advance it, so the swap must
    // hard-error `MathOverflow` (like the quote paths) after fully
    // rolling back the leg it had already applied. Drive the
    // (otherwise unreachable) overflow by poking the nonce to its max.
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    f.poke_nonce(u64::MAX);
    assert_eq!(
        f.market_header().nonce.get(),
        u64::MAX,
        "nonce armed at max"
    );

    let taker = f.funded_depositor(0, 200_000);
    let quote_ata = f.quote_ata(&taker.pubkey());
    let q_before = f.token_balance(&quote_ata);

    // A Buy that would otherwise fill — the first leg's bump overflows.
    let err = f
        .swap(&taker, 0, 100_000, Price::INFINITY.as_u32(), 1)
        .expect_err("nonce overflow must hard-error");
    common::assert_program_error(&err, dropset::DropsetError::MathOverflow);

    // No transfers fired — the taker is made whole.
    assert_eq!(f.token_balance(&quote_ata), q_before, "no quote spent");
    assert_eq!(
        f.token_balance(&f.base_ata(&taker.pubkey())),
        0,
        "no base received"
    );

    // Vault inventory and nonce restored exactly; FLUSH_BIT re-armed so
    // the next legitimate taker re-materializes against current state.
    let v = f.vault(0);
    assert_eq!(v.base_atoms.get(), SEED_BASE, "vault base restored");
    assert_eq!(v.quote_atoms.get(), SEED_QUOTE, "vault quote restored");
    assert_eq!(
        f.market_header().nonce.get(),
        u64::MAX,
        "nonce reset to its pre-swap value"
    );
    assert!(
        v.reference_price.stamp.get() & FLUSH_BIT != 0,
        "FLUSH_BIT re-armed after the hard revert"
    );

    // Treasury invariant intact across the failed swap — and no fee
    // accrued, since the hard revert unwinds the accumulators too.
    let h = f.market_header();
    assert_eq!(h.accrued_base_fee_atoms.get(), 0);
    assert_eq!(h.accrued_quote_fee_atoms.get(), 0);
    assert_eq!(f.token_balance(&f.base_treasury), v.base_atoms.get());
    assert_eq!(f.token_balance(&f.quote_treasury), v.quote_atoms.get());
}

#[test]
fn taker_fee_accrues_to_the_market_not_the_vault() {
    // Same swap with and without a taker fee. The fee'd taker receives
    // strictly less base, and the difference lands in the market's
    // protocol-fee accumulator — *not* in the vault, whose inventory is
    // debited the gross output either way (it trades at the price it
    // quoted). Leaving it in the vault would raise depositor NAV and,
    // through `L = isqrt(base·quote)`, read as leader edge at realize time.
    let run = |fee_ppm: u16| {
        let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
        let admin = f.authority.insecure_clone();
        f.set_taker_fee(&admin, fee_ppm).expect("set taker fee");
        let taker = f.funded_depositor(0, 200_000);
        f.swap(&taker, 0, 100_000, Price::INFINITY.as_u32(), 1)
            .expect("swap");
        let v = f.vault(0);
        let h = f.market_header();
        (
            f.token_balance(&f.base_ata(&taker.pubkey())),
            v.base_atoms.get(),
            h.accrued_base_fee_atoms.get(),
            f.token_balance(&f.base_treasury),
        )
    };
    let (no_fee_recv, no_fee_vault_base, no_fee_accrued, no_fee_treasury) = run(0);
    let (fee_recv, fee_vault_base, accrued, treasury) = run(10_000); // 1%

    assert!(
        fee_recv < no_fee_recv,
        "a positive taker fee leaves the taker with less base ({fee_recv} vs {no_fee_recv})"
    );
    assert_eq!(no_fee_accrued, 0, "a zero fee rate accrues nothing");
    assert_eq!(accrued, no_fee_recv - fee_recv, "the fee slice is accrued");
    assert!(accrued > 0, "1% of the fill is a non-zero atom count");
    // The vault gave up the same gross output in both runs — the fee comes
    // out of the taker's proceeds, not out of the vault's quote.
    assert_eq!(
        fee_vault_base, no_fee_vault_base,
        "vault inventory is debited the gross output regardless of the fee"
    );
    assert_eq!(fee_vault_base, SEED_BASE - no_fee_recv);
    // Treasury custody invariant in both runs: the fee atoms stay in the
    // treasury, claimed by the accumulator rather than by the vault.
    assert_eq!(no_fee_treasury, no_fee_vault_base + no_fee_accrued);
    assert_eq!(treasury, fee_vault_base + accrued);
}

#[test]
fn sell_side_accrues_the_quote_leg() {
    // The accrual's leg-select is a hand-duplicated `if is_ask_side`, so
    // the Sell branch needs its own witness: every other fee'd swap in the
    // suite is a Buy, which would leave a base/quote mix-up in the `else`
    // arm completely uncovered.
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let admin = f.authority.insecure_clone();
    f.set_taker_fee(&admin, 10_000).expect("1% taker fee");
    let taker = f.funded_depositor(100_000, 0);

    f.swap(&taker, 1, 100_000, Price::ZERO.as_u32(), 1)
        .expect("fee'd sell");

    let h = f.market_header();
    let v = f.vault(0);
    assert!(
        h.accrued_quote_fee_atoms.get() > 0,
        "a Sell pays out quote, so the fee accrues on the quote leg"
    );
    assert_eq!(
        h.accrued_base_fee_atoms.get(),
        0,
        "the base leg accrues nothing on a Sell"
    );
    // Custody invariant on both legs: the quote treasury carries the
    // accrued term, the base treasury (credited the taker's input) does not.
    assert_eq!(
        f.token_balance(&f.quote_treasury),
        v.quote_atoms.get() + h.accrued_quote_fee_atoms.get()
    );
    // The input leg is the one exact-in leaves change on: the taker's
    // whole 100_000 base reaches the treasury, the vault is credited only
    // what the bid priced, and the single-atom difference is unattributed
    // residual claimed by neither.
    f.assert_treasury_residual(1, 0);
    assert_eq!(
        f.token_balance(&f.base_treasury),
        v.base_atoms.get() + 1,
        "the base treasury holds the vault's credit plus the exact-in residue"
    );
}

#[test]
fn multi_leg_accrual_sums_every_leg() {
    // Accrual is per-leg while the taker is paid once, netted — so the
    // invariant needs `Σ per-leg fee == total_fee`. A single-vault fill
    // can't tell an accrual placed once per *swap* from one placed per
    // *leg*, so drive a spill across both vaults and reconcile the
    // counter against the per-leg `FillEvent`s.
    let hi = Price::encode(10_900_000, 0).unwrap().as_u32();
    let lo = Price::encode(10_800_000, 0).unwrap().as_u32();
    let mut f = Fixture::seeded_two_vaults(hi, lo);
    let admin = f.authority.insecure_clone();
    f.set_taker_fee(&admin, 10_000).expect("1% taker fee");

    // 1_500_000 quote drains the cheaper vault's ask and spills into the
    // pricier one — the same sizing as `multi_vault_spills_cheaper_then_pricier`.
    let taker = f.funded_depositor(0, 1_500_000);
    let meta = f
        .swap_meta(&taker, 0, 1_500_000, Price::INFINITY.as_u32(), 1)
        .expect("large fee'd buy spills across both vaults");

    let fills = common::events::fills(&meta);
    assert!(
        fills.len() >= 2,
        "expected a multi-leg fill, got {}",
        fills.len()
    );
    let leg_fees: u64 = fills.iter().map(|e| e.taker_fee_atoms).sum();
    assert!(leg_fees > 0, "a 1% fee on a spill is a non-zero atom count");

    let h = f.market_header();
    assert_eq!(
        h.accrued_base_fee_atoms.get(),
        leg_fees,
        "the counter is the sum of every leg's fee, not one leg's"
    );
    // Custody invariant across the whole slab, not just one vault.
    let vault_base = f.vault(0).base_atoms.get() + f.vault(1).base_atoms.get();
    assert_eq!(
        f.token_balance(&f.base_treasury),
        vault_base + h.accrued_base_fee_atoms.get()
    );
}

// ── Multi-vault price-time priority + flush / expiry ─────────────────

#[test]
fn multi_vault_cheaper_price_fills_first() {
    // Sector 1 quotes the lower reference (cheaper asks for a Buy), so
    // a small Buy must fill entirely against sector 1 and leave the
    // pricier sector 0 untouched.
    let hi = Price::encode(10_900_000, 0).unwrap().as_u32();
    let lo = Price::encode(10_800_000, 0).unwrap().as_u32();
    let mut f = Fixture::seeded_two_vaults(hi, lo);

    let taker = f.funded_depositor(0, 200_000);
    f.swap(&taker, 0, 50_000, Price::INFINITY.as_u32(), 1)
        .expect("buy fills the cheaper vault");

    // The cheaper vault absorbs the buy. Integer rounding can leave a
    // 1-2 atom quote residual that buys a single base atom off the
    // pricier vault's level, so assert the *bulk* landed on sector 1
    // and sector 0 saw at most rounding dust.
    let fill_cheaper = 1_000_000 - f.vault(1).base_atoms.get();
    let fill_pricier = 1_000_000 - f.vault(0).base_atoms.get();
    assert!(
        fill_cheaper >= 40_000,
        "cheaper vault (sector 1) absorbed the buy (filled {fill_cheaper})"
    );
    assert!(
        fill_pricier <= 2,
        "pricier vault (sector 0) saw only rounding dust (filled {fill_pricier})"
    );
}

#[test]
fn oversize_ask_side_skips_that_vault_but_not_the_swap() {
    // A vault whose ask side sums past BPS must be thrown out of matching,
    // not abort the whole swap. (`set_liquidity_profile` stores such a ladder
    // without complaint, so this is reachable through the instruction too —
    // see `set_liquidity_profile.rs`'s
    // `over_cap_ladder_is_dropped_from_the_book`; the poke here sets one
    // level without the nonce bump and re-armed FLUSH_BIT a real write
    // brings.) Before the match-time gate a
    // single such level made `flush_level_size` `?`-propagate and reject the
    // entire `swap`, so one corrupt vault could DoS every taker. Here sector
    // 0's ask side is corrupted and sector 1 is healthy: the Buy fills
    // entirely against sector 1 and the take succeeds.
    let same = Price::encode(10_850_000, 0).unwrap().as_u32();
    let mut f = Fixture::seeded_two_vaults(same, same);
    f.poke_level_size_bps(0, true, 0, 20_000); // 200% of the leg — Σ ask > BPS

    let taker = f.funded_depositor(0, 200_000);
    f.swap(&taker, 0, 50_000, Price::INFINITY.as_u32(), 1)
        .expect("swap succeeds despite an oversized ask side on sector 0");

    // The oversized-ask vault contributed no asks; the healthy vault absorbed
    // the whole buy. A zeroed side takes no fill at all — not even dust.
    let fill_bad = 1_000_000 - f.vault(0).base_atoms.get();
    let fill_good = 1_000_000 - f.vault(1).base_atoms.get();
    assert_eq!(fill_bad, 0, "the oversized-ask vault contributed no asks");
    assert!(
        fill_good >= 40_000,
        "the healthy vault absorbed the buy (filled {fill_good})"
    );
    // The stored profile bytes are left intact, so the leader's ladder
    // self-heals the moment they resubmit a valid one: the poked 20_000 is
    // still there, untouched by the flush that zeroed only `remaining`.
    assert_eq!(
        f.vault(0).profile.asks[0].size_bps.get(),
        20_000,
        "the stored (corrupt) profile is left intact, not rewritten"
    );
}

#[test]
fn multi_vault_equal_price_older_nonce_wins() {
    // Both vaults anchored at the same 1.0850 reference → identical ask
    // price, so the fill order is decided purely by the price-time
    // tiebreak `(price_key, nonce, sector_idx, …)`. To isolate the
    // *nonce* term from the *sector_idx* term, give the OLDER nonce to
    // the HIGHER-index vault: `seeded_two_vaults` quotes sector 0 first,
    // so re-quote it here to stamp it with the newest nonce. Now sector
    // 1 holds the older nonce but the higher index — if `nonce` breaks
    // the tie, sector 1 fills; if `sector_idx` did, sector 0 would.
    let same = Price::encode(10_850_000, 0).unwrap().as_u32();
    let mut f = Fixture::seeded_two_vaults(same, same);
    // Distinct blockhash so the re-quote isn't a duplicate of sector 0's
    // original set_reference_price txn.
    f.svm.expire_blockhash();
    f.set_reference_price(&f.authority.insecure_clone(), 0, same, 0)
        .expect("re-quote sector 0 with the newest nonce");

    let taker = f.funded_depositor(0, 200_000);
    f.swap(&taker, 0, 50_000, Price::INFINITY.as_u32(), 1)
        .expect("buy fills the older-nonce vault");

    // Sector 1 (older nonce, higher index) absorbs the buy; sector 0
    // (newer nonce, lower index) sees at most a rounding-dust atom.
    let fill_older = 1_000_000 - f.vault(1).base_atoms.get();
    let fill_newer = 1_000_000 - f.vault(0).base_atoms.get();
    assert!(
        fill_older >= 40_000,
        "older-nonce vault (sector 1) filled first (filled {fill_older})"
    );
    assert!(
        fill_newer <= 2,
        "newer-nonce vault (sector 0) untouched on the tie (filled {fill_newer})"
    );
}

#[test]
fn multi_vault_spills_cheaper_then_pricier() {
    // A Buy large enough to exhaust the cheaper vault's ask level then
    // spill into the pricier one. The cheaper sector 1 (full ask = its
    // whole 1_000_000 base) drains to zero before sector 0 is touched.
    let hi = Price::encode(10_900_000, 0).unwrap().as_u32();
    let lo = Price::encode(10_800_000, 0).unwrap().as_u32();
    let mut f = Fixture::seeded_two_vaults(hi, lo);

    // 1_500_000 quote fully drains the cheaper vault's 1_000_000-base
    // ask (~1.0854e6 quote) and spills the rest into the pricier one,
    // leaving sector 0 partially filled.
    let taker = f.funded_depositor(0, 1_500_000);
    f.swap(&taker, 0, 1_500_000, Price::INFINITY.as_u32(), 1)
        .expect("large buy spills across both vaults");

    let v0 = f.vault(0).base_atoms.get();
    let v1 = f.vault(1).base_atoms.get();
    assert_eq!(v1, 0, "cheaper vault drained first");
    assert!(
        v0 < 1_000_000,
        "pricier vault partially filled by the spillover"
    );
    assert!(v0 > 0, "pricier vault not fully drained");
    assert!(
        v1 < v0,
        "cheaper vault is more depleted than the pricier one"
    );
}

#[test]
fn a_level_capped_fill_then_an_unaffordable_level_leaves_the_change() {
    // The exact-in walk shape the residue accounting is easiest to get
    // wrong, end-to-end through the real handler: a first leg that fills
    // but is *level*-capped, then a second leg the taker cannot afford one
    // output atom of, which stops the walk. Neither leg may charge beyond
    // the priced input of what actually filled — a level-capped leg is not
    // `taker_bound`, so it charges no residue, and `LegFill::Exhausted`
    // deliberately absorbs nothing at all (see its doc: absorbing there
    // would let a leader post a far-out tail level and confiscate the
    // unspent budget of any taker whose walk reached it, with `min_out` no
    // defense since residue never reduces output).
    //
    // Asserting the taker's *remaining input* is the whole point. The
    // first cut of exact-in absorbed the unspent budget on the stopping
    // leg, and the output amount cannot see that — which is exactly why
    // `min_out` could not object to it.
    let anchor = Price::encode(10_850_000, 0).unwrap().as_u32();
    let mut f = Fixture::seeded_two_vaults(anchor, anchor);
    let auth = f.authority.insecure_clone();

    // Re-quote sector 0 with a single ask worth 1% of its base inventory
    // — 10_000 of 1_000_000 atoms — so the *level* cap binds well before
    // the taker's budget does. A real profile write bumps the nonce and
    // re-arms FLUSH_BIT, so the 100-bps ladder is what materializes.
    f.set_liquidity_profile(&auth, 0, ladder_profile(&[(5_000, 100, u32::MAX)], &[]))
        .expect("sector 0 quotes one 100-bps ask");
    // Re-stamp sector 1 at an astronomical reference, re-arming its
    // FLUSH_BIT so its full ask ladder re-materializes there. The level is
    // non-empty and its vault is funded — the only thing wrong with it is
    // the price, which is what makes it the `Exhausted` arm rather than a
    // `Skip`. Asks are visited cheapest-first, so it is reached last.
    let far_out = Price::encode(10_000_000, 9).unwrap(); // 1e9 quote/base atom
    f.set_reference_price(&auth, 1, far_out.as_u32(), 0)
        .expect("sector 1 re-quotes far out of reach");

    let budget = 200_000u64;
    let taker = f.funded_depositor(0, budget);
    f.assert_treasury_invariant();
    f.swap(&taker, 0, budget, Price::INFINITY.as_u32(), 1)
        .expect("the level-capped leg fills and the walk then stops");

    // Leg 1 was level-capped, not taker-capped: the exact 10_000 is the
    // level's materialized size, and matching it is what makes this the
    // level-capped shape rather than a taker-bound one. The gap is wide —
    // this budget can afford ~182_000 base at this price, so the level cap
    // binds by more than an order of magnitude and no rounding can blur
    // which of the two bound.
    let filled = SEED_BASE - f.vault(0).base_atoms.get();
    assert_eq!(filled, 10_000, "the 100-bps level cap bound the fill");
    // The vault is debited the gross output either way; a taker fee is
    // skimmed off it and accrued to the market, so account for it rather
    // than depending on the registry's default rate being zero.
    let accrued_base = f.market_header().accrued_base_fee_atoms.get();
    assert_eq!(
        f.token_balance(&f.base_ata(&taker.pubkey())) + accrued_base,
        filled,
        "the gross fill splits into the taker's payout and the accrued fee"
    );

    // Leg 2 stopped the walk without filling anything — and pin that it was
    // a real, live, non-empty level the walk actually reached, rather than
    // an absent or empty one. `remaining` materializes lazily at match
    // time and sector 1 has never been swapped against, so a full-size
    // level here is positive proof the walk visited it and flushed its
    // ladder. Vault inventory alone cannot carry that: it reads identically
    // whether the level returned `Exhausted`, returned `Skip` because it
    // was empty, or was never considered at all — and only the first of
    // those exercises the arm this test exists to pin. Without this
    // assertion a change that stopped sector 1 materializing would leave
    // the test green and silently vacuous.
    assert_eq!(
        f.vault(1).remaining.asks[0].size.get(),
        SEED_BASE,
        "sector 1's ask materialized at full size and went wholly unfilled"
    );
    assert_eq!(
        f.vault(1).base_atoms.get(),
        SEED_BASE,
        "the unaffordable level filled nothing"
    );

    // The headline: the taker paid exactly the priced input leg of what
    // filled and kept every other atom of `amount_in`, rather than the
    // whole budget the pre-merge bug charged. The taker fee is skimmed off
    // the *output* leg, so on a Buy nothing but residue can separate the
    // taker's quote debit from the vault's quote credit — equality here is
    // the residue-free assertion, stated in atoms the taker can feel.
    let vault_credit = f.vault(0).quote_atoms.get() - SEED_QUOTE;
    let spent = budget - f.token_balance(&f.quote_ata(&taker.pubkey()));
    assert_eq!(
        spent, vault_credit,
        "the taker paid the priced input leg and nothing more"
    );
    assert!(
        spent < budget / 10,
        "a 1%-of-inventory fill costs a fraction of the budget, \
         spent {spent} of {budget}"
    );

    // And the custody relation. `treasury_residual` checks the `>=` itself,
    // and residue is the only thing a take can leave unattributed, so zero
    // slack is the direct assertion that neither leg absorbed any: a change
    // that absorbed on either arm shows up here as slack. Note this pins
    // only that direction — a change re-tightening custody to equality
    // would still satisfy `(0, 0)`, and is caught instead by the suite's
    // non-zero-residual witnesses on taker-bound fills.
    f.assert_treasury_invariant();
}

#[test]
fn flush_re_materializes_after_reference_price_change() {
    // First Buy materializes `remaining` from the 1.0850 ladder. After
    // a much higher reference is stamped (re-arming FLUSH_BIT), an
    // identical Buy must re-materialize at the new (worse-for-taker)
    // ask and return less base.
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let t1 = f.funded_depositor(0, 200_000);
    f.swap(&t1, 0, 50_000, Price::INFINITY.as_u32(), 1)
        .expect("first buy");
    let got1 = f.token_balance(&f.base_ata(&t1.pubkey()));

    let higher = Price::encode(13_000_000, 0).unwrap(); // 1.30
    f.set_reference_price(&f.authority.insecure_clone(), 0, higher.as_u32(), 0)
        .expect("raise reference, re-arms FLUSH_BIT");

    let t2 = f.funded_depositor(0, 200_000);
    f.swap(&t2, 0, 50_000, Price::INFINITY.as_u32(), 1)
        .expect("second buy at the new price");
    let got2 = f.token_balance(&f.base_ata(&t2.pubkey()));

    assert!(
        got2 < got1,
        "higher reference re-materialized a worse ask: {got2} < {got1}"
    );
}

#[test]
fn expired_levels_are_skipped() {
    // Re-profile the seeded vault with a 1-second expiry, advance wall
    // time well past it, then Buy: every level has expired, so nothing
    // fills. The reshape leaves `reference_price` — datum included —
    // alone, so the levels materialize against the seeded quote's
    // `quote_unix`.
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    f.set_liquidity_profile(
        &f.authority.insecure_clone(),
        0,
        simple_profile(5_000, 10_000, 1),
    )
    .expect("short-expiry profile");
    f.warp_unix(100);

    let taker = f.funded_depositor(0, 200_000);
    let q_before = f.token_balance(&f.quote_ata(&taker.pubkey()));
    f.swap(&taker, 0, 50_000, Price::INFINITY.as_u32(), 0)
        .expect("ok, all levels expired");

    assert_eq!(
        f.token_balance(&f.quote_ata(&taker.pubkey())),
        q_before,
        "no quote spent against expired levels"
    );
    assert_eq!(
        f.vault(0).base_atoms.get(),
        SEED_BASE,
        "inventory untouched"
    );
}

/// The gate is `<=` dead, pinned **engine-side** at the exact boundary.
///
/// The conformance vectors already pin `<=` for the *simulator*, and
/// `sdk_conformance` pins simulator == engine — but only at live clocks,
/// so a `<` vs `<=` slip in the engine alone would ship as a one-second
/// SDK-vs-engine divergence: the book shows a level the engine drops, and
/// the taker eats a `min_out` revert. Sit exactly on the deadline here so
/// the engine owns its own boundary.
#[test]
fn a_level_is_dead_at_exactly_its_wall_deadline() {
    const TIF: u32 = 60;
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    f.set_liquidity_profile(
        &f.authority.insecure_clone(),
        0,
        simple_profile(5_000, 10_000, TIF),
    )
    .expect("bounded-TIF profile");

    // Re-stamp the same price at a known datum, so the level's deadline
    // is exactly `datum + TIF` rather than whatever the seed stamped.
    let price_bits = f.vault(0).reference_price.price.as_u32();
    let datum = f.now_unix().get();
    f.svm.expire_blockhash();
    f.set_reference_price_at(&f.authority.insecure_clone(), 0, price_bits, 0, datum)
        .expect("fresh quote at a known datum");

    // One second short of the deadline: still live.
    f.warp_unix((TIF - 1) as i64);
    let taker = f.funded_depositor(0, 400_000);
    let before = f.token_balance(&f.base_ata(&taker.pubkey()));
    f.swap(&taker, 0, 50_000, Price::INFINITY.as_u32(), 1)
        .expect("a level one second inside its deadline still fills");
    assert!(
        f.token_balance(&f.base_ata(&taker.pubkey())) > before,
        "the level is live at deadline - 1"
    );

    // Exactly on the deadline: dead. `expires_at <= now`, not `<`.
    f.warp_unix(1);
    let q_before = f.token_balance(&f.quote_ata(&taker.pubkey()));
    f.swap(&taker, 0, 50_000, Price::INFINITY.as_u32(), 0)
        .expect("ok, the level expired");
    assert_eq!(
        f.token_balance(&f.quote_ata(&taker.pubkey())),
        q_before,
        "no quote spent: the deadline second itself is dead"
    );
}

/// Expiry skips the *vault*, it does not abort the *take*. The cheaper
/// vault — the one that would otherwise absorb the whole fill — is aged
/// out, and the buy must still fill against its pricier, live sibling.
///
/// This is the property that makes stratified expiry safe to rely on: one
/// leader letting its book go stale degrades the book by its own depth
/// rather than taking the market down with it.
#[test]
fn an_expired_vault_is_skipped_while_its_live_sibling_still_fills() {
    let hi = Price::encode(10_900_000, 0).unwrap().as_u32();
    let lo = Price::encode(10_800_000, 0).unwrap().as_u32();
    let mut f = Fixture::seeded_two_vaults(hi, lo);

    // Sector 1 quotes the better price, so it has price priority. Give it
    // a 1-second wall TIF and age it out; sector 0 keeps the
    // never-expiring ladder it was seeded with.
    f.set_liquidity_profile(
        &f.authority.insecure_clone(),
        1,
        simple_profile(5_000, 10_000, 1),
    )
    .expect("short-expiry profile on the cheaper vault");
    f.warp_unix(100);

    let taker = f.funded_depositor(0, 200_000);
    let base_before = f.token_balance(&f.base_ata(&taker.pubkey()));
    let expired_base_before = f.vault(1).base_atoms.get();
    let live_base_before = f.vault(0).base_atoms.get();

    f.swap(&taker, 0, 50_000, Price::INFINITY.as_u32(), 1)
        .expect("the take still fills against the live sibling");

    assert!(
        f.token_balance(&f.base_ata(&taker.pubkey())) > base_before,
        "the taker received base: an expired vault must not abort the take"
    );
    assert_eq!(
        f.vault(1).base_atoms.get(),
        expired_base_before,
        "the expired vault is skipped, not filled"
    );
    assert!(
        f.vault(0).base_atoms.get() < live_base_before,
        "the live sibling is what filled, despite its worse price"
    );
}

/// Expiry is the **min of two bounds**, so each domain has to kill a level
/// on its own. Here the wall bound is wide open and only the slot bound has
/// passed — the level must still be dead. The mirror case (wall dead, slot
/// open) is `expired_levels_are_skipped` above, which leaves the slot side
/// unbounded.
///
/// Together the two pin that neither conjunct was dropped: removing the
/// slot compare turns this test green-to-red, removing the wall compare
/// does the same to its sibling.
#[test]
fn a_passed_slot_bound_kills_a_level_whose_wall_bound_is_still_open() {
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    // Hours of wall life, but only two slots of it.
    f.set_liquidity_profile(
        &f.authority.insecure_clone(),
        0,
        dual_profile(5_000, 10_000, 86_400, 2),
    )
    .expect("slot-bounded profile");
    // Advance slots past the bound while wall time barely moves, which is
    // the ordinary case: slots tick ~2.5x a second.
    f.warp_slots(50);

    let taker = f.funded_depositor(0, 200_000);
    let q_before = f.token_balance(&f.quote_ata(&taker.pubkey()));
    f.swap(&taker, 0, 50_000, Price::INFINITY.as_u32(), 0)
        .expect("ok, the slot bound expired every level");

    assert_eq!(
        f.token_balance(&f.quote_ata(&taker.pubkey())),
        q_before,
        "no quote spent: the slot bound passed even though the wall bound holds"
    );
    assert_eq!(
        f.vault(0).base_atoms.get(),
        SEED_BASE,
        "inventory untouched"
    );
}

/// A zero offset is dead **in either domain**, whatever the datum says —
/// materialization encodes it as the zero deadline rather than letting the
/// bare datum stand in. Pinned here on the slot axis with a live wall TIF,
/// so a regression that dropped the zero encoding would leave the level
/// matchable.
#[test]
fn a_zero_slot_offset_is_dead_even_with_wall_life_remaining() {
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    f.set_liquidity_profile(
        &f.authority.insecure_clone(),
        0,
        dual_profile(5_000, 10_000, 86_400, 0),
    )
    .expect("zero slot-offset profile");

    let taker = f.funded_depositor(0, 200_000);
    let q_before = f.token_balance(&f.quote_ata(&taker.pubkey()));
    f.swap(&taker, 0, 50_000, Price::INFINITY.as_u32(), 0)
        .expect("ok, a zero offset is dead");

    assert_eq!(
        f.token_balance(&f.quote_ata(&taker.pubkey())),
        q_before,
        "a zero slot offset never matches, however long the wall TIF"
    );
}

#[test]
fn min_out_soft_revert_restores_multiple_legs_and_rearms_flush() {
    // A vault with two ask levels. A Buy that crosses both, then fails
    // its `min_out`, must restore *both* levels' remaining size and
    // re-arm FLUSH_BIT.
    let mut f = Fixture::seeded_two_ask_levels();

    let taker = f.funded_depositor(0, 5_000_000);
    let q_before = f.token_balance(&f.quote_ata(&taker.pubkey()));
    // Big enough to fill both 500_000-base levels; min_out unattainable.
    f.swap(&taker, 0, 2_000_000, Price::INFINITY.as_u32(), u64::MAX)
        .expect("soft-revert returns Ok");

    let v = f.vault(0);
    // Each level materialized to base_atoms * 5_000 / 10_000 = 500_000;
    // the revert must restore both to that full size.
    assert_eq!(v.remaining.asks[0].size.get(), 500_000, "level 0 restored");
    assert_eq!(v.remaining.asks[1].size.get(), 500_000, "level 1 restored");
    assert!(
        v.reference_price.stamp.get() & FLUSH_BIT != 0,
        "FLUSH_BIT re-armed after soft-revert"
    );
    assert_eq!(
        f.token_balance(&f.quote_ata(&taker.pubkey())),
        q_before,
        "taker spent nothing on the reverted swap"
    );
    assert_eq!(v.base_atoms.get(), 1_000_000, "vault inventory restored");
}

#[test]
fn nonce_overflow_on_second_leg_hard_reverts_the_committed_first_leg() {
    // `nonce_overflow_hard_reverts_and_errors` pokes `nonce = u64::MAX`,
    // so the *first* leg overflows and only one leg is ever committed —
    // the multi-leg overflow path (a leg commits, bumps the nonce, then a
    // later leg overflows and the loop breaks into the shared revert with
    // more than one snapshot in hand) is never exercised. Pin it here
    // with a two-ask-level vault and a Buy large enough to cross both
    // levels, so the fill loop does exactly two per-leg `nonce` bumps.
    //
    // The contrast between the two arms below proves the bumps are
    // per-leg and the overflow lands on the *second* one:
    //   * armed at `u64::MAX - 2`: leg 0 bumps to `u64::MAX - 1`, leg 1
    //     bumps to `u64::MAX` — both fit, so the swap commits.
    //   * armed at `u64::MAX - 1`: leg 0 bumps to `u64::MAX`, then leg 1's
    //     `checked_add(1)` overflows and the swap hard-errors.
    // A handler that bumped once-per-swap (or overflowed on leg 0) could
    // not produce this exact boundary.

    // Arm one below the wrap point: both per-leg bumps fit, swap commits.
    {
        let mut f = Fixture::seeded_two_ask_levels();
        f.poke_nonce(u64::MAX - 2);
        let taker = f.funded_depositor(0, 5_000_000);
        f.swap(&taker, 0, 2_000_000, Price::INFINITY.as_u32(), 1)
            .expect("both per-leg bumps fit (u64::MAX-2 → u64::MAX)");
        assert!(
            f.token_balance(&f.base_ata(&taker.pubkey())) > 0,
            "taker received base across both committed legs"
        );
        assert_eq!(
            f.market_header().nonce.get(),
            u64::MAX,
            "two legs advanced the nonce by exactly two (u64::MAX-2 → u64::MAX)"
        );
    }

    // Arm at the wrap point: leg 0 commits and bumps to u64::MAX, leg 1
    // overflows. The erroring instruction discards the whole transaction,
    // so every committed-leg mutation is rolled back to the pre-swap seed
    // — exactly what `nonce_overflow_hard_reverts_and_errors` asserts for
    // the single-leg case, now with a committed leg ahead of the failing
    // one.
    {
        let mut f = Fixture::seeded_two_ask_levels();
        f.poke_nonce(u64::MAX - 1);

        let taker = f.funded_depositor(0, 5_000_000);
        let q_before = f.token_balance(&f.quote_ata(&taker.pubkey()));
        // Snapshot the full pre-swap vault state to assert against.
        let v_before = f.vault(0);

        let err = f
            .swap(&taker, 0, 2_000_000, Price::INFINITY.as_u32(), 1)
            .expect_err("second-leg nonce overflow must hard-error");
        common::assert_program_error(&err, dropset::DropsetError::MathOverflow);

        // Taker untouched — no input spent, no output received.
        assert_eq!(
            f.token_balance(&f.quote_ata(&taker.pubkey())),
            q_before,
            "no quote spent"
        );
        assert_eq!(
            f.token_balance(&f.base_ata(&taker.pubkey())),
            0,
            "no base received"
        );

        // Every field is back at its pre-swap value: the committed leg 0
        // (inventory debit + level decrement + nonce bump to u64::MAX) and
        // the partially-applied leg 1 are both gone.
        let v = f.vault(0);
        assert_eq!(
            v.base_atoms.get(),
            v_before.base_atoms.get(),
            "vault base restored to pre-swap"
        );
        assert_eq!(
            v.quote_atoms.get(),
            v_before.quote_atoms.get(),
            "vault quote restored to pre-swap"
        );
        assert_eq!(
            v.remaining.asks[0].size.get(),
            v_before.remaining.asks[0].size.get(),
            "level 0 (committed leg) restored to pre-swap"
        );
        assert_eq!(
            v.remaining.asks[1].size.get(),
            v_before.remaining.asks[1].size.get(),
            "level 1 (partially-applied leg) restored to pre-swap"
        );
        assert_eq!(
            f.market_header().nonce.get(),
            u64::MAX - 1,
            "nonce reset to its pre-swap value, not the leg-0-bumped u64::MAX"
        );
        // FLUSH_BIT armed so the next legitimate taker re-materializes.
        assert!(
            v.reference_price.stamp.get() & FLUSH_BIT != 0,
            "FLUSH_BIT still armed after the rolled-back swap"
        );
        // Treasury invariant intact across the failed multi-leg swap.
        assert_eq!(f.token_balance(&f.base_treasury), v.base_atoms.get());
        assert_eq!(f.token_balance(&f.quote_treasury), v.quote_atoms.get());
    }
}

// ── Caller-declared platform fee ─────────────────────────────────────
//
// The integrator fee: permissionless, bounded by the market's
// `max_platform_fee`, skimmed off the output leg *after* the taker fee and
// paid straight through to the integrator's token account.
//
// The invariant these tests are really guarding is that the fee splits the
// taker's payout rather than adding to what leaves the treasury. Every case
// that fills therefore re-asserts the custody invariant afterwards via
// `assert_treasury_residual` — the three-term
// `treasury >= Σ vault + accrued_<leg>_fee_atoms`, since the taker fee is
// accrued to the market rather than left in the vault. The fee moves no
// vault state and accrues none of its own, so a change that made it draw
// extra atoms out of the treasury would surface right there rather than as
// a slow depositor shortfall.
//
// The slack each case pins is the exact-in residue on the *input* leg: a
// taker-bound fill consumes the caller's whole `amount_in`, and the part no
// level could price is unattributed residual rather than anyone's revenue.
// Pinning the exact atom count is what keeps that distinct from the fee —
// a fee that quietly grew by an atom would otherwise hide inside a `>=`.

/// 100 bps — the registry-seeded ceiling every fixture market starts at, so
/// a test declaring exactly this exercises the boundary of what is allowed.
const CEILING_BPS: u16 = 100;

#[test]
fn platform_fee_pays_the_integrator_and_creates_their_ata() {
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let taker = f.funded_depositor(0, 200_000);
    // The integrator is just a pubkey — no onboarding, no signature. It
    // never signs this transaction, which is the permissionless property.
    let integrator = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS);
    let fee_ata = f.platform_fee_ata(&integrator.pubkey(), 0);

    // The destination does not exist yet: the engine's `create_idempotent`
    // CPI is what brings it into being, funded by the taker.
    assert_eq!(
        f.maybe_token_balance(&fee_ata),
        None,
        "the integrator has no base ATA before the first fee-bearing swap"
    );

    let meta = f
        .swap_with_fee_meta(
            &taker,
            0,
            100_000,
            Price::INFINITY.as_u32(),
            1,
            &integrator.pubkey(),
            CEILING_BPS,
        )
        .expect("swap with a declared platform fee");

    let fee_paid = f
        .maybe_token_balance(&fee_ata)
        .expect("the fee ATA was created by the swap");
    assert!(fee_paid > 0, "the integrator was actually paid");

    // The event is the integrator's only on-chain receipt — the fee accrues
    // no state, so this is what revenue gets reconciled against.
    let ev = common::events::platform_fee(&meta);
    assert_eq!(ev.market, f.market.to_bytes());
    assert_eq!(ev.taker, taker.pubkey().to_bytes());
    assert_eq!(ev.fee_authority, integrator.pubkey().to_bytes());
    assert_eq!(
        ev.mint,
        f.base_mint.to_bytes(),
        "a Buy pays the fee in the base (output) mint"
    );
    assert_eq!(ev.atoms, fee_paid, "the event's atoms match the transfer");
    assert_eq!(ev.platform_fee_bps, CEILING_BPS);

    // The fee split the payout; it did not conjure atoms. Everything that
    // left the treasury went to either the taker or the integrator, so the
    // custody invariant still ties out — the fee moved no vault state and
    // accrued nothing of its own. The one quote atom of slack is the
    // exact-in residue on the taker's input leg, which the fee path must
    // leave exactly where the fee-free path does.
    f.assert_treasury_residual(0, 1);
}

#[test]
fn platform_fee_splits_the_payout_without_touching_the_vault() {
    // The same swap with and without a declared fee. The vault trades
    // identically in both — same input booked, same output debited — and the
    // fee comes entirely out of what the taker would otherwise have
    // received. This is the property that keeps the fee off the LPs' backs.
    let run = |bps: u16| {
        let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
        let admin = f.authority.insecure_clone();
        // A live taker fee too, so the composition order is under test and
        // not just the platform fee in isolation.
        f.set_taker_fee(&admin, 1_000).expect("set taker fee");
        let taker = f.funded_depositor(0, 200_000);
        let integrator = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS);
        let fee_ata = f.platform_fee_ata(&integrator.pubkey(), 0);
        let quote_before = f.token_balance(&f.quote_ata(&taker.pubkey()));

        f.swap_with_fee_meta(
            &taker,
            0,
            100_000,
            Price::INFINITY.as_u32(),
            1,
            &integrator.pubkey(),
            bps,
        )
        .expect("swap");

        let v = f.vault(0);
        // Custody invariant, per arm. This arm carries a live taker fee, so
        // the accrued term is non-zero and the two-term form would fail
        // here; the one quote atom of slack is the exact-in residue on the
        // taker's input leg, identical in both arms.
        f.assert_treasury_residual(0, 1);
        (
            f.token_balance(&f.base_ata(&taker.pubkey())),
            f.maybe_token_balance(&fee_ata).unwrap_or(0),
            quote_before - f.token_balance(&f.quote_ata(&taker.pubkey())),
            v.base_atoms.get(),
            v.quote_atoms.get(),
        )
    };

    let (free_recv, free_fee, free_spent, free_vb, free_vq) = run(0);
    let (paid_recv, paid_fee, paid_spent, paid_vb, paid_vq) = run(CEILING_BPS);

    assert_eq!(free_fee, 0, "a zero rate pays the integrator nothing");
    assert!(paid_fee > 0, "the fixture is large enough to show a fee");

    // The vault is indifferent to the platform fee: identical inventory
    // after, and the taker paid in exactly the same amount.
    assert_eq!(paid_spent, free_spent, "the taker's input leg is unchanged");
    assert_eq!(paid_vb, free_vb, "vault base inventory is unchanged");
    assert_eq!(paid_vq, free_vq, "vault quote inventory is unchanged");

    // What the taker gives up is exactly what the integrator gains.
    assert_eq!(
        paid_recv + paid_fee,
        free_recv,
        "taker payout + integrator fee == the no-fee payout"
    );
    // 100 bps of the taker-fee-net output, rounded down — the same
    // composition `platform_fee_atoms` pins and the SDK simulator mirrors.
    assert_eq!(paid_fee, free_recv * u64::from(CEILING_BPS) / 10_000);
}

#[test]
fn platform_fee_on_a_sell_pays_in_the_quote_mint() {
    // Mirror of the Buy case: a Sell's output leg is quote, so the fee lands
    // in the integrator's *quote* ATA. A leg mix-up in the engine would pay
    // the wrong mint here (and be rejected by the ATA program), so this is
    // the case the Buy test structurally cannot catch.
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let taker = f.funded_depositor(200_000, 0);
    let integrator = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS);
    let quote_fee_ata = f.platform_fee_ata(&integrator.pubkey(), 1);
    let base_fee_ata = f.platform_fee_ata(&integrator.pubkey(), 0);

    let meta = f
        .swap_with_fee_meta(
            &taker,
            1,
            100_000,
            Price::ZERO.as_u32(),
            1,
            &integrator.pubkey(),
            CEILING_BPS,
        )
        .expect("swap Sell with a declared platform fee");

    let fee_paid = f
        .maybe_token_balance(&quote_fee_ata)
        .expect("the quote-leg fee ATA was created");
    assert!(fee_paid > 0, "the integrator was paid in quote");
    assert_eq!(
        f.maybe_token_balance(&base_fee_ata),
        None,
        "the base-leg ATA is untouched on a Sell"
    );

    let ev = common::events::platform_fee(&meta);
    assert_eq!(
        ev.mint,
        f.quote_mint.to_bytes(),
        "a Sell pays the fee in the quote (output) mint"
    );
    assert_eq!(ev.atoms, fee_paid);

    // A Sell pays base in, so the exact-in residue lands on the base leg —
    // the mirror of the Buy case's quote-leg atom.
    f.assert_treasury_residual(1, 0);
}

#[test]
fn platform_fee_above_the_market_ceiling_rejects() {
    // One bps over the seeded ceiling. A hard error, not a clamp: filling at
    // a rate the caller did not ask for would hand the integrator a quote
    // they never agreed to and give the taker no signal at all.
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let taker = f.funded_depositor(0, 200_000);
    let integrator = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS);
    let quote_before = f.token_balance(&f.quote_ata(&taker.pubkey()));

    let err = f
        .swap_with_fee_meta(
            &taker,
            0,
            100_000,
            Price::INFINITY.as_u32(),
            1,
            &integrator.pubkey(),
            CEILING_BPS + 1,
        )
        .expect_err("an over-ceiling platform fee must reject the swap");
    common::assert_program_error(&err, dropset::DropsetError::PlatformFeeTooHigh);

    // Rejected before any matching work — the taker is untouched.
    assert_eq!(
        f.token_balance(&f.quote_ata(&taker.pubkey())),
        quote_before,
        "no input spent"
    );
    assert_eq!(f.vault(0).base_atoms.get(), SEED_BASE, "vault untouched");
}

#[test]
fn platform_fee_ceiling_of_zero_turns_integrator_fees_off() {
    // An admin can decline platform fees on a market outright. With the
    // ceiling at zero every non-zero declaration is rejected, while a
    // zero-rate swap still fills normally.
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let admin = f.authority.insecure_clone();
    f.set_max_platform_fee(&admin, 0).expect("ceiling to zero");
    let integrator = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS);

    let taker = f.funded_depositor(0, 200_000);
    let err = f
        .swap_with_fee_meta(
            &taker,
            0,
            100_000,
            Price::INFINITY.as_u32(),
            1,
            &integrator.pubkey(),
            1,
        )
        .expect_err("1 bps against a 0 bps ceiling must reject");
    common::assert_program_error(&err, dropset::DropsetError::PlatformFeeTooHigh);

    // The ordinary no-integrator swap is unaffected.
    f.swap(&taker, 0, 100_000, Price::INFINITY.as_u32(), 1)
        .expect("a zero-rate swap still fills with the ceiling at zero");
    assert!(f.token_balance(&f.base_ata(&taker.pubkey())) > 0);
}

#[test]
fn platform_fee_without_its_accounts_rejects() {
    // A non-zero rate with the optional group absent has nowhere to send the
    // fee. Rejecting is the honest outcome: silently skipping the transfer
    // would quote a fee and then pocket it for the taker.
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let taker = f.funded_depositor(0, 200_000);
    // `None` for the fee group — exactly what the direct paths send — but
    // paired with a non-zero rate.
    let ix = f.swap_ix_with_fee(
        &taker.pubkey(),
        0,
        100_000,
        Price::INFINITY.as_u32(),
        1,
        None,
        CEILING_BPS,
    );
    let err = f
        .send_ix(&taker, ix)
        .expect_err("a declared fee with no destination must reject");
    common::assert_program_error(&err, dropset::DropsetError::MissingPlatformFeeAccounts);
    assert_eq!(f.vault(0).base_atoms.get(), SEED_BASE, "vault untouched");
}

#[test]
fn platform_fee_rounding_to_zero_pays_nothing_and_emits_nothing() {
    // Below one atom of fee the integrator earns nothing and the taker keeps
    // the dust, matching the taker fee's rule. No transfer means no
    // `PlatformFeeEvent` either — an indexer must not see a zero-atom fee
    // record, and the destination ATA is not even created.
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let taker = f.funded_depositor(0, 200_000);
    let integrator = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS);
    let fee_ata = f.platform_fee_ata(&integrator.pubkey(), 0);

    // 1 bps on a ~900-atom output rounds to 0.09 → 0.
    let meta = f
        .swap_with_fee_meta(
            &taker,
            0,
            1_000,
            Price::INFINITY.as_u32(),
            1,
            &integrator.pubkey(),
            1,
        )
        .expect("the swap itself still fills");

    assert!(
        f.token_balance(&f.base_ata(&taker.pubkey())) > 0,
        "the taker still received their fill"
    );
    assert_eq!(
        f.maybe_token_balance(&fee_ata),
        None,
        "a fee that rounds to zero doesn't even create the destination ATA"
    );
    assert_eq!(
        common::events::count::<dropset::PlatformFeeEvent>(&meta),
        0,
        "no PlatformFeeEvent for a zero-atom fee"
    );
}

#[test]
fn platform_fee_tightens_the_min_out_floor() {
    // `min_out` is checked against the output net of *both* fees — what
    // actually lands in the taker's ATA. Pin it at the boundary: a floor set
    // to the fee-inclusive payout commits, and one atom above it
    // soft-reverts. If the engine compared against the taker-fee-net figure
    // instead, the second arm would commit too, and a route could declare
    // the market's maximum fee while still clearing a floor the taker sized
    // against a quote that never contemplated it.
    let probe = {
        let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
        let taker = f.funded_depositor(0, 200_000);
        let integrator = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS);
        f.swap_with_fee_meta(
            &taker,
            0,
            100_000,
            Price::INFINITY.as_u32(),
            1,
            &integrator.pubkey(),
            CEILING_BPS,
        )
        .expect("probe swap");
        f.token_balance(&f.base_ata(&taker.pubkey()))
    };
    assert!(probe > 0, "probe produced a fill to anchor the boundary");

    let run = |min_out: u64| {
        let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
        let taker = f.funded_depositor(0, 200_000);
        let integrator = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS);
        let fee_ata = f.platform_fee_ata(&integrator.pubkey(), 0);
        f.swap_with_fee_meta(
            &taker,
            0,
            100_000,
            Price::INFINITY.as_u32(),
            min_out,
            &integrator.pubkey(),
            CEILING_BPS,
        )
        .expect("min_out is a soft revert, never an error");
        (
            f.token_balance(&f.base_ata(&taker.pubkey())),
            f.maybe_token_balance(&fee_ata).unwrap_or(0),
            f.vault(0).base_atoms.get(),
        )
    };

    // Exactly the fee-inclusive payout: commits.
    let (recv, fee, _) = run(probe);
    assert_eq!(recv, probe, "a floor at the net payout commits");
    assert!(fee > 0, "and the integrator is paid");

    // One atom above it: soft-reverts, so nobody is paid and the book is
    // untouched.
    let (recv, fee, vault_base) = run(probe + 1);
    assert_eq!(recv, 0, "one atom above the net payout soft-reverts");
    assert_eq!(fee, 0, "a soft-reverted swap pays the integrator nothing");
    assert_eq!(vault_base, SEED_BASE, "vault inventory restored");
}

#[test]
fn platform_fee_reuses_an_existing_integrator_ata() {
    // Second fee-bearing swap to the same integrator: the ATA already
    // exists, so `create_idempotent` is a no-op and the fee accumulates
    // rather than failing on an already-initialized account.
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let integrator = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS);
    let fee_ata = f.platform_fee_ata(&integrator.pubkey(), 0);

    let taker = f.funded_depositor(0, 400_000);
    // Distinct amounts so the two transactions aren't byte-identical (litesvm
    // dedups a replayed one as `AlreadyProcessed`), which also makes the
    // accumulation assertion below a real sum of two different fees.
    let swap_once = |f: &mut Fixture, amount_in: u64| {
        f.swap_with_fee_meta(
            &taker,
            0,
            amount_in,
            Price::INFINITY.as_u32(),
            1,
            &integrator.pubkey(),
            CEILING_BPS,
        )
        .expect("fee-bearing swap");
    };

    swap_once(&mut f, 100_000);
    let after_first = f.token_balance(&fee_ata);
    assert!(after_first > 0);

    swap_once(&mut f, 90_000);
    assert!(
        f.token_balance(&fee_ata) > after_first,
        "the second fee accumulated into the existing ATA"
    );

    // Two taker-bound Buys, so two quote atoms of exact-in residue — the
    // bucket accumulates across swaps rather than being swept per fill.
    f.assert_treasury_residual(0, 2);
}

#[test]
fn platform_fee_multi_leg_fill_stays_inside_the_cu_budget() {
    // The fee adds two CPIs (an ATA `create_idempotent` and a second
    // `transfer_checked`) on top of the per-leg fill loop, plus four accounts
    // and one event. The worst case for both is a multi-leg fill that also
    // creates the destination ATA, so measure exactly that and pin it under
    // the default per-instruction budget.
    //
    // The 200k figure is the Solana default a caller gets without a
    // `SetComputeUnitLimit`, so staying under it is what lets a plain
    // frontend swap work with no budget instruction at all — the property
    // worth a regression test rather than the precise CU count, which moves
    // with every matcher change.
    const DEFAULT_CU_BUDGET: u64 = 200_000;

    let free_cu = {
        let mut f = Fixture::seeded_two_ask_levels();
        let taker = f.funded_depositor(0, 5_000_000);
        f.swap_meta(&taker, 0, 2_000_000, Price::INFINITY.as_u32(), 1)
            .expect("no-fee multi-leg swap")
            .compute_units_consumed
    };

    let mut f = Fixture::seeded_two_ask_levels();
    let taker = f.funded_depositor(0, 5_000_000);
    let integrator = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS);
    let fee_ata = f.platform_fee_ata(&integrator.pubkey(), 0);
    let meta = f
        .swap_with_fee_meta(
            &taker,
            0,
            2_000_000,
            Price::INFINITY.as_u32(),
            1,
            &integrator.pubkey(),
            CEILING_BPS,
        )
        .expect("fee-bearing multi-leg swap");
    let paid_cu = meta.compute_units_consumed;

    // Both legs really filled and the ATA really got created in this run —
    // otherwise the measurement isn't the worst case it claims to be.
    assert_eq!(
        common::events::fills(&meta).len(),
        2,
        "the measured swap must actually be a two-leg fill"
    );
    assert!(
        f.maybe_token_balance(&fee_ata).is_some_and(|b| b > 0),
        "the measured swap must actually have created and funded the fee ATA"
    );

    println!(
        "multi-leg swap CU: no fee {free_cu}, with platform fee {paid_cu} \
         (+{} for the ATA create + fee transfer + event)",
        paid_cu.saturating_sub(free_cu)
    );
    assert!(
        paid_cu < DEFAULT_CU_BUDGET,
        "a fee-bearing multi-leg swap must fit the default {DEFAULT_CU_BUDGET} CU \
         budget without a SetComputeUnitLimit, used {paid_cu}"
    );
    // Bound what the fee itself adds, so the no-fee arm is a real control
    // rather than decoration for the `println!`. This is the figure that
    // regresses if a future change makes the fee path materially heavier —
    // the total above could stay under budget while the delta doubled. The
    // ceiling is loose on purpose: the dominant term is the one-time ATA
    // creation (~20k), and pinning it tightly would fail on any upstream
    // SPL/ATA cost change without indicating a problem here.
    const MAX_FEE_OVERHEAD_CU: u64 = 45_000;
    let overhead = paid_cu.saturating_sub(free_cu);
    assert!(
        overhead < MAX_FEE_OVERHEAD_CU,
        "the platform fee added {overhead} CU over the identical no-fee swap, \
         above the {MAX_FEE_OVERHEAD_CU} ceiling — the ATA create, the second \
         transfer, and the event should not cost this much"
    );
}

#[test]
fn platform_fee_rejects_a_non_canonical_destination() {
    // The fee ATA carries no Anchor constraints — `create_idempotent`'s own
    // derivation is what pins the destination. Prove that guard is live by
    // aiming the fee at the market's base treasury, which is a real,
    // initialized token account on the right mint but is *not* the
    // integrator's ATA. This is the case `unsafe(dup)` on that field would
    // otherwise have left unguarded.
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let taker = f.funded_depositor(0, 200_000);
    let integrator = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS);

    let mut ix = f.swap_ix_with_fee(
        &taker.pubkey(),
        0,
        100_000,
        Price::INFINITY.as_u32(),
        1,
        Some(&integrator.pubkey()),
        CEILING_BPS,
    );
    // Swap the derived fee ATA (meta index 12) for the base treasury.
    let treasury = f.base_treasury;
    ix.accounts[12].pubkey = treasury;

    let treasury_before = f.token_balance(&treasury);
    f.send_ix(&taker, ix)
        .expect_err("the ATA program must reject a non-canonical fee account");
    assert_eq!(
        f.token_balance(&treasury),
        treasury_before,
        "the rejected swap moved nothing"
    );
    assert_eq!(f.vault(0).base_atoms.get(), SEED_BASE, "vault untouched");
}
