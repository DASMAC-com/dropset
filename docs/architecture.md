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

Each vault is operated by a single **leader** — the pubkey that paid the
`vault_open_fee` to call `OpenVault`. Outside depositors can back the leader's
quotes with paired (base, quote) baskets, earning a pro-rata share of the
spread the leader captures. The leader remains the active manager — they
alone set quotes — while depositors are passive participants. A
skin-in-the-game floor and per-share high-water-mark performance fee align
incentives. Vault mechanics are detailed in the next section.

## Vault

A **vault** holds a leader's pooled inventory (their own capital plus outside
depositor contributions), their `BookProfile` (bids and asks as offsets from
a single reference price), and a `ReferencePrice` they update on the hot
path. Vaults live contiguously inside a shared market account (see
`MarketHeader` below).

The leader is the vault's active manager — they alone set the `BookProfile`
and `ReferencePrice` (or their delegated `quote_authority`). Outside
depositors are passive participants who share in the spread the leader
captures. Outside shares are SPL tokens held in standard ATAs (see
**Shares** below); the leader's stake is non-SPL bookkeeping. Neither
imposes any per-depositor storage on the vault sector itself.

Leader-supplied prices are **not** validated on write — takers range-check
at match time, so a nonsense reference price just renders that vault
unmatchable.

Every quote gets a unique, monotonically increasing identifier drawn from
`market.nonce` — a global counter incremented on every `SetReferencePrice`
(and every taker fill). At match time, quotes at the same price are ranked
by nonce: lower nonce = earlier arrival = wins. This is the canonical CLOB
**price-time priority** rule, with the nonce standing in for "time" — slot
timestamps would be too coarse, since multiple quotes can land in the same
slot.

```rust
struct Vault {
    leader: Pubkey,
    /// Authority for quote-mutating ix (`SetReferencePrice`,
    /// `SetBookProfile`). Always populated — at `OpenVault` time
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
    /// inventory across the leader and outside depositors.
    base_atoms: u64,
    /// Quote tokens (atoms) backing this vault's bids. Pooled
    /// inventory across the leader and outside depositors.
    quote_atoms: u64,
    /// Total vault shares outstanding (= leader_shares +
    /// share_mint.supply).
    total_shares: u64,
    /// Leader's stake. Non-SPL, protocol-tracked. Increments on
    /// leader `Deposit` and on `Realize` perf-fee accrual;
    /// decrements on leader `Withdraw`. Used to enforce
    /// `leader_shares / total_shares >= registry.min_leader_share`
    /// at `Deposit` and leader `Withdraw` (against active vaults).
    leader_shares: u64,
    /// High-water mark of value-per-share (`L / total_shares`),
    /// stored as Q32.32 fixed-point. Never decreases —
    /// performance fee accrues only when VPS exceeds this mark.
    hwm: u64,
    /// Performance fee rate the leader charges on profits above
    /// HWM, in basis points (10000 = 100%). Set at `OpenVault`
    /// time and capped by `registry.max_perf_fee_rate`.
    perf_fee_rate: u16,
    /// Set to 1 when an admin freezes the vault. Frozen vaults
    /// reject all quote-mutating instructions; depositors and the
    /// leader can still `Withdraw` (the `min_leader_share` floor
    /// is bypassed for the leader so they can exit alongside
    /// everyone else). See `FreezeVault`.
    frozen: u8,
    /// Set to 1 if outside depositors are permitted. When 0, only
    /// the leader may `Deposit`; existing outside SPL share holders
    /// (from before the flag was flipped) can still `Withdraw`.
    /// Mutable by the leader via `SetAllowOutsideDepositors`.
    allow_outside_depositors: u8,
    /// Next vault in the active DLL, or next free sector when
    /// this vault is on the free list.
    next: *mut Vault,
    /// Previous vault in the active DLL (unused on the free list).
    prev: *mut Vault,
    /// Bids and asks parameterized in basis points: a `price_offset`
    /// in hundredths of a bp from `reference_price.price`, a
    /// `size_bps` as fraction of inventory, and an `expiry_offset`
    /// in slots after `quote_slot`. See `BookProfile`.
    profile: BookProfile,
    /// Materialized per-level state — absolute price, atom-sized
    /// allowance, and absolute expiry — computed from `profile`
    /// and the current inventory by the first taker to match this
    /// vault after a `SetReferencePrice` or `SetBookProfile` (see
    /// `reference_price.stamp`). Subsequent takers read these
    /// values directly and decrement `size` on fills.
    remaining: Remaining,
}

struct ReferencePrice {
    /// `market.nonce` stamped at the last `SetReferencePrice`,
    /// OR'd with `FLUSH_BIT` (`1 << 63`) as a "flush pending"
    /// flag. Low 63 bits break ties for price-time priority at
    /// match time — takers mask off `FLUSH_BIT` before
    /// comparing. The first taker to match this vault materializes
    /// `BookProfile` (price offsets and size_bps) and inventory
    /// into `Vault.remaining` and clears the flag.
    stamp: u64,
    /// Reference price the leader's book profile is anchored to.
    /// Custom 32-bit representation; range-checked by the taker
    /// at match time.
    price: Price,
    /// Slot the quote was "as of" — supplied by the leader, validated
    /// `<= current_slot` (no future-dating) and `>= current_slot − MAX_BACKDATE`.
    /// Per-level effective expiry is `quote_slot + level.expiry_offset`.
    quote_slot: u32,
}

struct Remaining {
    bids: [LevelLive; N_LEVELS],
    asks: [LevelLive; N_LEVELS],
}

struct LevelLive {
    /// Absolute price for this level, materialized at flush from
    /// `ref_price.price × (1_000_000 ∓ level.price_offset) / 1_000_000`
    /// (subtract for bids, add for asks).
    price: Price,
    /// Live allowance in atoms (base for asks, quote for bids).
    /// Materialized at flush from `inventory × level.size_bps / 10000`;
    /// decremented on fills.
    size: u64,
    /// Absolute slot this level expires at, materialized at flush
    /// from `ref_price.quote_slot + level.expiry_offset`. Takers
    /// skip levels where `current_slot >= expires_at`.
    expires_at: u32,
}
```

