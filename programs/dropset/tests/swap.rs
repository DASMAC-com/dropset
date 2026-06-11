//! Swap integration tests — the multi-vault heap matcher and `min_out`
//! soft-revert, end-to-end against the deployed `.so`. All built on the
//! shared [`Fixture`]: `Fixture::seeded` for the single-vault cases and
//! `seeded_two_vaults` for cross-vault price-time priority.

mod common;

use anchor_v2_testing::Signer;
use common::fixture::{simple_profile, Fixture, PROFILE_BYTES};
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

    // Treasury invariant holds.
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
fn taker_fee_retained_in_vault() {
    // Same swap with and without a taker fee — the fee'd taker receives
    // strictly less base, and the difference stays in the vault.
    let run = |fee_ppm: u16| {
        let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
        f.poke_taker_fee(fee_ppm);
        let taker = f.funded_depositor(0, 200_000);
        f.swap(&taker, 0, 100_000, Price::INFINITY.as_u32(), 1)
            .expect("swap");
        (
            f.token_balance(&f.base_ata(&taker.pubkey())),
            f.vault(0).base_atoms.get(),
        )
    };
    let (no_fee_recv, _) = run(0);
    let (fee_recv, fee_vault_base) = run(10_000); // 1%

    assert!(
        fee_recv < no_fee_recv,
        "a positive taker fee leaves the taker with less base ({fee_recv} vs {no_fee_recv})"
    );
    // Without a fee the vault sent out `no_fee_recv` base; with the fee
    // it retains the fee slice, so its remaining base is higher.
    assert!(
        fee_vault_base > SEED_BASE - no_fee_recv,
        "vault retained the taker fee"
    );
}

// ── Multi-vault price-time priority + flush / expiry ─────────────────

/// Build a two-vault market (sectors 0 then 1), each seeded with
/// 1_000_000 base / 1_085_000 quote and a full-inventory ±0.5% ladder,
/// but anchored at the given reference prices. Sector 0 is set up
/// first, so its `reference_price.stamp` nonce is strictly older than
/// sector 1's — the price-time tiebreaker when prices are equal.
fn seeded_two_vaults(ref0_bits: u32, ref1_bits: u32) -> Fixture {
    let mut f = Fixture::bootstrap();
    let auth = f.authority.insecure_clone();

    // Sector 0.
    f.register_vault(0, f.authority.pubkey(), false, Pubkey::default())
        .expect("register vault 0");
    f.set_reference_price(&auth, 0, ref0_bits, 0)
        .expect("ref 0");
    f.set_liquidity_profile(&auth, 0, simple_profile(5_000, 10_000, u32::MAX))
        .expect("profile 0");
    f.deposit_leader(0, 1_000_000, 1_085_000, 1_000_000, 1_085_000)
        .expect("seed 0");

    // Advance the blockhash so sector 1's seed mint (same amount, same
    // ATA as sector 0's) isn't a byte-identical, already-processed txn.
    f.svm.expire_blockhash();

    // Sector 1. Its register / seed transactions mirror sector 0's
    // argument-for-argument; the blockhash bump above is what keeps
    // them from colliding as already-processed duplicates.
    f.register_vault(1, f.authority.pubkey(), false, Pubkey::default())
        .expect("register vault 1");
    f.set_reference_price(&auth, 1, ref1_bits, 0)
        .expect("ref 1");
    f.set_liquidity_profile(&auth, 1, simple_profile(5_000, 10_000, u32::MAX))
        .expect("profile 1");
    f.deposit_leader(1, 1_000_000, 1_085_000, 1_000_000, 1_085_000)
        .expect("seed 1");
    f
}

#[test]
fn multi_vault_cheaper_price_fills_first() {
    // Sector 1 quotes the lower reference (cheaper asks for a Buy), so
    // a small Buy must fill entirely against sector 1 and leave the
    // pricier sector 0 untouched.
    let hi = Price::encode(10_900_000, 0).unwrap().as_u32();
    let lo = Price::encode(10_800_000, 0).unwrap().as_u32();
    let mut f = seeded_two_vaults(hi, lo);

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
    let mut f = seeded_two_vaults(same, same);
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
    let mut f = seeded_two_vaults(hi, lo);

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
    // Re-profile the seeded vault with a 1-slot expiry, warp well past
    // it, then Buy: every level has expired, so nothing fills.
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    f.set_liquidity_profile(
        &f.authority.insecure_clone(),
        0,
        simple_profile(5_000, 10_000, 1),
    )
    .expect("short-expiry profile");
    f.svm.warp_to_slot(100);

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

/// Two ask levels, 5_000 bps each (Σ = 10_000), at different offsets.
fn two_ask_level_profile() -> [u8; PROFILE_BYTES] {
    let mut p: dropset::LiquidityProfile = anchor_lang_v2::bytemuck::Zeroable::zeroed();
    p.asks[0].price_offset = 5_000u32.into();
    p.asks[0].size_bps = 5_000u16.into();
    p.asks[0].expiry_offset = u32::MAX.into();
    p.asks[1].price_offset = 10_000u32.into();
    p.asks[1].size_bps = 5_000u16.into();
    p.asks[1].expiry_offset = u32::MAX.into();
    let mut bytes = [0u8; PROFILE_BYTES];
    bytes.copy_from_slice(anchor_lang_v2::bytemuck::bytes_of(&p));
    bytes
}

#[test]
fn min_out_soft_revert_restores_multiple_legs_and_rearms_flush() {
    // A vault with two ask levels. A Buy that crosses both, then fails
    // its `min_out`, must restore *both* levels' remaining size and
    // re-arm FLUSH_BIT.
    let mut f = Fixture::bootstrap();
    let auth = f.authority.insecure_clone();
    f.register_vault(0, f.authority.pubkey(), false, Pubkey::default())
        .expect("register");
    f.set_reference_price(&auth, 0, Price::encode(10_850_000, 0).unwrap().as_u32(), 0)
        .expect("ref");
    f.set_liquidity_profile(&auth, 0, two_ask_level_profile())
        .expect("two-level profile");
    f.deposit_leader(0, 1_000_000, 1_085_000, 1_000_000, 1_085_000)
        .expect("seed");

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
