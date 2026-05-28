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
the `vault_open_fee` to call `OpenVault`). Outside depositors back
that leader's quotes with paired (base, quote) baskets and share in
spread capture, with a skin-in-the-game floor and per-share
high-water-mark performance fee aligning incentives. See **Vault**
below for details.

## Conventions

**Ppm (parts per million)** is the unit for all sub-basis-point rates in this spec:
1 ppm = 10⁻⁶ = 0.0001 bps; 1 bps = 100 ppm; 1% = 10,000 ppm; 100% = 1,000,000 ppm.
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
by paying `vault_open_fee.atoms` of `vault_open_fee.mint` to the
Registry's fee ATA — derived as
`get_associated_token_address(registry_pda, vault_open_fee.mint)` —
no storage needed on the Registry itself. Admins may call the same
instruction without paying, including on behalf of others (useful for
protocol-onboarded market makers). If admins later change
`vault_open_fee.mint`, a fresh ATA is used going forward and prior
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
    /// Taker fee rate stamped into `MarketHeader.taker_fee_rate`
    /// at market creation. Admins may change a market's fee
    /// later; this field only sets the initial value.
    default_taker_fee_rate: Ppm16,
    /// Minimum fraction of vault shares the leader must hold,
    /// in ppm (1,000,000 = 100%). Enforced at `Deposit` and
    /// leader `Withdraw` against active vaults. Default 50_000 = 5%.
    /// See **Vault → Skin-in-the-game floor**.
    min_leader_share: Ppm32,
    /// Fee paid to the protocol treasury on `OpenVault`. Waived
    /// when the signer is an admin.
    vault_open_fee: FeeConfig,
    /// Admins authorized to mutate the `Registry`, change
    /// per-market `taker_fee_rate`, call `FreezeVault`, and
    /// open vaults without paying `vault_open_fee`.
    admins: Set<Pubkey>,
}
```

Notably absent: there is **no leader allowlist**. Banning a pubkey
would be trivially defeated by registering a fresh wallet, so the
protocol does not maintain one. Admin power is exercised per-vault via
`FreezeVault` (see **Leader operations**), and the non-refundable
`vault_open_fee` acts as the only material gate on fresh entry: every
new wallet pays the fee again, so spinning up replacements after a
freeze has a real, repeated cost rather than being free.

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
    taker_fee_rate: Ppm16,

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
outside-depositor shares live as SPL tokens (see **Shares**)
and the leader's stake is non-SPL bookkeeping — neither imposes any
per-depositor storage on the vault sector itself.

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
    /// SPL mint for outside-depositor shares. Mint authority is a
    /// vault-owned PDA. Leader's own stake is tracked in
    /// `leader_shares` and is **not** minted as SPL tokens.
    /// Invariant: `leader_shares + share_mint.supply == total_shares`.
    share_mint: Pubkey,
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
    /// share_mint.supply).
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
    /// Set to 1 when an admin freezes the vault. See
    /// **Frozen and tombstoned vaults**.
    frozen: u8,
    /// Set to 1 if outside depositors are permitted. When 0, only
    /// the leader may `Deposit`; existing outside depositors (from
    /// before the flag was flipped) can still `Withdraw`. Mutable
    /// by the leader via `SetAllowOutsideDepositors`.
    allow_outside_depositors: u8,
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
    /// Custom 32-bit representation; range-checked by the taker
    /// at match time.
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

### Value-per-share and the L measure

Vault value is tracked via a dimensionless metric borrowed from
constant-product AMMs:

```
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

**I6. Invariant on total shares.** `total_shares = leader_shares +
share_mint.supply` at all times. Every path that mutates `total_shares`
mutates exactly one of the two terms by the same amount:

| Operation | `leader_shares` | `share_mint.supply` |
|---|---|---|
| `Deposit` (seeding; `s = 0` → leader path) | +Δs | 0 |
| `Deposit` (leader path) | +Δs | 0 |
| `Deposit` (outside path) | 0 | +Δs |
| `Withdraw` (leader path) | −Δs | 0 |
| `Withdraw` (outside path) | 0 | −Δs |
| `Realize` | +m | 0 |

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