### Value-per-share and the L invariant

Vault value is tracked via a dimensionless invariant borrowed from
constant-product AMMs:

```
L = isqrt(base_atoms × quote_atoms)
```

Three properties make this the right metric for an actively-quoted
two-asset vault:

- **No oracle, no numeraire.** L lives in units of √(base × quote);
  it is only ever compared against itself at the same vault.
- **Deposits and withdrawals at the current ratio leave `L / total_shares`
  invariant.** Both legs scale proportionally, so value-per-share (VPS)
  does not tick on basket flows.
- **Fills move L meaningfully.** Buying base cheap (favorable fill)
  grows L; selling base cheap (adverse selection) shrinks it. VPS
  captures both spread captured and adverse-selection PnL in a single
  number, independent of any directional move in the underlying pair.

### High-water mark and performance fee

`Vault.hwm` is the highest VPS the vault has ever reached, stored as a
Q32.32 fixed-point `u64`. It **never decreases** — prior losses must
be fully recovered (VPS back above HWM) before the leader earns again.

Performance fee accrues to the leader as **newly-minted shares**, not
token withdrawals: no forced liquidation, auto-compounding. The shares
land in `Vault.leader_shares` (the leader's stake is non-SPL, so no
SPL minting is involved). On `Realize`, if `VPS_new > hwm`:

- Existing depositors retain `(1 − f) × (VPS_new − hwm)` per share
  of the excess.
- The leader accrues `m` shares to `leader_shares`, capturing
  `f × (VPS_new − hwm)` per existing share, where:

```
m = f × s × (L − hwm × s) / ((1 − f) × L + f × hwm × s)
```

`s` is `total_shares` before the mint; `f` is the vault's
`perf_fee_rate` (basis points / 10000); `L` is the vault's current
value. After accrual, `total_shares` and `leader_shares` both grow
by `m`, and `hwm := L / (s + m)`.

`Vault.perf_fee_rate` is set at `OpenVault` time, capped by
`registry.max_perf_fee_rate`, and immutable thereafter.

### Skin-in-the-game floor

`Registry.min_leader_share` (basis points; default 500 = 5%) is a hard
floor on the leader's stake in their own vault, enforced at the two
natural choke points:

