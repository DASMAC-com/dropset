# Ephemeral Central Limit Order Book (eCLOB) Architecture

This sketch presents an ephemeral central limit order book (eCLOB) design that
transparently aggregates liquidity from multiple market makers into a
single Solana account. The eCLOB provides a familiar order book API for makers,
transparency for takers and aggregators, and price update costs as low as a
propAMM. It synthesizes the benefits of two major existing designs, while
eliminating their respective drawbacks:

**Legacy CLOBs** offer a consolidated, transparent book that takers and
aggregators can query in one place, but the entire book must be kept
fully-sorted in memory onchain, typically via binary search trees, and
re-sorted on every maker quote update. Maintenance costs fall on makers
regardless of whether a taker ever trades against their re-shuffled liquidity,
making frequent re-quoting prohibitively expensive for active strategies.

**propAMMs** sit at the opposite extreme: a lazy-loading design where each maker
rapidly updates a single reference price in an isolated account. Quoting is
cheap, but liquidity is fragmented across opaque venues where a fill can
silently execute at a price different from what was quoted. Without a shared
book and common data model, takers and aggregators face difficulties detecting
such discrepancies or routing around them.

The eCLOB design collapses both tradeoffs. Because every maker quotes into
the same visible book, takers and aggregators hit a single account and compare
all competing prices at once; worst-case slippage is bounded by the next-best
visible level rather than whatever price one isolated venue chose to show.

The key innovation is **just-in-time order book reconstruction** (detailed
below): rather than maintaining a persistent sorted structure onchain, each
taker builds an ephemeral book on the SVM program heap for the duration of
their instruction, then discards it. Book-maintenance cost shifts onto
takers — makers never pay to keep a shared sorted structure coherent.

This design enables the same lazy-loading approach to price updates that
propAMMs use, made possible here by segmenting the market-maker set into
a bounded pool of per-leader vaults. N leaders share one market account,
and each hot-path price update is just a few aligned memory stores, enabling
propAMM-cadence reference-price refresh through a familiar CLOB-style API,
but without propAMM opacity or engineering burden.