```
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

`Registry.min_leader_share` (ppm; default 50_000 = 5%) is a hard
floor on the leader's stake in their own vault, enforced at the two
natural choke points:

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

| State | Set by | DLL membership | Quote ix | Deposit | Withdraw | Realize | Lifecycle exit |
|---|---|---|---|---|---|---|---|
| **Active** | default | active | accepted | accepted | accepted | accrues | becomes frozen or tombstoned |
| **Frozen** | admin via `FreezeVault` | stays on active DLL; takers skip via per-level expiry | rejected (`!vault.frozen` gate) | rejected | accepted; `min_leader_share` bypassed for the leader | no-op (HWM pinned at freeze time) | Reclaim when `total_shares == 0` |
| **Tombstoned** | leader via `CloseVault` | tombstone DLL; takers do not iterate | rejected (vault not visited) | rejected | accepted; `min_leader_share` bypassed for the leader | no-op (HWM pinned at close time) | Reclaim when `total_shares == 0` |
| **Reclaimed** | implicit on draining `Withdraw` | free DLL | n/a | n/a | n/a | n/a | sector available for reuse |

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
`vault_open_fee` again — on this or any other market.

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
    /// inventory leg, in basis points (10000 = 100%). Values above
    /// 10000 are clamped to 10000 at flush so the per-level
    /// allowance never exceeds the inventory leg. Materialized to
    /// atoms at flush from `inventory × min(size_bps, 10000) / 10000`.
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

```
asks_remaining[i].size       = base_atoms  × min(asks[i].size_bps, 10000) / 10000
asks_remaining[i].price      = ref.price   × (1_000_000 + asks[i].price_offset) / 1_000_000
asks_remaining[i].expires_at = ref.quote_slot + asks[i].expiry_offset