- **Deposit.** A `Deposit` is rejected if accepting it would push
  `leader_shares / total_shares` below `min_leader_share`.
- **Leader withdrawal.** A leader `Withdraw` against an active vault
  is rejected if it would push the ratio below `min_leader_share`.

Neither `SetReferencePrice` nor the taker hot path is touched. The
check uses on-vault numbers only (`leader_shares` and `total_shares`)
— no SPL mint or ATA load required, and the leader cannot evade the
floor by transferring shares to an alt wallet (their stake is
non-SPL by construction).

The deposit gate creates a clean implicit cap on outside capital:
once the vault reaches `leader_shares / min_leader_share`, new outside
deposits fail until the leader tops up. With a 5% floor, that caps
outside capital at 19× the leader's stake.

The floor is **bypassed for leader withdrawals from frozen vaults** —
frozen vaults are winding down, and the leader is treated as any other
depositor on exit.

### APR / yield accounting

Headline vault APR is **annualized VPS growth**: pure spread accrual,
by construction independent of directional moves in the underlying
pair. A price move with no trading leaves token counts (and therefore
L and VPS) unchanged; only spread capture or adverse selection move
VPS.

| Event                              | L         | VPS / APR | Basket USD-equivalent  |
|------------------------------------|-----------|-----------|------------------------|
| Underlying pair moves, no fills    | unchanged | flat      | up or down (direction) |
| Leader captures spread on flow     | grows     | positive  | up                     |
| Leader adversely selected          | shrinks   | negative  | down                   |

The depositor's total USD-equivalent return decomposes cleanly into
**APR (MM skill) × basket FX move (directional)**; the two are
separately attributable. The protocol math is oracle-free. UIs that
want to show "USD-equivalent total return" can layer in a price feed
(Pyth, Switchboard) for display only — no on-chain dependency.

APR can go **negative** when the leader is consistently adversely
selected. That is the same metric working in both directions, and it
is the right signal for depositors deciding whether to stay or pull
their basket.

## BookProfile

Every level is parameterized in **basis points**: a `price_offset`
in hundredths of a bp (100 = 1 bp) from `reference_price.price`, a
`size_bps` as fraction of vault inventory (10000 = 100%), and an
`expiry_offset` in slots after `quote_slot`. Direction is implicit
from which array the level lives in: bids subtract the offset from
the reference, asks add it.

Nothing in `BookProfile` is in absolute atoms or absolute slots —
the materialization to atoms and absolute slots happens at flush
time (see below). This lets a maker reshape the ladder once via
`SetBookProfile` and then leave it alone: as inventory drifts with
fills, subsequent flushes auto-rescale the level sizes to the
current `(base_atoms, quote_atoms)` without any further input from
the maker.

```rust
struct BookProfile {
    /// Bid levels, top of book first.
    bids: [Level; N_LEVELS],
    /// Ask levels, top of book first.
    asks: [Level; N_LEVELS],
}

struct Level {
    /// Spread from `reference_price.price`, in hundredths of a bp
    /// (100 = 1 bp). Direction is implicit: subtract for bids,
    /// add for asks. Materialized to an absolute price at flush.
    price_offset: u32,
    /// Per-refresh allowance as a fraction of the corresponding
    /// inventory leg, in basis points (10000 = 100%). Materialized
    /// to atoms at flush from `inventory × size_bps / 10000`.
    size_bps: u16,
    /// Per-level TTL in slots after `quote_slot`. Materialized to
    /// an absolute slot at flush. Takers skip levels where
    /// `current_slot >= expires_at`.
    expiry_offset: u32,
}
```

### Flush

When `SetReferencePrice` or `SetBookProfile` arms the `FLUSH_BIT` on
`reference_price.stamp`, the next taker to hit this vault performs a
one-time materialization across all levels into `Vault.remaining`:

```
asks_remaining[i].size       = base_atoms  × asks[i].size_bps / 10000
asks_remaining[i].price      = ref.price   × (1_000_000 + asks[i].price_offset) / 1_000_000
asks_remaining[i].expires_at = ref.quote_slot + asks[i].expiry_offset

bids_remaining[i].size       = quote_atoms × bids[i].size_bps / 10000
bids_remaining[i].price      = ref.price   × (1_000_000 − bids[i].price_offset) / 1_000_000
bids_remaining[i].expires_at = ref.quote_slot + bids[i].expiry_offset
```