Each vault is operated by a single **leader** (the pubkey that paid
the market's open-vault fee to call `OpenVault`). Outside depositors back
that leader's quotes with paired (base, quote) baskets and share in
spread capture, with a skin-in-the-game floor and per-share
high-water-mark performance fee aligning incentives. See **Vault**
below for details.

## Conventions

**Ppm (parts per million)** is the unit for all sub-basis-point rates
in this spec: 1 ppm = 10⁻⁶ = 0.0001 bps; 1 bps = 100 ppm;
1% = 10,000 ppm; 100% = 1,000,000 ppm.
Two integer widths appear:

- `Ppm16` — `u16`, max 65,535 ppm ≈ 6.55%. Used where a tight cap is
  intentional (e.g. taker fee rate).
- `Ppm32` — `u32`, max ~4.29 billion ppm ≈ 4,294%. Used where a wider
  range is needed (e.g. price offsets).

**Basis points (bps)** apply to coarser rates where ppm granularity
is overkill: `size_bps` (per-level fraction of inventory).
Convention: 10000 = 100%.

## Registry

The `Registry` is a global singleton account that holds protocol-wide
governance parameters and the admin allowlist.

Vault creation is **permissionless**: any pubkey may call `OpenVault`
by paying the **market's** open-vault fee — `market.fee_config.atoms`
of `market.fee_config.mint` — to the Registry's fee ATA, keyed on
`get_associated_token_address_with_program_id` over
`(registry_pda, fee_config.mint, token_program)`. The token-program
seed is mandatory
(classic SPL Token and Token-2022 derive different ATAs for the same
mint) and is taken from the **fee mint's account `owner`** — the
caller passes the mint and its owning token program, validated
`token_program == fee_config.mint.owner` at `OpenVault`. No storage is
needed on the Registry itself. The fee
is **per market** — each `MarketHeader` carries its own `fee_config`,
seeded from `Registry.default_fee_config` at market creation and
tuned per market by an admin via `SetMarketFeeConfig`. Admins may
call `OpenVault` without paying, including on behalf of others (useful
for protocol-onboarded market makers). If a market's `fee_config.mint`
later changes, a fresh registry ATA is used going forward and prior
fees stay in the old ATA; admins sweep both.

The per-market cap on vault count (`max_vaults_per_market`) is set by
the cost to reconstruct the ephemeral order book during each take
and can be tuned across the protocol's lifecycle as CU budgets and
runtime performance evolve.

```rust
struct FeeConfig {
    /// Mint accepted for this fee.
    mint: Pubkey,
    /// Amount in atoms of `mint`.
    atoms: u64,
}

struct Registry {
    /// Hard cap on how many vaults any one market may allocate
    /// (up to 255). Enforced at `OpenVault` time.
    max_vaults_per_market: u8,
    /// Taker fee rate stamped into `MarketHeader.taker_fee`
    /// at market creation. Admins may change a market's fee
    /// later; this field only sets the initial value.
    default_taker_fee: Ppm16,
    /// Minimum fraction of vault shares the leader must hold,
    /// in ppm (1,000,000 = 100%). Stamped into
    /// `MarketHeader.default_min_leader_share` at market creation,
    /// which in turn stamps each vault's `min_leader_share` at
    /// `OpenVault`. This field only sets the protocol-wide initial
    /// value; admins tune the floor per market and per vault
    /// downstream. Default 50_000 = 5%.
    /// See **Vault → Skin-in-the-game floor**.
    default_min_leader_share: Ppm32,
    /// Default `FeeConfig` for the per-`OpenVault` fee, **stamped into
    /// `MarketHeader.fee_config` at market creation**. This field only
    /// seeds the initial per-market value; admins tune each market's
    /// open-vault fee downstream via `SetMarketFeeConfig`. The fee is
    /// paid to the **Registry fee ATA** for the configured mint
    /// (`get_associated_token_address_with_program_id(registry_pda,
    /// fee_config.mint, token_program)`, the token program taken from
    /// the mint's account owner; see the **Registry** overview above),
    /// waived when the signer is an admin. (This fee
    /// account is distinct from a market's
    /// `base_treasury`/`quote_treasury`, which custody pooled trading
    /// inventory, not protocol fees.)
    default_fee_config: FeeConfig,
    /// Admins authorized to mutate the `Registry`, change a market's
    /// `taker_fee`, `default_min_leader_share`, and `fee_config`
    /// (the last via `SetMarketFeeConfig`), override a vault's
    /// `min_leader_share` via `SetMinLeaderShare`, call `FreezeVault`,
    /// approve outside deposits via `SetOutsideDepositsApproved`, and
    /// open vaults without paying the per-market open-vault fee.
    admins: Set<Pubkey>,
}
```

Notably absent: there is **no leader allowlist**. Banning a pubkey
would be trivially defeated by registering a fresh wallet, so the
protocol does not maintain one. Admin power is exercised per-vault via
`FreezeVault` (see **Leader operations**), and the non-refundable
per-market open-vault fee (`MarketHeader.fee_config`) acts as the only
material gate on fresh entry: every new wallet pays the fee again, so
spinning up replacements after a freeze has a real, repeated cost
rather than being free.

## MarketHeader

The `MarketHeader` is a fixed-size record at the front of the market
account. It holds the market-wide counters and the active set of
vaults; the physical sector array sits immediately after the header
(see **Storage layout**).

```rust
struct MarketHeader {
    /// Market-wide monotonic counter. Stamped onto the vault on
    /// every `SetReferencePrice` and `SetLiquidityProfile`; also advanced
    /// on every taker fill. A `u64`, wide enough to never wrap over
    /// the market's lifetime.
    nonce: u64,
    /// Active vaults on this market — bounded by
    /// `registry.max_vaults_per_market`.
    vaults: Set<Vault>,
    /// Taker fee rate, capped at ~6.55% (`Ppm16` max). Mutable by an
    /// admin.
    taker_fee: Ppm16,
    /// Default minimum fraction of vault shares the leader must hold,
    /// in ppm (1,000,000 = 100%). Stamped into `Vault.min_leader_share`
    /// at each `OpenVault` on this market. Seeded from
    /// `Registry.default_min_leader_share` at market creation; mutable
    /// by an admin, affecting only vaults opened after the change
    /// (existing vaults keep their stamped value). See
    /// **Vault → Skin-in-the-game floor**.
    default_min_leader_share: Ppm32,
    /// This market's per-`OpenVault` fee: mint and amount (see
    /// `FeeConfig`). Seeded from
    /// `Registry.default_fee_config` at market creation; mutable by an
    /// admin via `SetMarketFeeConfig`. The fee is paid to the
    /// **Registry fee ATA** for `fee_config.mint`
    /// (`get_associated_token_address_with_program_id(registry_pda,
    /// fee_config.mint, token_program)`, the token program taken from
    /// the mint's account owner), not to this market's treasuries, and
    /// is waived for admin signers. Changing
    /// the mint routes future fees to a fresh registry ATA — admins
    /// sweep both old and new.
    fee_config: FeeConfig,

    // Pubkeys and bumps.
    base_mint: Pubkey,
    quote_mint: Pubkey,
    /// SPL token account holding the **pooled** base inventory
    /// for every vault on this market. Authority is a PDA derived
    /// from the market account (`base_treasury_bump`).
    /// **Invariant:** `base_treasury.amount ==
    /// Σ vault.base_atoms` across every vault on the market
    /// (active and tombstoned). Each `Deposit`, `Withdraw`, and
    /// fill moves atoms between this treasury and the caller's
    /// ATA while adjusting the matching vault's `base_atoms` by
    /// the same delta — the two must stay aligned per instruction.
    /// The treasury is the SPL **custody account**; its `.amount`
    /// is the market's *reserves* quantity. Note this sums active
    /// **and tombstoned** vaults, so it is total inventory held
    /// in custody, not matchable liquidity.
    base_treasury: Pubkey,
    /// Same as `base_treasury`, for the quote leg.
    /// **Invariant:** `quote_treasury.amount ==
    /// Σ vault.quote_atoms`.
    quote_treasury: Pubkey,
    bump: u8,
    base_treasury_bump: u8,
    quote_treasury_bump: u8,
}
```

Every quote a vault produces is identified by `MarketHeader.nonce`
at the moment of stamping — a global counter incremented on every
`SetReferencePrice`, `SetLiquidityProfile`, and taker fill. At match time,
levels at the same price are ranked by nonce: lower nonce = earlier
arrival = wins. This is the canonical CLOB **price-time priority**
rule, with the nonce standing in for "time" — slot timestamps would
be too coarse, since multiple events can land in the same slot.

## Storage layout

Physically, the market account is a single contiguous slab grown by
`realloc`: the `MarketHeader` followed by a fixed-size sector array,
with three threaded lists tracking active, tombstoned, and free
sectors.

```txt
+----------------+----------+----------+----------+----------+-----+
| MarketHeader   | Sector 0 | Sector 1 | Sector 2 | Sector 3 | ... |
+----------------+----------+----------+----------+----------+-----+
```

Each `Vault` carries two opaque pointer fields (`next`, `prev`) not
shown in its struct (see **Vault**) — they thread whichever list the
vault is currently on (active, tombstone, or free). `MarketHeader`
separately stores three list heads (`head`, `tombstone_head`,
`free_head`). These are stored as offsets into the sector region
(durable across transactions); each instruction resolves them to
current-tx pointers against the account's input-buffer base.

Example state after opening Vaults 0–4, calling `CloseVault` on
Vault 2 (which still has outstanding shares), and fully draining
Vault 1 (`total_shares` reached 0):

```txt
  MarketHeader
  +------------------+
  | head           --+---> Vault 4 <-> Vault 3 <-> Vault 0 -> null
  | tombstone_head -+---> Vault 2 -> null
  | free_head      --+---> Vault 1 -> null
  +------------------+
```

New vaults are prepended at `head` (so the most recent open sits at
the front). `tombstone_head` points at vaults that have been
`CloseVault`'d but still hold outstanding shares — depositors can
continue to `Withdraw`, but the matching engine does not iterate
this list. `free_head` points at fully reclaimed sectors; the free
list is singly linked via `next` and ignores `prev`. All three
lists are mutated only on vault open / close / reclaim — the hot
path (`SetReferencePrice`) never touches list pointers.

`Set<Vault>` operations map onto this layout as follows:

- **Iterate active vaults** (taker hot path) → walk the DLL from
  `head`. Tombstones are not visited.
- **Insert (`OpenVault`)** → pop the free list if non-empty, else
  `realloc` by `size_of::<Vault>()`; prepend at `head`.
- **Tombstone (`CloseVault`)** → unlink from active DLL, prepend
  at `tombstone_head`. The vault keeps its data; only the list
  membership changes.
- **Reclaim (`Withdraw` that drives `total_shares` to 0 on any
  non-free vault)** → unlink from whichever DLL the vault is on
  (active for a drained frozen vault, tombstone for a closed
  vault), zero `vault.leader` and `vault.quote_authority` so the
  emptiness marker holds, push onto free list.

Market creation only pays rent for the header.

## Vault

A **vault** holds a leader's pooled inventory (their own inventory plus
outside depositor contributions), their `LiquidityProfile` (bids and asks
as offsets from a single reference price), and a `ReferencePrice`
they update on the hot path. Vaults live contiguously inside the
market account's sector array (see **Storage layout**). The leader
(or a delegated `quote_authority`) is the only signer that can mutate
quotes — both the `ReferencePrice` and the `LiquidityProfile`;
both the leader's stake and outside-depositor shares are non-SPL
bookkeeping (see **Shares**) — outside positions live on separate
`VaultDepositor` PDAs, so neither imposes any per-depositor storage
on the vault sector itself.

Leader-supplied prices are **not** validated on write — takers
range-check at match time, so a nonsense reference price just
renders that vault unmatchable.

```rust
struct Vault {
    leader: Pubkey,
    /// Authority for quote-mutating ix (`SetReferencePrice`,
    /// `SetLiquidityProfile`). Always populated — at `OpenVault` time
    /// the caller may pass `Some(pubkey)`; if `None`, the protocol
    /// stamps `leader`. Hot-path auth check is a single compare.
    /// Rotated via `SetQuoteAuthority` (leader-only).
    quote_authority: Pubkey,
    /// Packed `(stamp, price, expiry)`. Hot path —
    /// overwritten as two aligned u64 stores.
    reference_price: ReferencePrice,
    /// Base tokens (atoms) backing this vault's asks. Pooled
    /// inventory across the leader and outside depositors. Physical
    /// balance lives in the market-wide `base_treasury`; see
    /// treasury invariant in **MarketHeader**.
    base_atoms: u64,
    /// Quote tokens (atoms) backing this vault's bids. Pooled
    /// inventory across the leader and outside depositors. Physical
    /// balance lives in the market-wide `quote_treasury`; see
    /// treasury invariant in **MarketHeader**.
    quote_atoms: u64,
    /// Total vault shares outstanding (= leader_shares +
    /// Σ VaultDepositor.shares).
    total_shares: u64,
    /// Leader's stake. Non-SPL, protocol-tracked. Increments on
    /// leader `Deposit` and on `Realize` perf-fee accrual;
    /// decrements on leader `Withdraw`. See
    /// **Skin-in-the-game floor**.
    leader_shares: u64,
    /// High-water mark of value-per-share (`L / total_shares`),
    /// stored as Q32.32 fixed-point `u64` — VPS up to ~4.29×10⁹×
    /// the seed value is representable (practically unreachable
    /// in normal operation). Never decreases — performance fee
    /// accrues only when VPS exceeds this mark.
    hwm: u64,
    /// Performance fee rate the leader charges on profits above
    /// HWM, in ppm (1,000,000 = 100%). Set at `OpenVault` time.
    perf_fee_rate: Ppm32,
    /// Minimum fraction of vault shares the leader must hold, in ppm
    /// (1,000,000 = 100%). The value actually enforced at `Deposit`
    /// and leader `Withdraw` against this active vault. Stamped from
    /// `MarketHeader.default_min_leader_share` at `OpenVault`; an
    /// admin can override it per vault via `SetMinLeaderShare` (e.g.
    /// to seat an issuer-funded vault at a lower floor than the
    /// market default). See **Skin-in-the-game floor**.
    min_leader_share: Ppm32,
    /// Set to 1 when an admin freezes the vault. See
    /// **Frozen and tombstoned vaults**.
    frozen: u8,
    /// Set to 1 if outside depositors are permitted. When 0, only
    /// the leader may `Deposit`; existing outside depositors (from
    /// before the flag was flipped) can still `Withdraw`. Mutable
    /// by the leader via `SetAllowOutsideDepositors`.
    allow_outside_depositors: u8,
    /// Set to 1 when an admin has approved this vault to take
    /// outside deposits. An outside `Deposit` requires **both**
    /// this flag and `allow_outside_depositors` to be 1: the leader
    /// opts in, and an admin must independently sign off. Default 0
    /// at `OpenVault` — a fresh vault cannot take outside baskets
    /// until an admin approves it. Existing outside depositors (from
    /// before approval was revoked) can still `Withdraw`. Mutable by
    /// an admin via `SetOutsideDepositsApproved`.
    outside_deposits_approved: u8,
    /// Bids and asks as `(price_offset, size_bps, expiry_offset)`
    /// per level: ppm offset from `reference_price.price`, bps
    /// fraction of inventory, and slot offset after `quote_slot`.
    /// See **LiquidityProfile**.
    profile: LiquidityProfile,
    /// Materialized per-level state — absolute price, atom-sized
    /// allowance, and absolute expiry — computed from `profile`
    /// and the current inventory by the first taker to match this
    /// vault after a `SetReferencePrice` or `SetLiquidityProfile` (see
    /// `reference_price.stamp`). Subsequent takers read these
    /// values directly and decrement `size` on fills.
    remaining: Remaining,
}

struct ReferencePrice {
    /// `market.nonce` stamped at the last `SetReferencePrice` or
    /// `SetLiquidityProfile`, OR'd with `FLUSH_BIT` (`1 << 63`) as a
    /// "flush pending" flag. `FLUSH_BIT` is armed by both
    /// `SetReferencePrice` and `SetLiquidityProfile`; the first taker
    /// to match this vault materializes `LiquidityProfile` and inventory
    /// into `Vault.remaining` and clears the flag. Low 63 bits
    /// hold the nonce; takers mask off `FLUSH_BIT` before comparing
    /// for price-time priority. The 63-bit field is wide enough to
    /// never wrap over the market's lifetime (~10¹¹ years at one
    /// event per slot).
    stamp: u64,
    /// Reference price the leader's book profile is anchored to.
    /// A `Price` (see **Price**); range-checked by the taker at
    /// match time.
    price: Price,
    /// Slot the quote was "as of" — supplied by the leader,
    /// validated `<= current_slot` (no future-dating) and
    /// `>= current_slot − MAX_BACKDATE` (default 50 slots).
    /// Per-level effective expiry is
    /// `quote_slot + level.expiry_offset`.
    quote_slot: u32,
}

struct Remaining {
    bids: [Position; N_LEVELS],
    asks: [Position; N_LEVELS],
}

struct Position {
    /// Absolute price for this level. Materialized at flush; see
    /// **LiquidityProfile → Flush** for the formula.
    price: Price,
    /// Live allowance in atoms (base for asks, quote for bids).
    /// Materialized at flush from the level's `size_bps` against
    /// the matching inventory leg; decremented on fills.
    size: u64,
    /// Absolute slot this level expires at. Materialized at flush
    /// from `ref_price.quote_slot + level.expiry_offset`. Takers
    /// skip levels where `current_slot >= expires_at`.
    expires_at: u32,
}
```

### Price

`Price` is a `u32` decimal floating-point key. The high 5 bits hold a
base-10 exponent biased by 16 (unbiased range `-16..=15`); the low 27
bits hold a significand normalized to exactly 8 significant digits
(`10_000_000..=99_999_999`). The significand is a mantissa scaled by
`10^7`, so the value is `(significand / 10^7) × 10^exponent` —
equivalently `significand × 10^(exponent − 7)` — placing the mantissa
in `[1.0000000, 9.9999999]`. The price spans ~`1e-16` to
~`9.9999999e15` at 8 significant figures.

```text
 [ 5-bit biased exponent ][ 27-bit normalized significand ]
   bits 31..27               bits 26..0
```

Two reserved encodings double as taker bounds: `0x0000_0000` is zero
(a market sell with no minimum fill price) and `0xFFFF_FFFF` is
infinity (a market buy with no maximum). Every other bit pattern is a
regular price.

**Integer order is price order.** With the exponent in the high bits
and the significand normalized to a fixed width, an unsigned `u32`
compare of two `Price`s matches comparing the values they encode. The
matching engine leans on this: price-time priority — the
`(price, nonce)` heap keys, including the bid-side `Price::MAX − price`
inversion — is a raw integer compare with no decode. Normalization
also makes the encoding canonical (one bit pattern per representable
price), so equality and tie-breaks are unambiguous.

**Built for ordering, not multiplication.** The significand/exponent
form is never multiplied directly. Fills move integer atom counts
(`base_atoms`, `quote_atoms`), and any price arithmetic first decodes
`Price` to a scaled value. Base-10 exponents keep FX prices like
`1.0850` exact — no binary-fraction rounding — which matters for tick
alignment and for the cost-basis math in **Depositor positions and
cost basis**.

The 32-bit width is load-bearing: it lets `(price, quote_slot)` pack
into one `u64` on the `SetReferencePrice` hot path and keeps every
materialized `Position.price` compact.

### Value-per-share and the L measure

Vault value is tracked via a dimensionless metric borrowed from
constant-product AMMs:

```text
L = isqrt(base_atoms × quote_atoms)
```

**L is a measure, not a curve constraint.** The matching engine does
not constrain trades to preserve any invariant — leaders quote
freely, and `L` is just a function of the post-trade inventory used
for share accounting and perf-fee calculation.

Three properties make this the right metric for an actively-quoted
two-asset vault:

- **No oracle, no external unit of account.** L lives in units of
  √(base × quote); it is only ever compared against itself at the
  same vault.
- **Deposits and withdrawals at the current ratio leave `L / total_shares`
  invariant.** Both legs scale proportionally, so value-per-share
  (VPS) does not tick on basket flows.
- **L tracks performance against a passive constant-product hold.**
  For a sell of `dx` base at price P, the exact post-trade identity
  is `new L² = old L² + dx·(b·P − q) − dx²·P`. The linear term is
  what reads as "spread captured" vs "adversely selected"; the
  quadratic correction is negligible for `dx ≪ b` and only matters
  near full-leg drain. L grows when the leader sells above the
  AMM-implied price `q/b` and shrinks when they sell below.
  Directional moves of the underlying pair with no fills leave
  inventory and L unchanged — directional exposure flows through to
  depositors via the basket, not through VPS.

Unlike a Uniswap-v2 LP share — where L grows monotonically because
fees stay in the pool — **L in Dropset can shrink**. A leader who
quotes badly is observably losing value-per-share. This is why HWM
does real work in the next section: it prevents perf fee from
accruing on the way back up from drawdowns.

### Share-accounting invariants

Let `b = base_atoms`, `q = quote_atoms`, `s = total_shares`,
`L = isqrt(b · q)`, `VPS = L / s`.

**I1. Basket flows preserve VPS.** `Deposit` and `Withdraw` at the
current ratio scale `b` and `q` by the same factor `(s ± Δs)/s`;
hence `L' = L · (s ± Δs)/s` and `VPS' = L'/(s ± Δs) = L/s = VPS`.
Existing holders are neither diluted nor accreted. ∎

**I2. Fills move L, not s.** A taker fill changes `b` and `q` per
the trade but never mints or burns shares. So `VPS' ≠ VPS` iff
`L' ≠ L`, and the sign follows the slippage condition: `L` grows
iff the trade price `P` satisfies `P > q/(b − dx)` on a sell of
`dx` base (and the symmetric condition on bids). ∎

**I3. `Realize` moves s, not L.** Perf-fee accrual adds `m` shares
to both `leader_shares` and `total_shares` without touching `b` or
`q`; `L` is unchanged. `VPS` drops from `L/s` to `L/(s + m)`, and
`hwm := L/(s + m)`. ∎

**I4. `leader_shares` only grows from below.** Three paths mutate
it: `Deposit` (leader path) and `Realize` (permissionless) add;
`Withdraw` (leader path) subtracts under leader signature. No path
decreases `leader_shares` as a side-effect of `L` moving —
drawdowns lower VPS but not the share count. ∎

**I5. `hwm` is monotonic.** Initialized to `Q32.32(1.0)` by seeding
(see Deposit's seeding branch); thereafter set only by `Realize`,
and only when `VPS_new > hwm`. Recoveries to a prior VPS do not
earn perf fee. ∎

**I6. Invariant on total shares.**
`total_shares = leader_shares + Σ VaultDepositor.shares` at all
times. Every path that mutates `total_shares` mutates exactly one of
the two terms by the same amount (the outside paths touch a single
`VaultDepositor.shares`):

| Operation                                  | `leader_shares` | `Σ VaultDepositor.shares` |
| ------------------------------------------ | --------------- | ------------------------- |
| `Deposit` (seeding; `s = 0` → leader path) | +Δs             | 0                         |
| `Deposit` (leader path)                    | +Δs             | 0                         |
| `Deposit` (outside path)                   | 0               | +Δs                       |
| `Withdraw` (leader path)                   | −Δs             | 0                         |
| `Withdraw` (outside path)                  | 0               | −Δs                       |
| `Realize`                                  | +m              | 0                         |

### High-water mark and performance fee

Prior losses must be fully recovered — VPS back above `Vault.hwm` —
before the leader earns again.

Performance fee accrues as **newly-minted shares** into
`Vault.leader_shares`, not as token withdrawals: no forced
liquidation, auto-compounding, no SPL mint touched (the leader's
stake is non-SPL). On `Realize`, if `VPS_new > hwm`:

- Existing depositors retain `(1 − f) × (VPS_new − hwm)` per share
  of the excess.
- The leader accrues `m` shares to `leader_shares`, capturing
  `f × (VPS_new − hwm)` per existing share, where:

```text
m = f × s × (L − hwm × s) / ((1 − f) × L + f × hwm × s)
```

`s` is `total_shares` before the mint; `f` is the vault's
`perf_fee_rate` (ppm / 1,000,000); `L` is the vault's current
value. After accrual, `total_shares` and `leader_shares` both grow
by `m`, and `hwm := L / (s + m)`.

### Realize

Applies the formula above: mints `m` new shares into
`Vault.leader_shares` (and `Vault.total_shares`) and updates `hwm`.

**Permissionless.** Anyone may call. The leader has the strongest
economic incentive; indexers and keepers may invoke it to pin HWM
at a known point in time. `Realize` runs implicitly at the start of
every `Deposit` and `Withdraw`, so outside flows always cross at a
post-fee VPS and never transfer leader-owed fee value to or from the
caller. **Never runs on the taker hot path.** Touches no SPL
accounts — perf fee accrual is purely on-vault bookkeeping.

**No-op on frozen and tombstoned vaults.** Once a vault leaves the
active eCLOB, HWM is pinned and no further perf fee accrues,
regardless of residual VPS movement from late fills. See
**Frozen and tombstoned vaults** for the lifecycle picture.

### Skin-in-the-game floor

Each vault's `Vault.min_leader_share` (ppm) is a hard floor on the
leader's stake in their own vault, enforced at the two natural choke
points. The value cascades: `Registry.default_min_leader_share`
(default 50_000 = 5%) seeds `MarketHeader.default_min_leader_share`
at market creation, which stamps `Vault.min_leader_share` at
`OpenVault`; an admin can override any level downstream — the market
default for future vaults, or a single vault directly via
`SetMinLeaderShare`. The choke points:

- **Deposit.** A `Deposit` is rejected if accepting it would push
  `leader_shares / total_shares` below `min_leader_share`.
- **Leader withdrawal.** A leader `Withdraw` against an active vault
  is rejected if it would push the ratio below `min_leader_share`.

Neither `SetReferencePrice` nor the taker hot path is touched. The
check uses on-vault numbers only (`leader_shares` and `total_shares`)
— no SPL mint or ATA load required, and the leader cannot evade the
floor *within a single vault* by transferring shares to an alt
wallet (their stake is non-SPL by construction). Cross-vault
collusion (the leader of vault A also acting as an outside depositor
on vault B and vice versa) is unconstrained by this check; the floor
only guarantees per-vault skin in the game.

The deposit gate creates a clean implicit cap on outside inventory:
once the vault reaches `leader_shares / min_leader_share`, new outside
deposits fail until the leader tops up. With a 5% floor, that caps
outside inventory at 19× the leader's stake.

The floor is **bypassed for leader withdrawals from frozen or
tombstoned vaults** — those vaults are winding down, and the leader
is treated as any other depositor on exit. See
**Frozen and tombstoned vaults**.

### Frozen and tombstoned vaults

A vault leaves the active eCLOB by either of two lifecycle paths:

| State          | Set by                          | DLL membership                                        | Quote ix                        | Deposit  | Withdraw                                             | Realize                           | Lifecycle exit                   |
| -------------- | ------------------------------- | ----------------------------------------------------- | ------------------------------- | -------- | ---------------------------------------------------- | --------------------------------- | -------------------------------- |
| **Active**     | default                         | active                                                | accepted                        | accepted | accepted                                             | accrues                           | becomes frozen or tombstoned     |
| **Frozen**     | admin via `FreezeVault`         | stays on active DLL; takers skip via per-level expiry | rejected (`!vault.frozen` gate) | rejected | accepted; `min_leader_share` bypassed for the leader | no-op (HWM pinned at freeze time) | Reclaim when `total_shares == 0` |
| **Tombstoned** | leader via `CloseVault`         | tombstone DLL; takers do not iterate                  | rejected (vault not visited)    | rejected | accepted; `min_leader_share` bypassed for the leader | no-op (HWM pinned at close time)  | Reclaim when `total_shares == 0` |
| **Reclaimed**  | implicit on draining `Withdraw` | free DLL                                              | n/a                             | n/a      | n/a                                                  | n/a                               | sector available for reuse       |

Both terminal states are designed so depositors can always exit and
no further fee accrues to the leader after exit. They differ in
*who* initiated and *how* matching is suppressed:

- **Frozen** — protocol revocation lever (admin-initiated). Vault
  stays on the active DLL; existing levels die off as their
  `expires_at` passes. Terminal — no "unfreeze".
- **Tombstoned** — leader's intended lifecycle exit. Vault is
  unlinked from active matching immediately.

Either state ends at **Reclaim** (see **Storage layout**): the
final `Withdraw` that drives `total_shares` to 0 unlinks the vault
from its current DLL, zeroes `vault.leader` and `vault.quote_authority`
so the emptiness marker holds, and pushes the sector onto the free
list. The same leader pubkey may then `OpenVault` afresh — paying
the open-vault fee again — on this or any other market.

## LiquidityProfile

Each level carries a `price_offset` in **ppm** (1_000_000 = 100%)
from `reference_price.price`, a `size_bps` as fraction of vault
inventory in **basis points** (10000 = 100%), and an `expiry_offset`
in slots after `quote_slot`. The two scales differ on purpose:
prices need sub-bp granularity (so `Ppm32`), sizes do not. Direction
is implicit from which array the level lives in: bids subtract the
price offset from the reference, asks add it.

Nothing in `LiquidityProfile` is in absolute atoms or absolute slots —
the materialization to atoms and absolute slots happens at flush
time (see below). This lets a leader reshape the ladder once via
`SetLiquidityProfile` and then leave it alone: as inventory drifts with
fills, subsequent flushes auto-rescale the level sizes to the
current `(base_atoms, quote_atoms)` without any further input from
the leader.

```rust
struct LiquidityProfile {
    /// Bid levels, top of book first.
    bids: [Level; N_LEVELS],
    /// Ask levels, top of book first.
    asks: [Level; N_LEVELS],
}

struct Level {
    /// Spread from `reference_price.price`. Direction is implicit:
    /// subtract for bids, add for asks. Materialized to an absolute
    /// price at flush.
    price_offset: Ppm32,
    /// Per-flush allowance as a fraction of the corresponding
    /// inventory leg, in basis points (10000 = 100%). The leg is
    /// `base_atoms` for asks and `quote_atoms` for bids, so a
    /// materialized bid size is denominated in **quote atoms** —
    /// the leader is allocating their quote pool across bid prices.
    /// **Invariant:** `Σ size_bps ≤ 10000` per side, enforced at
    /// `SetLiquidityProfile`. Setting the sum to exactly 10000
    /// fully commits that leg; lower sums leave a reserve.
    size_bps: u16,
    /// Per-level expiry in slots after `quote_slot`. Materialized
    /// to an absolute slot at flush. Takers skip levels where
    /// `current_slot >= expires_at`.
    expiry_offset: u32,
}
```

### Flush

When `SetReferencePrice` or `SetLiquidityProfile` arms the `FLUSH_BIT` on
`reference_price.stamp`, the next taker to hit this vault performs a
one-time materialization across all levels into `Vault.remaining`:

```text
// PPM = 1_000_000. Let a = asks[i], b = bids[i].

asks_remaining[i].size       = base_atoms × a.size_bps / 10000  // base
asks_remaining[i].price      = ref.price × (PPM + a.price_offset) / PPM
asks_remaining[i].expires_at = ref.quote_slot + a.expiry_offset

bids_remaining[i].size       = quote_atoms × b.size_bps / 10000  // quote
bids_remaining[i].price      = ref.price × (PPM −sat b.price_offset) / PPM
bids_remaining[i].expires_at = ref.quote_slot + b.expiry_offset
```

**`ref.price` is decoded here.** `Price` is a comparison key, not an
arithmetic type (see **Price**), so the offset math runs in decoded
space: decode `ref.price`, apply `(PPM ± offset) / PPM`, and store
`remaining[i].price` as the re-encoded absolute `Price`. The matching
fill loop likewise decodes a level's `Price` to a scaled ratio for the
atom arithmetic — a shift plus a base-10 scale, cacheable on the
ephemeral heap entry so a level is never decoded twice in one take.

Note the unit asymmetry: ask `size` is in **base atoms** (the maker
is offering base), bid `size` is in **quote atoms** (the maker is
offering quote). Off-chain renderers that want a base-equivalent bid
size for display compute `size / price`.

A u128 intermediate is used during multiplication to avoid u64
overflow (relevant for both the price and size computations); the
result is truncated back to the native field width. The `−sat`
operator on bids is saturating subtraction — bid `price_offset`
values ≥ 1_000_000 ppm produce a 0 bid price, which is range-checked
out at match time. The per-side `Σ size_bps ≤ 10000` invariant
(enforced at `SetLiquidityProfile`, see below) means the sum of
materialized sizes per side never exceeds the inventory leg, so no
runtime clamp is needed. `FLUSH_BIT` is then cleared with one
`u64` store.

Properties:

- **Per-flush allowance is preserved.** Once `size` decrements to
  zero at level `i`, that level is dead until the next flush —
  even if inventory remains. This caps per-flush drainage and
  prevents takers from chain-draining a stale top-of-book across
  successive instructions.
- **Inventory snapshot is automatic.** The leader doesn't manage
  absolute sizes; the percentages bind to whatever inventory exists
  at flush. After heavy buying drains base, the next flush
  automatically rescales the ladder to the new (smaller) base leg.
- **Per-level expiry stratifies the ladder.** A leader can give
  top-of-book a short `expiry_offset` (e.g., a few seconds in slots)
  and deep levels a much longer one, so flush cadence can be graded
  by depth instead of forced to the top-of-book rate.

## Shares

Shares are **never SPL tokens** — neither the leader's stake nor an
outside depositor's. The leader's stake is tracked as
`Vault.leader_shares`; every outside depositor's stake is tracked as
`shares` on a per-depositor **`VaultDepositor`** account (see
**Depositor positions and cost basis**). Both are pure on-vault
bookkeeping: nothing lives in an ATA, so the skin-in-the-game floor
cannot be evaded by moving shares to an alt wallet and no token
accounts are loaded at check time.

This makes a vault position **non-transferable and non-composable** —
a depositor exits by `Withdraw`, not by sending shares to someone
else (the Hyperliquid / Drift vault model). The trade is deliberate:
positions are not a tradeable secondary asset, but in exchange the
vault stores each depositor's cost basis authoritatively on-chain,
with no lot-attribution ambiguity and no reliance on transfer-history
reconstruction. CLMM-style fungible/NFT shares were rejected because
Dropset positions are fungible *within* a vault (a pro-rata basket,
no price range), so neither a fungible mint nor a position NFT buys
anything the `VaultDepositor` doesn't.

Invariant: `leader_shares + Σ VaultDepositor.shares == total_shares`.

### Depositor positions and cost basis

Each outside depositor's position in a vault is a `VaultDepositor`
account — one per `(vault, owner)` pair, PDA-seeded by
`("vault_depositor", vault, owner)`. It is the authoritative on-chain
record of both the depositor's claim and what they paid for it:

```rust
struct VaultDepositor {
    /// The vault this position is in.
    vault: Pubkey,
    /// The depositor. Bound by the PDA seeds
    /// (`"vault_depositor", vault, owner`), so the account is
    /// non-transferable — there is no authority field to reassign.
    owner: Pubkey,
    /// Pro-rata claim on the vault — the per-depositor term of the
    /// I6 invariant (`leader_shares + Σ VaultDepositor.shares ==
    /// total_shares`).
    shares: u64,
    /// Quote-denominated principal of the **remaining** position:
    /// `Σ (quote_in + base_in × entry_ref)` over deposits, reduced by
    /// the withdrawn slice's `released_basis` on withdraw. The basis of
    /// what is still in the vault — the cost basis the unrealized
    /// `net_pnl` is measured against.
    net_deposits: u64,
    /// Monotonic lifetime contributions: incremented by each deposit's
    /// `(quote_in + base_in × ref_now)`, **never reduced on withdraw**.
    /// This is "total deposited" and the stable denominator for an
    /// all-time return percentage (unlike `net_deposits`, it doesn't
    /// shrink when a depositor takes profit).
    gross_deposited: u64,
    /// Shares-weighted average reference price (quote per base)
    /// across deposits. A `Price`, same as `ReferencePrice.price`;
    /// stored canonical and decoded to a scaled value to compute PnL
    /// and to re-average on top-off (all cold-path). Re-encoding to an
    /// 8-significant-figure `Price` each top-off adds negligible drift
    /// to the running average.
    entry_ref_price: Price,
    /// Shares-weighted average VPS (`L / total_shares`) across
    /// deposits, as Q32.32 fixed-point `u64` — same encoding and
    /// practical bound as `Vault.hwm`. Basis for the "yield since
    /// open" figure.
    entry_vps: u64,
    /// Slot of the first deposit.
    opened_at: u64,
    /// **Signed** quote-denominated PnL crystallized by past
    /// withdrawals (a withdrawal can realize a loss). Discarded when
    /// the account is closed at zero shares. `i64` holds per-depositor
    /// PnL; widen to `i128` if a quote mint's atom scale could exceed
    /// it.
    realized_pnl: i64,
    /// Signed yield (ex-FX) component of `realized_pnl`.
    realized_yield: i64,
    /// Signed FX component of `realized_pnl`.
    /// Invariant: `realized_yield + realized_fx == realized_pnl`.
    realized_fx: i64,
}
```

Every basis field is captured from **on-chain** state at deposit
time — `entry_vps` from the vault's `L / total_shares`,
`entry_ref_price` from the leader's live `ReferencePrice`. No oracle
is needed to *record* basis; a price feed is only needed at *display*
time, to mark a position's current value (`ref_now`).

**Top-off (deposit into an existing position).** A second deposit
merges the new lot into the running averages, weighted by shares
(`s` = prior `shares`, `Δs` = `shares_out` this deposit; `base_in`,
`quote_in` = this deposit's basket):

```text
shares'          = s + Δs
entry_vps'       = (s · entry_vps       + Δs · VPS_now) / shares'
entry_ref_price' = (s · entry_ref_price + Δs · ref_now) / shares'
net_deposits'    = net_deposits + (quote_in + base_in · ref_now)
gross_deposited' = gross_deposited + (quote_in + base_in · ref_now)
```

**PnL decomposition (display only).** The protocol math stays
oracle-free (it stores basis, not PnL); a UI marks a position to a
display price `ref_now` (the live `ReferencePrice`, or an external FX
feed). Both `entry_ref_price` and `ref_now` are `Price` values,
decoded to a common scale before the arithmetic below (see **Price**).
For a position of `shares` in a vault holding `B` base / `Q`
quote atoms over `S_tot` total shares, the current basket is
`base_out = shares × B / S_tot`, `quote_out = shares × Q / S_tot`,
and:

```text
current_value     = quote_out + base_out × ref_now
value_at_entry_fx = quote_out + base_out × entry_ref_price
yield_pnl         = value_at_entry_fx − net_deposits       # spread, ex-FX
fx_pnl            = base_out × (ref_now − entry_ref_price)  # FX direction
net_pnl           = current_value − net_deposits = yield_pnl + fx_pnl
```

`yield_pnl` is the depositor's share of spread capture vs. adverse
selection, valued at constant FX; `fx_pnl` is the directional move of
the underlying pair on the base they hold. Together they are the
per-depositor form of the **APR (leader skill) × basket price move
(directional)** split in **APR / yield accounting**. Because the
position is soulbound, the basis is always the depositor's own — there
is no transfer or lot-attribution ambiguity to resolve.

Two caveats on these display figures:

- **The yield/FX split is exact in total but approximate per leg.**
  `net_pnl` is always exact (the `entry_ref_price` terms cancel). But
  `fx_pnl` marks the *current* base holding `base_out` against the
  *shares-weighted* `entry_ref_price`, while a lot's real FX exposure
  is base-weighted; across top-offs at different reference prices the
  yield/FX attribution drifts (bounded by `base_out ×` the spread of
  entry prices, worst under large FX moves between top-offs). The two
  legs always sum back to the exact `net_pnl`.
- **"Yield since open %" is a separate, geometric metric.** The
  headline `VPS_now / entry_vps − 1` is the FX-neutral, oracle-free
  VPS growth — the per-depositor APR. It is **not** equal to
  `yield_pnl / net_deposits`: VPS is a geometric measure
  (`L = isqrt(base × quote)`), whereas `yield_pnl` is an arithmetic
  constant-FX quote value, so the two diverge as the inventory ratio
  moves. Show the VPS ratio as the headline % and `yield_pnl` as the
  dollar attribution; don't derive one from the other.

**All-time PnL.** The figures above are **unrealized** — they cover
only the shares still in the vault. Each `Withdraw` crystallizes the
withdrawn slice into the account's `realized_*` accumulators (see
**Withdraw**), so a depositor's lifetime figures add the two:

```text
all_time_yield = realized_yield + yield_pnl
all_time_fx    = realized_fx    + fx_pnl
all_time_pnl   = realized_pnl   + net_pnl = all_time_yield + all_time_fx
all_time_pct   = all_time_pnl / gross_deposited
```

The percentage uses `gross_deposited`, not `net_deposits`:
`net_deposits` is the basis of the **remaining** position and shrinks
pro-rata on withdraw, so dividing by it would make the headline
percentage jump every time a depositor takes profit. `gross_deposited`
only ever grows, so it is both "total deposited" and a stable
denominator — the same convention a CLMM venue uses when it bases
position PnL% on lifetime deposits. Because the account is closed at
zero shares, these figures span from the first deposit to a full
exit; carrying them across a close-and-reopen needs an external
indexer.

Note the realized accumulators are marked at the vault's **on-chain
`reference_price`** at each withdrawal, whereas the unrealized
`yield_pnl` / `fx_pnl` use the display `ref_now` — which may be an
external feed. So when the display price differs from the on-chain
reference, only the **totals** (`all_time_pnl`) reconcile across the
realized/unrealized boundary; the per-leg yield/FX split does not.

## Caller mechanics

Every instruction that targets a specific vault — leader-callable,
outside-depositor-callable, permissionless, and admin — passes a
pointer into the market account's data region pointing directly at
the vault, avoiding any list walk. Before touching the vault, the
program performs three checks. The first two are the same for every
caller; the third is the per-ix authority gate.

1. **Bounds.** The entire vault struct fits within the data region:
   `vaults_start <= ptr && ptr + size_of::<Vault>() <= account_data_end`,
   where `vaults_start = account_data_base + size_of::<MarketHeader>()`.
1. **Alignment.** `(ptr - vaults_start) % size_of::<Vault>() == 0` —
   guarantees the pointer lands on a real vault boundary, so the
   cast to `&mut Vault` is well-formed.
1. **Authority.** Differs by instruction:
   - **Quote-mutating** (`SetReferencePrice`, `SetLiquidityProfile`):
     `vault.quote_authority == signer && !vault.frozen` — single
     compare on the hot path, no branching for the unset case
     (`quote_authority` is always populated; see
     `SetQuoteAuthority`).
   - **Leader-only** (`SetQuoteAuthority`,
     `SetAllowOutsideDepositors`, `CloseVault`):
     `vault.leader == signer`.
   - **Deposit**: leader path requires `vault.leader == signer`;
     otherwise outside path requires both
     `vault.allow_outside_depositors == 1` (leader opt-in) and
     `vault.outside_deposits_approved == 1` (admin approval).
   - **Withdraw**: leader path requires `vault.leader == signer`;
     otherwise outside path requires a `VaultDepositor` PDA seeded by
     `(vault, signer)` with `shares >= shares_in` (the seeds bind the
     account to the signer, proving ownership). See
     **Vault → Frozen and tombstoned vaults** for the wind-down
     behavior on non-active vaults.
   - **Permissionless** (`Realize`): no signer check. The leader
     has the strongest economic incentive, but anyone may call —
     useful for indexers and keepers that want to pin HWM at a
     known point in time.
   - **Admin-only** (`FreezeVault`, `SetOutsideDepositsApproved`,
     `SetMinLeaderShare`, `SetMarketFeeConfig`):
     `signer ∈ registry.admins`.

No discriminant tag is needed: the vault region is homogeneous, so
(1) + (2) fully determine that `ptr` refers to a valid `Vault`. The
`leader` field doubles as an emptiness marker — `Pubkey::default()`
means "on the free list / unassigned," and updates against such
vaults are rejected by (3).

**Zero-data leader accounts.** The pointer scheme assumes the market
account's data region starts at a known offset in the transaction's
input memory map. For this to hold under static addressing, the
leader's signer account must carry **zero account data** — any
variable-size payload on the leader account would shift downstream
offsets and break direct addressing.

Simplified input buffer schematic:

```txt
+---------------+-------------------+----------------+
| n_accounts    | Leader account    | Market account |
| (u64)         | (signer, 0 data)  |                |
+---------------+-------------------+----------------+
                                    ^
                                    |
                             fixed offset
```

## Leader operations

A leader joins a market by calling `OpenVault` to allocate a vault
sector (paying the market's open-vault fee, `market.fee_config`), then
seeding the vault with their
first `Deposit`, then `SetLiquidityProfile` to lay down their bid/ask
ladder as offsets from a reference price. From there, steady-state
activity is just `SetReferencePrice` on the hot path — sliding the
whole ladder by updating a single anchor price. `SetLiquidityProfile` can
be re-called to reshape the ladder as needed.

Authority gates and pointer validation are uniform across all
instructions in this section; see **Caller mechanics**.

### OpenVault

Called by anyone to allocate a vault sector and become its leader.
The caller transfers `market.fee_config.atoms` of
`market.fee_config.mint` to the Registry's fee ATA
(`get_associated_token_address_with_program_id` over
`(registry_pda, fee_config.mint, token_program)`) — unless the signer
is an admin (fee
waived; admins may also pass a separate `leader: Pubkey` argument to
open a vault on someone else's behalf — that pubkey becomes
`Vault.leader`). The caller passes the fee mint and its owning token
program; the program reads the token program from the **mint's account
`owner`** (validating `token_program == fee_config.mint.owner`) to both
derive the fee ATA above and issue the transfer CPI — classic SPL
Token and Token-2022 derive different ATAs for the same mint, so the
program is never assumed. If `fee_config.mint` carries the Token-2022
transfer-fee extension, the amount landing in the fee ATA is less than
`atoms`; admins should configure only mints without a transfer fee
(see `SetMarketFeeConfig`).

Caller arguments stamped onto the vault:

- `perf_fee_rate: Ppm32` — immutable thereafter.
- `quote_authority: Option<Pubkey>` — if `None`, the protocol stamps
  `Vault.leader`. Rotatable post-open via `SetQuoteAuthority`.
- `allow_outside_depositors: bool` — toggleable post-open via
  `SetAllowOutsideDepositors`.

Side effect: the instruction stamps `Vault.min_leader_share` from the
market's `MarketHeader.default_min_leader_share` (the skin-in-the-game
floor this vault will be held to; admin-overridable per vault via
`SetMinLeaderShare`). The vault is otherwise initialized empty
(`base_atoms`, `quote_atoms`, `total_shares`, `leader_shares`, `hwm`,
`frozen`, `outside_deposits_approved` all zero); the leader seeds
inventory with their first `Deposit` (see **Depositor operations**
below). Because `outside_deposits_approved` starts at 0, a new vault
cannot take outside baskets until an admin calls
`SetOutsideDepositsApproved` — see **Leader operations**.

If this market's `fee_config.mint` changes (via `SetMarketFeeConfig`)
after this vault was opened, old fees remain in the prior registry fee
ATA and admins sweep both — the vault itself is unaffected.

The new vault is inserted via the **Insert** operation in
**Storage layout** (O(1); reuses a freed sector when available). If
`vaults.len() == registry.max_vaults_per_market`, `OpenVault` fails
and the caller must wait for an existing vault to close.

### SetLiquidityProfile

Setup-and-reshape path. Writes the full `LiquidityProfile` — all levels
expressed in ppm/bps and slot offsets, never absolute. Called after
seeding the vault and any time the leader wants to reshape their
ladder.

**Per-side collateral invariant.** Validated before the write:

```text
Σ bids[i].size_bps ≤ 10000
Σ asks[i].size_bps ≤ 10000
```

A sum of exactly 10000 commits the full inventory leg across the
ladder; smaller sums leave an unallocated reserve. Either side
exceeding 10000 is rejected — it would over-commit the leg and
turn `Position.size` into an unenforceable nominal. The check is
N_LEVELS adds and one comparison per side.

The instruction reads and increments `market.nonce`, writes
the new value (OR'd with `FLUSH_BIT`) to `reference_price.stamp`,
and leaves `reference_price.price` and `reference_price.quote_slot`
unchanged. Bumping the nonce on reshape means the new ladder takes
fresh time priority at match time; otherwise a leader could quietly
reshape into a more aggressive ladder while keeping a stale stamp
that beats fresher quotes from other vaults at the same price. The
next taker re-materializes `Vault.remaining` from the new profile
and current inventory.

### SetReferencePrice

Hot path. Takes `(price: Price, quote_slot: u32)` from the leader.
`quote_slot` is validated:

- `quote_slot <= current_slot` — no future-dating (which would
  extend the effective expiry window artificially).
- `current_slot - quote_slot <= MAX_BACKDATE` — sanity cap;
  default 50 slots (~20s on Solana mainnet). Backdating only
  shortens the effective expiry window, so this is self-grief
  rather than an exploit, but worth bounding.

Reads `market.nonce`, writes `Vault.reference_price` as two aligned
`u64` stores: one for `market.nonce | FLUSH_BIT` as `stamp`, one
packing `(price, quote_slot)`. Increments `market.nonce`. Setting
`FLUSH_BIT` arms a pending materialization of `Vault.remaining`,
deferred to the next taker — so the leader write stays at two stores
regardless of `N_LEVELS`. No vault iteration, no reallocations, no
profile touch — asm-optimized, analogous to a propAMM
reference-price update.

Off-chain pre-signing: because `quote_slot` is supplied by the leader
rather than read from the clock, a quote can be signed at slot N and
relayed at slot M > N, with on-chain expiry math anchored to N.

### SetQuoteAuthority

Leader-only. Writes `Vault.quote_authority = new`, where `new` may be
any pubkey including the leader's own (effectively revoking
delegation). Useful for rotating a hot wallet, delegating to a
third-party MM firm, or moving quoting authority while keeping
custody of inventory.

### SetAllowOutsideDepositors

Leader-only. Writes `Vault.allow_outside_depositors = flag`. Flipping
to `false` blocks **new** outside `Deposit` ix but does not affect
existing outside depositors, who can continue to `Withdraw` normally.

This is only the leader's half of the outside-deposit gate: an
outside `Deposit` also requires admin approval
(`Vault.outside_deposits_approved == 1`, set via
`SetOutsideDepositsApproved`). Setting this flag to `true` on a
vault an admin has not approved has no effect on outside flow until
that approval lands.

### CloseVault

Leader-only. Moves the vault from the active DLL to the tombstone
DLL: matching stops, depositor flows stay open until the vault
drains. This is the intended leader-initiated lifecycle exit ("done
quoting this market"). See **Vault → Frozen and tombstoned vaults**
for full state semantics and the comparison with `FreezeVault`.

### FreezeVault

Admin-only. Sets `Vault.frozen = 1`. This is the protocol's
revocation lever against a misbehaving leader: the vault stays on
the active DLL (existing levels still match until their
`expires_at`) but cannot be re-quoted. There is no "unfreeze" — to
re-enter, the same leader pubkey pays the open-vault fee again and
starts a new vault. See **Vault → Frozen and tombstoned vaults** for
full state semantics and the comparison with `CloseVault`.

### SetOutsideDepositsApproved

Admin-only. Writes `Vault.outside_deposits_approved = flag`. This is
the admin's half of the two-key gate on outside deposits: a vault
takes outside baskets only when an admin has approved it
(`outside_deposits_approved == 1`) **and** the leader has opted in
(`allow_outside_depositors == 1`). New vaults start unapproved
(`outside_deposits_approved == 0` at `OpenVault`), so an admin must
explicitly sign off before any outside depositor can join.

Setting the flag back to `false` **revokes** approval: it blocks
**new** outside `Deposit` ix but, like the leader's
`SetAllowOutsideDepositors`, does not affect existing outside
depositors, who can continue to `Withdraw` normally. Approval is
independent of `frozen` — freezing a vault already rejects all
deposits (see **FreezeVault**), so revoking approval is the lighter
lever for gating only the outside-deposit path while leaving the
leader free to keep quoting and managing inventory.

### SetMinLeaderShare

Admin-only. Writes `Vault.min_leader_share = value` (ppm), overriding
the floor stamped from `MarketHeader.default_min_leader_share` at
`OpenVault`. This is the per-vault skin-in-the-game lever: lowering it
lets a vault run with a smaller leader stake — e.g. seating an
issuer-funded vault where a stablecoin issuer supplies most of the
basket as an outside depositor and the leader holds only a thin
slice — while leaving every other vault on the market at the default.
Pairs naturally with `SetOutsideDepositsApproved`: the same admin sign-off
that opens a vault to outside baskets can also relax its floor.

The new value takes effect on the next `Deposit` or leader
`Withdraw`; it does not retroactively force an out-of-floor vault
back into compliance. Raising the floor above the current ratio
simply blocks further outside deposits (and floor-violating leader
withdrawals) until the leader tops up, exactly as the standing check
in **Vault → Skin-in-the-game floor** describes.

### SetMarketFeeConfig

Admin-only. Overwrites `MarketHeader.fee_config` (the per-`OpenVault`
fee: `mint` and `atoms`), seeded at market creation from
`Registry.default_fee_config`. Use it to retune the open-vault fee on
a single market — raise or lower the amount, or switch the fee to a
different mint — while every other market stays at its own value.

The admin passes the new mint **and its owning token program**, which
the instruction validates as `token_program == mint.owner` so the
stored mint is always backed by a real, classifiable token program
(classic SPL Token or Token-2022). The token program is not stored —
it is re-derived from the mint's owner on each `OpenVault` (see there)
— so this check exists only to reject a mint/program mismatch at
configuration time. Admins should configure only mints **without** the
Token-2022 transfer-fee extension, since that extension would deliver
less than `atoms` into the registry fee ATA.

Changing the mint routes future fees to a fresh registry ATA
(`get_associated_token_address_with_program_id` over
`(registry_pda, mint, token_program)`); fees already collected stay in
the prior ATA and
admins sweep both. Takes effect on the next `OpenVault`; vaults
already open are unaffected (the fee is charged only at open time).

## Depositor operations

`Deposit` and `Withdraw` use the same pointer validation as leader
ix (see **Caller mechanics**), and the same instruction discriminants
for both the leader and outside depositors. The path splits
internally on `signer == vault.leader`: the leader updates
`Vault.leader_shares` directly, while outside depositors update
`shares` on their `VaultDepositor` account (PDA seeded by
`("vault_depositor", vault, owner)`; see **Depositor positions and
cost basis**). The `VaultDepositor` account is required on the
outside path — `init_if_needed` on `Deposit`, `close`-on-empty on
`Withdraw` — and **omitted on the leader path**. No SPL share mint or
ATA exists anymore; shares are pure on-vault bookkeeping on both
paths.

Both `Deposit` and `Withdraw` realize the vault first — see
**Vault → Realize**.

### Deposit

Caller sizes the deposit by **one leg** — a base amount *or* a quote
amount — and passes a max basket `(max_base_in, max_quote_in)` for
slippage protection. The argument is a tagged
`amount_in: { Base(u64) | Quote(u64) }`: the depositor commits the
leg they hold ("add 1,000 USDC") and the other leg follows from the
vault's current ratio, mirroring the linked inputs in the deposit UI.
The sized leg fixes `shares_out`, and the basket is then derived from
`shares_out` at the current ratio:

```text
shares_out =
  floor(amount_in × total_shares / base_atoms)   // amount_in = Base(_)
  floor(amount_in × total_shares / quote_atoms)  // amount_in = Quote(_)

base_in  = ceil(shares_out × base_atoms  / total_shares)
quote_in = ceil(shares_out × quote_atoms / total_shares)
```

`shares_out` is rounded **down** and the basket **up**, so the
depositor always backs their minted shares with a full pro-rata
basket; any rounding dust stays on the depositor's side (their sized
leg is an upper bound — `base_in ≤ amount_in` when sizing by base,
and symmetrically for quote), preserving VPS for existing depositors
(invariant I1). The instruction reverts if `base_in > max_base_in`
or `quote_in > max_quote_in` — the ratio moved beyond the caller's
tolerance. The basket is transferred from the depositor to the
treasuries, then:

- **Leader path** (`signer == vault.leader`): increment
  `Vault.leader_shares` by `shares_out`. No `VaultDepositor`.
- **Outside path** (`signer != vault.leader`): credit `shares_out`
  to the caller's `VaultDepositor` account (`init_if_needed`), and
  record cost basis on it (see **Depositor positions and cost
  basis** for the field semantics and the top-off merge). A first
  deposit sets `shares`, `entry_vps`, `entry_ref_price`,
  `net_deposits`, `gross_deposited`, and `opened_at`; a top-off into
  an existing account merges them shares-weighted. Requires both
  `Vault.allow_outside_depositors == 1` (leader opt-in) and
  `Vault.outside_deposits_approved == 1` (admin approval); either
  flag unset rejects the deposit. See
  **Leader operations → SetOutsideDepositsApproved**.

`Vault.total_shares` is incremented in both paths.

**Skin-in-the-game check.** After update, if the caller is not the
leader and
`leader_shares × 1_000_000 < vault.min_leader_share × total_shares`,
the instruction reverts. The floor is the vault's own
`Vault.min_leader_share`. The check uses on-vault numbers only — no
ATA load needed. See **Vault → Skin-in-the-game floor**.

**Seeding (first deposit).** If `total_shares == 0`, the vault has
never been seeded. There is no ratio yet to derive one leg from, so
single-leg sizing does not apply: the first depositor **must** be the
leader and must supply both legs explicitly,
`base_in > 0 && quote_in > 0` — a zero leg would yield
`total_shares = 0` and re-trigger seeding on the next deposit (and
divide by zero in the pro-rata basket math). The instruction sets
`total_shares := isqrt(base_in × quote_in)`,
`leader_shares := total_shares`, and `hwm := Q32.32(1.0)`. No
`VaultDepositor` is created on seeding (the leader's stake lives on
`Vault.leader_shares`).

Deposits against frozen or tombstoned vaults are rejected.

### Withdraw

Caller specifies `shares_in` to burn. The vault delivers a pro-rata
basket:

```text
slice_base  = floor(shares_in × base_atoms  / total_shares)
slice_quote = floor(shares_in × quote_atoms / total_shares)
```

Rounding down keeps any dust in the vault for the benefit of
remaining depositors. Then:

- **Leader path** (`signer == vault.leader`): decrement
  `Vault.leader_shares` by `shares_in`. The leader has no
  `VaultDepositor`, so no basis or realized accounting applies.
- **Outside path** (`signer != vault.leader`): decrement `shares` on
  the caller's `VaultDepositor` by `shares_in` (the PDA seeds bind the
  account to `signer`, so authority is gated by ownership and
  `shares_in <= VaultDepositor.shares`). The withdrawn slice's PnL is
  crystallized and the basis reduced, per the accounting below.

On the outside path, before the basis is reduced the withdrawn
slice's PnL is added to the signed `realized_*` accumulators, marked
at the vault's current `reference_price` (`r_now`) — the same source
`entry_ref_price` was captured from, so the realized split's FX and
yield legs share one reference. `slice_base` / `slice_quote` are the
withdrawn basket above; `released_basis` is the floored slice of the
remaining basis, and `net_deposits` is reduced by exactly that:

```text
released_basis  = floor(net_deposits × shares_in / shares)
realized_fx    += slice_base × (r_now − entry_ref_price)
realized_yield += slice_quote + slice_base × entry_ref_price − released_basis
realized_pnl   += slice_quote + slice_base × r_now − released_basis
net_deposits'   = net_deposits − released_basis
```

`entry_vps`, `entry_ref_price`, and `gross_deposited` are left
unchanged — a proportional reduction preserves the shares-weighted
averages, and `gross_deposited` only ever grows (on deposit). When
`shares` reaches 0, `close` the account and return its rent to the
owner; this discards the accumulators, so all-time PnL spans one
open→full-exit lifetime (see
**Depositor positions and cost basis → All-time PnL**).

`Vault.total_shares` is decremented in both paths; the basket is
transferred from the treasuries to the caller.

If the caller is the leader against an **active** vault, the
post-burn ratio must remain at or above `vault.min_leader_share`.
The floor is **bypassed for frozen and tombstoned vaults** — see
**Vault → Skin-in-the-game floor** and
**Vault → Frozen and tombstoned vaults**.

If `total_shares` reaches 0 on a frozen or tombstoned vault, the
sector returns to the free list via **Reclaim** in **Storage layout**.

## Order matching

There is no persistent order book account. Each take builds a fresh
**ephemeral book** on the SVM program heap, uses it to fill the
taker, and discards it when the instruction returns. Levels are
read from `Vault.remaining`, where prices, sizes, and per-level
expiries are already materialized — the matching engine does no bps
arithmetic at match time.

### Book construction

On every taker instruction:

1. **Iterate** `MarketHeader.vaults` (active DLL only — tombstoned
   and frozen-then-drained vaults are not visited; frozen vaults
   that still sit on the active DLL are visited but their levels
   are skipped via per-level expiry).

1. **Range-check** the vault's `reference_price.price`. Drop the
   vault entirely if out-of-range (this is the deferred validation
   from the leader's hot path — a nonsense price renders the vault
   unmatchable here).

1. **Flush if armed.** If `FLUSH_BIT` is set on
   `reference_price.stamp`, materialize `Vault.remaining` from
   `LiquidityProfile` and current inventory per the formulas in
   **LiquidityProfile → Flush**, and clear the bit with one `u64` store.

1. Iterate the relevant side of `remaining` (asks for a buy taker,
   bids for a sell taker).

1. **Push** each
   `(remaining.price, remaining.size, stamp & !FLUSH_BIT, vault_ptr, level_idx)`
   tuple onto a binary heap allocated on the program heap, skipping
   levels where `remaining.size == 0` or
   `current_slot >= remaining.expires_at`. The heap is keyed so the
   next-to-pop is the best price with the oldest nonce on ties. For
   asks, "best" is lowest price — a min-heap on `(price, nonce)`
   works directly. For bids, "best" is highest price — use a
   min-heap on `(Price::MAX − price, nonce)` (or equivalently invert
   the price comparator while leaving nonce ascending). A naïve
   max-heap on `(price, nonce)`
   for bids would flip the nonce comparison and let *newer* quotes
   win on equal-price ties, violating price-time priority.
   `FLUSH_BIT` is masked off the stamp before keying so a
   just-flushed vault doesn't sort younger than a previously-flushed
   one with the same underlying nonce.

1. **Pop** from the heap and compute the fill. Units depend on side:
   ask `level.size` is in base, bid `level.size` is in quote (see
   **LiquidityProfile → Flush**), so the min runs in whichever unit
   the maker's leg is denominated in.

   - **Asks** (taker buying base):
     `fill_base = min(taker_unfilled_base, level.size, base_atoms)`;
     debit `base_atoms -= fill_base`, credit
     `quote_atoms += fill_base × level.price`.
   - **Bids** (taker selling base): let
     `taker_unfilled_quote = taker_unfilled_base × level.price`;
     `fill_quote = min(taker_unfilled_quote, level.size, quote_atoms)`;
     debit `quote_atoms -= fill_quote`, credit
     `base_atoms += fill_quote / level.price`.

   In both cases the trade never debits more inventory than the
   vault holds. (A popped entry with `vault_leg == 0` yields a
   zero fill; the loop moves on.) Decrement the taker's unfilled
   amount, decrement the popped level's `Vault.remaining.<side>[i].size`
   by the fill, and accrue the taker fee from `market.taker_fee`.
   Continue until the taker is filled, the next heap top exceeds the
   taker's limit price, or the heap is drained.

1. **Tear down.** The heap buffer is freed with the transaction;
   debited inventory, `Vault.remaining.size` decrements, the cleared
   `FLUSH_BIT` on any flushed vault, and `market.nonce` persist to
   chain. Takers bump `market.nonce` per fill but never touch
   `reference_price.stamp` beyond clearing `FLUSH_BIT`, and never
   touch `Vault.remaining.price` or `Vault.remaining.expires_at`.

### Crossed leader quotes

The protocol **does not** auto-match leaders against each other. If
Leader A's ask drifts below Leader B's bid (e.g. because A just
`SetReferencePrice`'d without observing B), nothing happens on chain
until the next taker arrives. A crossed book is an arbitrage
opportunity — any taker can profit from it, including the leaders
themselves (a leader is just another pubkey on the taker side, so
self-arbitraging a stale neighbor is the cheapest path to clean it
up) — which gives leaders a standing incentive to keep their
reference prices honest without the matching engine needing a
leader-vs-leader pre-pass.

## Events and emission

The protocol emits structured events on its **cold paths** so off-chain
indexers can reconstruct trades, liquidity flows, and fee accrual. The
**hot path emits nothing** — `SetReferencePrice` and `SetLiquidityProfile`
stay at two aligned `u64` stores (see **SetReferencePrice**); a leader's
quote refresh is recovered off-chain from account-state diffs, not from
an event.

**Mechanism — inner-instruction events (full fidelity, never dropped).**
Events are Anchor `#[event]` structs emitted via `emit_cpi!`: a self-CPI
whose *instruction data* carries the event, recorded as an inner
instruction. This is chosen for **full fidelity** — every fill of every
take must be recorded, even when a taker blasts through many price
levels. Inner-instruction data is **not** subject to the runtime's
cumulative ~10 KB-of-log-bytes-per-transaction limit
(`LOG_MESSAGES_BYTES_LIMIT`), so a large sweep never drops a fill. The
log-based alternative (`sol_log_data`/`emit!`) costs zero extra accounts
but **silently truncates** past that hard per-transaction ceiling — an
unacceptable, unrecoverable loss for the canonical trade record — so it
is rejected here. `emit_cpi!` requires the `event-cpi` feature and
appends two accounts, the `event_authority` PDA and the `program`, to
every emitting instruction.

**Account cost — cheap on the fill, matters only for routers.** This is
negligible on the taker fill itself: Dropset keeps the entire book
(`MarketHeader` + every vault) in a **single market account**, so a take
loads only a handful of accounts (the market, both treasuries, the
taker's two ATAs, the token program) and reconstructs the book in program
memory — it is **not** account-hungry, and +2 is immaterial. The cost
that matters is on **CPI callers (routers such as Jupiter/DFlow/Titan)**,
which thread our accounts into a multi-hop route under a tight
per-transaction account budget. If that budget ever binds, the
optimization is a **bare self-CPI** that carries the event in instruction
data but drops the `event_authority` auth PDA (saving one account — the
`program` account is still required for any self-CPI); origin is then
authenticated off-chain by program id + instruction binding. Default to
standard `emit_cpi!` for IDL/tooling compatibility.

**Emission points.** The **taker fill** (at book tear-down), `Deposit`,
`Withdraw`, `OpenVault`, and `Realize` emit. The leader quote-refresh
instructions do not.

**Why fills must be events, not account diffs.** `market.nonce` is
bumped on every fill and every quote update, and a geyser stream
delivers end-of-slot *coalesced* account state — so per-fill price,
counterparty, and size cannot be recovered from account diffs alone. The
fill event is therefore the authoritative trade record. It carries the
taker and, per leg, the matched vault's **`leader` and `quote_authority`
directly** — not merely the vault's sector index, since sectors are
**reused via the free list** (see **Storage layout**), so an index is not
a stable attribution key; the inner-instruction budget easily affords the
pubkeys — alongside amounts, price, and post-fill inventory. **The
protocol does not stamp
an on-chain self-trade/wash flag** — there is no leader allowlist (see
**Registry**), a fresh wallet trivially defeats a signer-based check,
and the deliberately-minimized match loop should not carry it. Wash
classification is left to off-chain consumers, which have the
maker/taker identities to cluster on.

**Granularity — every leg recorded, never truncated.** A single take can
sweep many levels across many vaults (the heap pop loop in **Order
matching**), and **every leg is recorded** — no truncation, no revert for
an event-size reason. The per-leg `(vault, level)` records and the
take-level summary ride as inner-instruction data. A single CPI's
instruction data is itself bounded (~10 KB per call), but there is **no
cumulative cap** on inner-instruction data across a transaction, so a
sweep too large for one self-CPI simply **splits across multiple
self-CPIs** — full fidelity is preserved either way. Whether to emit one
aggregated `FillBatch` per self-CPI (fewer CPIs, less CU, must chunk when
a sweep exceeds one CPI) or one event per leg (simplest, always fits,
more CPIs / CU) is a **CU/byte optimization** that does not affect
fidelity; see the plan's open decision.

**Serialization mode.** Anchor v2's `#[event]` macro picks between two
serializers: the default (`wincode` with a borsh-wire-compatible
config; supports `Vec` / `String` / `Option`) and opt-in zero-copy
`#[event(bytemuck)]` (`repr(C)` POD structs only, written as
`bytemuck::bytes_of(self)`). The **fill event uses
`#[event(bytemuck)]`**: it is fixed-size by construction (taker plus
per-leg `leader` / `quote_authority` pubkeys, amounts, price, post-fill
inventory) and is the hot-path emission, so both the zero serializer
cost and the small stack footprint of the event-struct literal at the
macro site matter. The cold-path events (`Deposit`, `Withdraw`,
`OpenVault`, `Realize`) use the default `#[event]` — they benefit from
dynamic fields and emit too rarely for bytemuck to pay back.

**Schema source of truth.** The event field layouts are the program's
`#[event]` structs, surfaced verbatim in the generated IDL; the IDL is
the canonical schema that off-chain clients are generated from, and the
self-CPI instruction data decodes against it. Default-mode events
encode borsh-wire-compatible, so existing borsh-decoder tooling keeps
working unchanged; bytemuck events surface in the IDL as a `repr(C)`
blob (tagged `{serialization:"bytemuck",repr:{kind:"c"}}`) and decode
by offset — indexers must read the IDL tag and dispatch accordingly.
Verified macro expansion and CU sources are in
[`docs/research/svm-heap-emit-cpi.md`](research/svm-heap-emit-cpi.md)
§4.

## Operating model

Reader-facing notes about how a vault behaves in steady state.
Nothing here changes protocol semantics; it explains how leaders
manage drift and how depositor returns decompose.

### Rebalancing

When a vault drifts heavy on one leg (e.g. base depletes, quote
accumulates), the leader has three levers in increasing order of
cost:

1. **Do nothing — auto-rebalance.** Because sizes are
   pct-of-inventory, the next flush after a drift automatically
   makes the depleted side's quotes smaller and the accumulated
   side's larger. If quote is heavy, bids materialize larger
   (`quote_atoms × size_bps / 10000`) while asks shrink. Larger
   bids attract sellers, who dump base onto the leader, rebuilding
   the base leg. The ladder is self-correcting as long as the
   reference price is reasonable.

1. **Bump `reference_price.price`.** Shifts the whole ladder up or
   down without changing offsets or sizes. Moving the reference up
   makes bids more attractive to sellers (they get more quote per
   base) and asks less attractive to buyers — net: invites selling
   to the leader, rebuilds base. One hot-path write
   (`SetReferencePrice`), asm-cost identical to a normal price update.

1. **Reshape via `SetLiquidityProfile` with asymmetric ladders.** `bids`
   and `asks` are independent arrays — tighten one side's
   `price_offset` or grow one side's `size_bps` to skew flow more
   aggressively than the auto-rebalance alone provides. Costs a
   full `LiquidityProfile` rewrite + materialization on the next take.

For most operating regimes (1) suffices. (2) and (3) are levers for
when the leader has a directional view or specifically wants to
accelerate rebalancing past what pct-of-inventory provides on its
own.

### APR / yield accounting

Headline vault APR is **annualized VPS growth**: pure spread accrual,
by construction independent of directional moves in the underlying
pair. A price move with no trading leaves token counts (and therefore
L and VPS) unchanged; only spread capture or adverse selection move
VPS.

| Event                           | L         | VPS / APR | Basket quote value     |
| ------------------------------- | --------- | --------- | ---------------------- |
| Underlying pair moves, no fills | unchanged | flat      | up or down (direction) |
| Leader captures spread on flow  | grows     | positive  | up                     |
| Leader adversely selected       | shrinks   | negative  | down                   |

The depositor's total quote-denominated return decomposes cleanly
into **APR (leader skill) × basket price move (directional)**; the
two are separately attributable. The protocol math is oracle-free. UIs
that want to display a quote-converted total return can layer in a
price feed for display only — no on-chain dependency. The
per-depositor form of this split — yield vs. FX PnL against a
position's stored entry basis — is in **Depositor positions and cost
basis**.

APR can go **negative** when the leader is consistently adversely
selected. That is the same metric working in both directions, and it
is the right signal for depositors deciding whether to stay or pull
their basket.

### Versus concentrated-liquidity APR

A concentrated-liquidity venue reports two APR flavors; Dropset needs
neither, which is worth stating because depositors arriving from those
venues will expect them.

- A **realized** fee APR taken from the delta of a fee-growth
  accumulator between two snapshots. Dropset's headline is the same
  delta idea on **VPS** rather than a fee counter — but VPS is
  *signed*, so it already nets adverse selection, whereas a fee
  accumulator only ever rises and books impermanent loss separately.
  Dropset's number is therefore net market-making performance, not
  gross fees, and it **auto-compounds** (spread stays in the vault),
  where a CLMM fee APR is linear until the LP manually re-collects and
  redeploys. Label the headline accordingly: it is a compounding
  figure, net of adverse selection — not a gross fee rate.
- A **forward** estimate that scales the pool headline by per-position
  multipliers (concentration, time-in-range, transfer-fee haircut).
  This **collapses here**: vault positions are homogeneous — one
  fungible pro-rata basket at one VPS — so the headline APR is already
  each depositor's APR, adjusted only by entry timing
  (`yield_since_open`). There is no range and no time-in-range, so the
  in-range-TVL denominator that makes trailing CLMM APR swing wildly
  does not exist — APR is measured against VPS, which moves only on
  fills and fee accrual, not on positions drifting in and out of a
  range.