bids_remaining[i].size       = quote_atoms × min(bids[i].size_bps, 10000) / 10000
bids_remaining[i].price      = ref.price   × (1_000_000 −sat bids[i].price_offset) / 1_000_000
bids_remaining[i].expires_at = ref.quote_slot + bids[i].expiry_offset
```

A u128 intermediate is used during multiplication to avoid u64
overflow (relevant for both the price and size computations); the
result is truncated back to the native field width. The `−sat`
operator on bids is saturating subtraction — bid `price_offset`
values ≥ 1_000_000 ppm produce a 0 bid price, which is range-checked
out at match time. The `min(size_bps, 10000)` clamp keeps the
per-flush allowance bounded by the inventory leg even if the leader
sets `size_bps` above 100%. `FLUSH_BIT` is then cleared with one
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

Outside-depositor shares are SPL tokens minted from
`Vault.share_mint`. They live in standard ATAs, are transferable, and
compose with the rest of the Solana ecosystem (collateral in lending
markets, listed on aggregators, etc.).

The leader's stake is **not** an SPL token — it's tracked as
`Vault.leader_shares` so the skin-in-the-game floor cannot be evaded
by transferring shares to an alt wallet, and the leader's ATA never
has to be loaded at check time.

Invariant: `leader_shares + share_mint.supply == total_shares`.

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
     otherwise outside path requires
     `vault.allow_outside_depositors == 1`.
   - **Withdraw**: leader path requires `vault.leader == signer`;
     otherwise outside path requires the caller to hold > 0 SPL
     tokens on `vault.share_mint` (the burn from their ATA proves
     possession). See **Vault → Frozen and tombstoned vaults** for
     the wind-down behavior on non-active vaults.
   - **Permissionless** (`Realize`): no signer check. The leader
     has the strongest economic incentive, but anyone may call —
     useful for indexers and keepers that want to pin HWM at a
     known point in time.
   - **Admin-only** (`FreezeVault`): `signer ∈ registry.admins`.

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
sector (paying `vault_open_fee`), then seeding the vault with their
first `Deposit`, then `SetLiquidityProfile` to lay down their bid/ask
ladder as offsets from a reference price. From there, steady-state
activity is just `SetReferencePrice` on the hot path — sliding the
whole ladder by updating a single anchor price. `SetLiquidityProfile` can
be re-called to reshape the ladder as needed.

Authority gates and pointer validation are uniform across all
instructions in this section; see **Caller mechanics**.

### OpenVault

Called by anyone to allocate a vault sector and become its leader.
The caller transfers `registry.vault_open_fee.atoms` of the fee mint
to the Registry's fee ATA — `get_associated_token_address(registry_pda,
vault_open_fee.mint)` — unless the signer is an admin (fee waived;
admins may also pass a separate `leader: Pubkey` argument to open a
vault on someone else's behalf — that pubkey becomes `Vault.leader`).

Caller arguments stamped onto the vault:

- `perf_fee_rate: Ppm32` — immutable thereafter.
- `quote_authority: Option<Pubkey>` — if `None`, the protocol stamps
  `Vault.leader`. Rotatable post-open via `SetQuoteAuthority`.
- `allow_outside_depositors: bool` — toggleable post-open via
  `SetAllowOutsideDepositors`.

Side effect: the instruction creates a new SPL mint for outside-share
tokens (mint authority a vault-owned PDA) and records its address as
`Vault.share_mint`. The vault is otherwise initialized empty
(`base_atoms`, `quote_atoms`, `total_shares`, `leader_shares`, `hwm`,
`frozen` all zero); the leader seeds inventory with their first
`Deposit` (see **Depositor operations** below).

If the fee mint on `Registry` changes after this vault was opened,
old fees remain in the prior fee ATA and admins sweep both — the
vault itself is unaffected.

The new vault is inserted via the **Insert** operation in
**Storage layout** (O(1); reuses a freed sector when available). If
`vaults.len() == registry.max_vaults_per_market`, `OpenVault` fails
and the caller must wait for an existing vault to close.

### SetLiquidityProfile

Setup-and-reshape path. Writes the full `LiquidityProfile` — all levels
expressed in ppm/bps and slot offsets, never absolute. Called after
seeding the vault and any time the leader wants to reshape their
ladder. The instruction reads and increments `market.nonce`, writes
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
re-enter, the same leader pubkey pays `vault_open_fee` again and
starts a new vault. See **Vault → Frozen and tombstoned vaults** for
full state semantics and the comparison with `CloseVault`.

## Depositor operations

`Deposit` and `Withdraw` use the same pointer validation as leader
ix (see **Caller mechanics**), and the same instruction discriminants
for both the leader and outside depositors. The path splits
internally on `signer == vault.leader`: the leader updates
`Vault.leader_shares` directly, while outside depositors mint or burn
SPL tokens against `Vault.share_mint`. SPL mint and depositor-ATA
accounts are required for the outside path, **optional for the
leader path** — leaders can omit them.

Both `Deposit` and `Withdraw` realize the vault first — see
**Vault → Realize**.

### Deposit

Caller specifies a target `shares_out` and a max basket
`(max_base_in, max_quote_in)` for slippage protection. The basket
required at the current vault ratio is:

```
base_in  = ceil(shares_out × base_atoms  / total_shares)
quote_in = ceil(shares_out × quote_atoms / total_shares)
```

Rounding up keeps any dust on the depositor's side, preserving VPS
for existing depositors. The basket is transferred from the
depositor to the treasuries, then:

- **Leader path** (`signer == vault.leader`): increment
  `Vault.leader_shares` by `shares_out`. No SPL tokens minted.
- **Outside path** (`signer != vault.leader`): mint `shares_out`
  SPL tokens to the caller's ATA on `Vault.share_mint`. Requires
  `Vault.allow_outside_depositors == 1`.

`Vault.total_shares` is incremented in both paths.

**Skin-in-the-game check.** After update, if the caller is not the
leader and `leader_shares × 1_000_000 < min_leader_share × total_shares`,
the instruction reverts. The check uses on-vault numbers only — no
ATA load needed. See **Vault → Skin-in-the-game floor**.

**Seeding (first deposit).** If `total_shares == 0`, the vault has
never been seeded. The first depositor **must** be the leader and
must supply `base_in > 0 && quote_in > 0` — a zero leg would yield
`total_shares = 0` and re-trigger seeding on the next deposit (and
divide by zero in the pro-rata basket math). The instruction sets
`total_shares := isqrt(base_in × quote_in)`, `leader_shares :=
total_shares`, and `hwm := Q32.32(1.0)`. No SPL tokens are minted
on seeding (leader stake is non-SPL).

Deposits against frozen or tombstoned vaults are rejected.

### Withdraw

Caller specifies `shares_in` to burn. The vault delivers a pro-rata
basket:

```
base_out  = floor(shares_in × base_atoms  / total_shares)
quote_out = floor(shares_in × quote_atoms / total_shares)
```

Rounding down keeps any dust in the vault for the benefit of
remaining depositors. Then:

- **Leader path** (`signer == vault.leader`): decrement
  `Vault.leader_shares` by `shares_in`. No SPL burn.
- **Outside path** (signer holds SPL tokens on
  `Vault.share_mint`): burn `shares_in` SPL tokens from the
  caller's ATA. The burn itself gates authority — a caller with
  no balance fails at the SPL layer.

`Vault.total_shares` is decremented in both paths; the basket is
transferred from the treasuries to the caller.

If the caller is the leader against an **active** vault, the
post-burn ratio must remain at or above `min_leader_share`. The
floor is **bypassed for frozen and tombstoned vaults** — see
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
1. **Push** each `(remaining.price, remaining.size, stamp &
   !FLUSH_BIT, vault_ptr, level_idx)` tuple onto a binary heap
   allocated on the program heap, skipping levels where
   `remaining.size == 0` or `current_slot >= remaining.expires_at`.
   The heap is keyed so the next-to-pop is the best price with the
   oldest nonce on ties. For asks, "best" is lowest price — a
   min-heap on `(price, nonce)` works directly. For bids, "best"
   is highest price — use a min-heap on `(Price::MAX − price,
   nonce)` (or equivalently invert the price comparator while
   leaving nonce ascending). A naïve max-heap on `(price, nonce)`
   for bids would flip the nonce comparison and let *newer* quotes
   win on equal-price ties, violating price-time priority.
   `FLUSH_BIT` is masked off the stamp before keying so a
   just-flushed vault doesn't sort younger than a previously-flushed
   one with the same underlying nonce.
1. **Pop** from the heap and compute the fill as
   `fill = min(taker_unfilled, level.size, vault_leg)` — where
   `vault_leg` is `base_atoms` for asks or `quote_atoms` for bids —
   so the trade never debits more inventory than the vault holds.
   (A popped entry with `vault_leg == 0` yields `fill = 0`; the
   loop moves on to the next pop.) Decrement the taker's unfilled
   size by `fill`, decrement the popped level's
   `Vault.remaining.<side>[i].size` by `fill`, debit the matching
   leg of `base_atoms` / `quote_atoms` accordingly, and accrue the
   taker fee from `market.taker_fee_rate`. Continue until the taker
   is filled, the next heap top exceeds the taker's limit price, or
   the heap is drained.
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

2. **Bump `reference_price.price`.** Shifts the whole ladder up or
   down without changing offsets or sizes. Moving the reference up
   makes bids more attractive to sellers (they get more quote per
   base) and asks less attractive to buyers — net: invites selling
   to the leader, rebuilds base. One hot-path write
   (`SetReferencePrice`), asm-cost identical to a normal price update.

3. **Reshape via `SetLiquidityProfile` with asymmetric ladders.** `bids`
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

| Event                              | L         | VPS / APR | Basket quote value     |
|------------------------------------|-----------|-----------|------------------------|
| Underlying pair moves, no fills    | unchanged | flat      | up or down (direction) |
| Leader captures spread on flow     | grows     | positive  | up                     |
| Leader adversely selected          | shrinks   | negative  | down                   |

The depositor's total quote-denominated return decomposes cleanly
into **APR (leader skill) × basket price move (directional)**; the
two are separately attributable. The protocol math is oracle-free. UIs
that want to display a quote-converted total return can layer in a
price feed (Pyth, Switchboard) for display only — no on-chain
dependency.

APR can go **negative** when the leader is consistently adversely
selected. That is the same metric working in both directions, and it
is the right signal for depositors deciding whether to stay or pull
their basket.