A u128 intermediate is used during multiplication to avoid u64
overflow (relevant for both the price and size computations); the
result is truncated back to the native field width. `FLUSH_BIT` is
then cleared with one `u64` store.

Properties:

- **Per-refresh allowance is preserved.** Once `size` decrements to
  zero at level `i`, that level is dead until the next refresh —
  even if inventory remains. This caps per-refresh drainage and
  prevents takers from chain-draining a stale top-of-book across
  successive instructions.
- **Inventory snapshot is automatic.** The maker doesn't manage
  absolute sizes; the percentages bind to whatever inventory exists
  at flush. After heavy buying drains base, the next flush
  automatically rescales the ladder to the new (smaller) base leg.
- **Per-level expiry stratifies the ladder.** A maker can give
  top-of-book a short `expiry_offset` (e.g., a few seconds in slots)
  and deep levels a much longer one, so refresh cadence can be
  graded by depth instead of forced to the top-of-book rate.

## MarketHeader

The `MarketHeader` is a fixed-size record at the front of the market account:

```rust
struct MarketHeader {
    /// Market-wide monotonic counter. Stamped onto the vault on every
    /// `SetReferencePrice`; also advanced on every taker fill. A `u64`, wide
    /// enough to never wrap over the market's lifetime.
    nonce: u64,
    /// Head of the active-vault doubly linked list, or null if empty.
    head: *mut Vault,
    /// Head of the free-vault list, or null if none to reuse.
    free_head: *mut Vault,
    /// Taker fee rate. `FeeRate` is a `u16` in hundredths of a basis point
    /// (100 units = 1 bp), capping the fee at ~6.55%. Mutable by an admin.
    taker_fee_rate: FeeRate,

    // Pubkeys and bumps.
    base_mint: Pubkey,
    quote_mint: Pubkey,
    base_treasury: Pubkey,
    quote_treasury: Pubkey,
    bump: u8,
    base_treasury_bump: u8,
    quote_treasury_bump: u8,
}
```

The market account's data begins with a `MarketHeader` followed by a
contiguous array of fixed-size `Vault` sectors. Vaults are allocated on
demand: when a leader calls `OpenVault`, the account is `realloc`'d by
`size_of::<Vault>()` (or a sector is pulled off the free list if one is
available). Market creation only pays rent for the header.

`MarketHeader` stores absolute SVM pointers (`head`, `free_head`) into
the vault region that remain valid across transactions (see below for
input buffer details).

Contiguous memory layout — one slab, grown by `realloc` only:

```txt
+----------------+----------+----------+----------+----------+-----+
| MarketHeader   | Sector 0 | Sector 1 | Sector 2 | Sector 3 | ... |
+----------------+----------+----------+----------+----------+-----+
```

Two logical lists are threaded through the same sectors via each
vault's `next`/`prev` pointers. Active vaults form a doubly linked
list; vacated sectors form a singly linked free list. Example state
after opening Vaults 0–3 and then closing Vault 1:

```txt
  MarketHeader
  +---------------+
  | head      ----+---> Vault 3 <-> Vault 2 <-> Vault 0 -> null
  | free_head ----+---> Vault 1 -> null
  +---------------+
```

New vaults are prepended at `head` (so `Vault 3` — the most recent
open — sits at the front). `free_head` points at the most recently
vacated sector; the free list is singly linked via `next` and ignores
`prev`. Both lists are mutated only on vault open/close — the hot
path (`SetReferencePrice`) never touches list pointers.

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
(detailed below) and can be tuned across the protocol's lifecycle as
CU budgets and runtime performance evolve.

```rust
struct FeeConfig {
    /// Mint accepted for this fee.
    mint: Pubkey,
    /// Amount in atoms of `mint`.
    atoms: u64,
}

struct Registry {
    /// Hard cap on how many vaults any one market may allocate
    /// (up to 255). Enforced at `OpenVault` time on the Grow path.
    max_vaults_per_market: u8,
    /// Taker fee rate stamped into `MarketHeader.taker_fee_rate`
    /// at market creation. Admins may change a market's fee
    /// later; this field only sets the initial value.
    default_taker_fee_rate: FeeRate,
    /// Minimum fraction of vault shares the leader must hold,
    /// in basis points (10000 = 100%). Enforced at `Deposit` and
    /// leader `Withdraw` against active vaults. Default 500 = 5%.
    min_leader_share: u16,
    /// Cap on `Vault.perf_fee_rate` enforced at `OpenVault` time,
    /// in basis points (10000 = 100%). Default 3000 = 30%.
    max_perf_fee_rate: u16,
    /// Fee paid to the protocol treasury on `OpenVault`. Waived
    /// when the signer is an admin.
    vault_open_fee: FeeConfig,
    /// Admins authorized to mutate the `Registry`, change
    /// per-market `taker_fee_rate`, call `FreezeVault`, and
    /// open vaults without paying `vault_open_fee`.
    admins: [Pubkey; N_ADMINS],
}
```

Notably absent: there is **no leader allowlist**. Banning a pubkey
would be trivially defeated by registering a fresh wallet, so the
protocol does not maintain one. Admin power is exercised per-vault via
`FreezeVault` (see **Leader operations**), and the `vault_open_fee`
acts as the only material gate on fresh entry.

## Shares

Outside depositors hold their stake as **SPL tokens** minted from
`Vault.share_mint` (one dedicated mint per vault, address recorded on
the Vault, mint authority a vault-owned PDA). Tokens live in standard
ATAs and are transferable, composable with the rest of the Solana
ecosystem (collateral in lending markets, listed on aggregators, etc.).

The leader's stake is **not** an SPL token. It's tracked as
`Vault.leader_shares`, a protocol-controlled `u64` that increments on
leader `Deposit` and on `Realize` perf-fee accrual, and decrements on
leader `Withdraw`. This guarantees the skin-in-the-game floor cannot
be evaded by transferring shares to an alt wallet, and avoids any
need to load the leader's ATA at check time.

Invariant: `leader_shares + share_mint.supply == total_shares`.

## Leader operations

A leader joins a market by calling `OpenVault` to allocate a vault
sector (paying `vault_open_fee`), then seeding the vault with their
first `Deposit`, then `SetBookProfile` to lay down their bid/ask
ladder as offsets from a reference price. From there, steady-state
activity is just `SetReferencePrice` on the hot path — sliding the
whole ladder by updating a single anchor price. `SetBookProfile` can
be re-called to reshape the ladder as needed.

### Authority & pointer validation

Leader instructions pass a pointer into the market account's data
region pointing directly at their vault, avoiding any list walk.
Before mutating the vault, the program performs three checks:

1. **Bounds.** `ptr` lies within the market account's data region
   after the header and before the end of allocated data
   (i.e. `vaults_start <= ptr < account_data_end`, where
   `vaults_start = account_data_base + size_of::<MarketHeader>()`).
1. **Alignment.** `(ptr - vaults_start) % size_of::<Vault>() == 0` —
   guarantees the pointer lands on a real vault boundary, so the
   cast to `&mut Vault` is well-formed.
1. **Authority.** `vault.quote_authority == signer && !vault.frozen`
   for quote-mutating ix (`SetReferencePrice`, `SetBookProfile`) —
   single compare on the hot path, no branching for the unset case.
   Inventory ix (`Deposit`, `Withdraw`, `Realize`,
   `SetQuoteAuthority`, `SetAllowOutsideDepositors`) require
   `vault.leader == signer`. Leader `Withdraw` remains available on
   frozen vaults so the leader can wind down alongside depositors.

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

### OpenVault

Called by anyone to allocate a vault sector and become its leader.
The caller transfers `registry.vault_open_fee.atoms` of the fee mint
to the Registry's fee ATA — `get_associated_token_address(registry_pda,
vault_open_fee.mint)` — unless the signer is an admin (fee waived;
admins may also pass a separate `leader: Pubkey` argument to open a
vault on someone else's behalf — that pubkey becomes `Vault.leader`).

Caller arguments stamped onto the vault:

- `perf_fee_rate: u16` — capped at `registry.max_perf_fee_rate`,
  immutable thereafter.
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

Two sector-allocation paths, tried in order:

1. **Reuse.** If `free_head != null`, pop that sector and initialize
   it.
1. **Grow.** Otherwise, if the current number of allocated sectors
   is below `registry.max_vaults_per_market`, `realloc` the account
   by `size_of::<Vault>()` and use the new tail sector. The caller
   pays the rent delta. If the cap is already reached, `OpenVault`
   fails and the caller must wait for a free sector.

In both cases, the sector is prepended at `head` — O(1), no list
walk.

### SetBookProfile

Setup path. Writes the full `BookProfile` — all levels expressed in
bps and slot offsets, never absolute. Called after seeding the vault
and any time the leader wants to reshape their ladder. Also sets
`FLUSH_BIT` on `reference_price.stamp` with a single `u64` store, so
the next taker re-materializes `Vault.remaining` from the new profile
and current inventory.

### SetReferencePrice

Hot path. Takes `(price: Price, quote_slot: u32)` from the leader.
`quote_slot` is validated:

- `quote_slot <= current_slot` — no future-dating (which would
  extend effective TTL artificially).
- `current_slot - quote_slot <= MAX_BACKDATE` — sanity cap (e.g.,
  50 slots). Backdating only shortens effective TTL, so this is
  self-grief rather than an exploit, but worth bounding.

Reads `market.nonce`, writes `Vault.reference_price` as two aligned
`u64` stores: one for `market.nonce | FLUSH_BIT` as `stamp`, one
packing `(price, quote_slot)`. Increments `market.nonce`. Setting
`FLUSH_BIT` arms a pending materialization of `Vault.remaining`,
deferred to the next taker — so the leader write stays at two stores
regardless of `N_LEVELS`. No vault iteration, no reallocations, no
profile touch — asm-optimized, analogous to a propAMM
reference-price update.

Authority is `vault.quote_authority`, not `vault.leader`. This is a
single compare on the hot path; the unset-authority case is avoided
by guaranteeing `quote_authority` is always populated (default copied
from `leader` at `OpenVault`).

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
existing SPL share holders, who can continue to `Withdraw` normally.

### FreezeVault

Admin-only. Sets `Vault.frozen = 1` on the target vault, after which
`SetReferencePrice` and `SetBookProfile` against that vault are
rejected at the authority check. Existing quotes expire naturally as
each level's `expires_at` passes; takers skip them via the existing
per-level expiry check (no taker-side change needed).

Frozen vaults remain usable by depositors: `Withdraw` still works,
and `min_leader_share` is bypassed for the leader so they can exit
alongside everyone else. Once `total_shares == 0`, the vault sector
returns to the free list per the usual close path.

`FreezeVault` is the protocol's revocation lever against a
misbehaving leader. There is no separate "unfreeze" — frozen is
terminal for that vault. If the same leader pubkey wants to
re-enter, they pay `vault_open_fee` again and start a new vault.

## Depositor operations

`Deposit`, `Withdraw`, and `Realize` use the same bounds/alignment
pointer validation as leader ix (minus the leader-signer check), and
the same instruction discriminants for both the leader and outside
depositors. The path splits internally on `signer == vault.leader`:
the leader updates `Vault.leader_shares` directly, while outside
depositors mint or burn SPL tokens against `Vault.share_mint`. SPL
mint and depositor-ATA accounts are required for the outside path,
**optional for the leader path** — leaders can omit them.

Every `Deposit` and `Withdraw` begins by **realizing** the vault:
if `VPS_now > hwm`, accrue the leader's perf-fee shares to
`leader_shares` and reset `hwm`. This guarantees outside shares mint
or burn at a post-fee VPS, so flows never transfer leader-owed fee
value to or from the caller.

### Deposit

Caller specifies a target `shares_out` and a max basket
`(max_base_in, max_quote_in)` for slippage protection. The basket
required at the current vault ratio is:

```
base_in  = ceil(shares_out × base_atoms  / total_shares)
quote_in = ceil(shares_out × quote_atoms / total_shares)
```

Rounding up keeps any dust on the depositor's side, preserving VPS
for existing shareholders. The basket is transferred from the
depositor to the treasuries, then:

- **Leader path** (`signer == vault.leader`): increment
  `Vault.leader_shares` by `shares_out`. No SPL tokens minted.
- **Outside path** (`signer != vault.leader`): mint `shares_out`
  SPL tokens to the caller's ATA on `Vault.share_mint`. Requires
  `Vault.allow_outside_depositors == 1`.

`Vault.total_shares` is incremented in both paths.

**Skin-in-the-game check.** After update, if the caller is not the
leader and `leader_shares × 10000 < min_leader_share × total_shares`,
the instruction reverts. The check uses on-vault numbers only — no
ATA load needed.

**Seeding (first deposit).** If `total_shares == 0`, the vault has
never been seeded. The first depositor **must** be the leader; the
instruction sets `total_shares := isqrt(base_in × quote_in)`,
`leader_shares := total_shares`, and `hwm := Q32.32(1.0)`. No SPL
tokens are minted on seeding (leader stake is non-SPL).

Deposits against frozen vaults are rejected.

### Withdraw

Caller specifies `shares_in` to burn. The vault delivers a pro-rata
basket:

```
base_out  = floor(shares_in × base_atoms  / total_shares)
quote_out = floor(shares_in × quote_atoms / total_shares)
```

Rounding down keeps any dust in the vault for the benefit of
remaining shareholders. Then:

- **Leader path** (`signer == vault.leader`): decrement
  `Vault.leader_shares` by `shares_in`. No SPL burn.
- **Outside path** (`signer != vault.leader`): burn `shares_in`
  SPL tokens from the caller's ATA on `Vault.share_mint`.

`Vault.total_shares` is decremented in both paths; the basket is
transferred from the treasuries to the caller.

If the caller is the leader against an **active** vault, the
post-burn ratio must remain at or above `min_leader_share`. This
check is **bypassed for frozen vaults**, allowing the leader to wind
down alongside depositors.

### Realize

Folds VPS gains above `hwm` into the leader's stake — increments
`Vault.leader_shares` (and `Vault.total_shares` by the same amount)
and updates `hwm := L / total_shares_after`. Permissionless (the
leader has the strongest incentive), and runs implicitly at the start
of every `Deposit` and `Withdraw`. **Never runs on the taker hot
path.** Touches no SPL accounts — perf fee accrual is purely on-vault
bookkeeping.

## Order matching

There is no persistent order book account. Each take builds a fresh
**ephemeral book** on the SVM program heap, uses it to fill the
taker, and discards it when the instruction returns. Levels are
read from `Vault.remaining`, where prices, sizes, and per-level
expiries are already materialized — the matching engine does no bps
arithmetic at match time.

### Book construction

On every taker instruction:

1. **Walk** the active-vault doubly linked list from `head`.
1. **Range-check** the vault's `reference_price.price`. Drop the
   vault entirely if out-of-range (this is the deferred validation
   from the leader's hot path — a nonsense price renders the vault
   unmatchable here).
1. **Flush if armed.** If `FLUSH_BIT` is set on
   `reference_price.stamp`, materialize `Vault.remaining` from
   `BookProfile` and current inventory per the formulas in the
   **BookProfile → Flush** section, and clear the bit with one
   `u64` store.
1. Iterate the relevant side of `remaining` (asks for a buy taker,
   bids for a sell taker).
1. **Push** each `(remaining.price, remaining.size, stamp &
   !FLUSH_BIT, vault_ptr, level_idx)` tuple onto a binary heap
   allocated on the program heap, skipping levels where
   `remaining.size == 0` or `current_slot >= remaining.expires_at`.
   The heap is keyed by `(price, nonce)`: min-heap for asks,
   max-heap for bids. Nonce breaks price ties (older = wins) —
   `FLUSH_BIT` is masked off the stamp so a just-flushed vault
   doesn't sort younger than a previously-flushed one with the same
   underlying counter value.
1. **Pop** from the heap and fill the taker: decrement the taker's
   remaining size, decrement the popped level's
   `Vault.remaining.<side>[i].size`, debit the vault's `base_atoms`
   / `quote_atoms`, and accrue the taker fee from
   `market.taker_fee_rate`. Continue until the taker is filled, the
   next heap top exceeds the taker's limit price, or the heap is
   drained.
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
opportunity — any taker can profit from it — which gives leaders a
standing incentive to keep their reference prices honest without the
matching engine needing a leader-vs-leader pre-pass.
