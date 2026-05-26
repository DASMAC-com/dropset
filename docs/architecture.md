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
and `ReferencePrice`. Outside depositors are passive participants who share
in the spread the leader captures. Per-depositor records live in separate
`Position` accounts (see below), so the vault sector's fixed size is
unaffected by depositor count.

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
    /// Packed `(stamp, price, expiry)`. Hot path —
    /// overwritten as two aligned u64 stores.
    reference_price: ReferencePrice,
    /// Base tokens (atoms) backing this vault's asks. Pooled
    /// inventory across the leader and outside depositors.
    base_atoms: u64,
    /// Quote tokens (atoms) backing this vault's bids. Pooled
    /// inventory across the leader and outside depositors.
    quote_atoms: u64,
    /// Total vault shares outstanding.
    total_shares: u64,
    /// Shares held by the leader themselves. Used to enforce
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
    /// reject all leader quote-mutating instructions; depositors
    /// can still `Withdraw` (the `min_leader_share` floor is
    /// bypassed for the leader so they can exit alongside
    /// everyone else). See `FreezeVault`.
    frozen: u8,
    /// Next vault in the active DLL, or next free sector when
    /// this vault is on the free list.
    next: *mut Vault,
    /// Previous vault in the active DLL (unused on the free list).
    prev: *mut Vault,
    /// Bids and asks expressed as offsets from
    /// `reference_price.price`.
    profile: BookProfile,
    /// Per-level fill allowance in base atoms, mirroring
    /// `profile`'s `(bids, asks)` shape. Flushed from `profile`
    /// sizes by the first taker to match this vault after a
    /// `SetReferencePrice` — see `reference_price.stamp`.
    remaining: Remaining,
}

struct ReferencePrice {
    /// `market.nonce` stamped at the last `SetReferencePrice`,
    /// OR'd with `FLUSH_BIT` (`1 << 63`) as a "flush pending"
    /// flag. Low 63 bits break ties for price-time priority at
    /// match time — takers mask off `FLUSH_BIT` before
    /// comparing. The first taker to match this vault copies
    /// `BookProfile` sizes into `Vault.remaining` and clears the
    /// flag.
    stamp: u64,
    /// Reference price for this leader's book profile.
    /// Custom 32-bit representation; range-checked by the taker
    /// at match time.
    price: Price,
    /// Slot after which this reference price is no longer valid
    /// (low 32 bits of `Clock::slot`, supplied by the leader).
    /// Expired quotes are skipped by takers at match time.
    expiry: u32,
}

struct Remaining {
    /// Live bid-side allowance, refilled from
    /// `BookProfile.bids[i].size` on flush.
    bids: [u64; N_LEVELS],
    /// Live ask-side allowance, refilled from
    /// `BookProfile.asks[i].size` on flush.
    asks: [u64; N_LEVELS],
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
token withdrawals: no forced liquidation, auto-compounding. On
crystallization, if `VPS_new > hwm`:

- Existing depositors retain `(1 − f) × (VPS_new − hwm)` per share
  of the excess.
- The leader is minted `m` shares capturing `f × (VPS_new − hwm)` per
  existing share, where:

```
m = f × s × (L − hwm × s) / ((1 − f) × L + f × hwm × s)
```

`s` is `total_shares` before the mint; `f` is the vault's
`perf_fee_rate` (basis points / 10000); `L` is the vault's current
value. After minting, `hwm := L / (s + m)`.

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
deposit gate creates a clean implicit cap on outside capital: once the
vault reaches `leader_shares / min_leader_share`, new outside deposits
fail until the leader tops up. With a 5% floor, that caps outside
capital at 19× the leader's stake.

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

Bids and asks are stored as **offsets from the reference price**, not
absolute prices, and materialized at match time by adding each offset to
`reference_price.price`. This keeps the onchain representation compatible
with standard batch-replace APIs — a leader's usual bid/ask ladder
translates directly into a `BookProfile` by subtracting each level's
absolute price from the reference.

Each level's `size` is a **per-refresh allowance**, not a standing
quantity. Live availability is tracked in `Vault.remaining`, which the
first post-refresh taker refills from `BookProfile` sizes (triggered
by the `FLUSH_BIT` on `reference_price.stamp`). A single refresh can
therefore be hit for at most `size` per level no matter how many
separate takes arrive before the leader next calls `SetReferencePrice`.

```rust
struct BookProfile {
    /// Bid levels, top of book first.
    bids: [Level; N_LEVELS],
    /// Ask levels, top of book first.
    asks: [Level; N_LEVELS],
}

struct Level {
    /// Unsigned offset from reference price, as a custom 32-bit
    /// decimal representation. Direction is implicit: subtract
    /// for bids, add for asks.
    offset: PriceOffset,
    /// Fill allowance at this level in base atoms, reset per
    /// `SetReferencePrice` refresh. Live per-level availability
    /// is tracked in `Vault.remaining`.
    size: u64,
}
```

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
by paying `vault_open_fee` to the protocol treasury. Admins may call
the same instruction without paying — including on behalf of others
(useful for protocol-onboarded market makers). The per-market cap on
vault count (`max_vaults_per_market`) is set by the cost to reconstruct
the ephemeral order book during each take (detailed below) and can be
tuned across the protocol's lifecycle as CU budgets and runtime
performance evolve.

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

## Position

A `Position` is a per-(vault, depositor) PDA holding the depositor's
share balance. Allocated on the depositor's first `Deposit`, closed
when `shares` drops to zero.

```rust
struct Position {
    /// Vault this position is against.
    vault: Pubkey,
    /// Depositor who owns this position.
    owner: Pubkey,
    /// Shares held in `vault`.
    shares: u64,
}
```

Seeds: `[b"position", vault.as_ref(), owner.as_ref()]`.

Per-depositor records living in separate accounts (rather than inside
the vault sector) preserves the fixed-size, contiguous sector layout
described under `MarketHeader`. Depositor count is therefore not
bounded by per-vault storage.

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
1. **Authority.** `vault.leader == signer && !vault.frozen` for
   quote-mutating ix (`SetReferencePrice`, `SetBookProfile`);
   `vault.leader == signer` (frozen bit ignored) for `Withdraw`,
   which remains available on frozen vaults so the leader can wind
   down alongside depositors.

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
The caller pays `registry.vault_open_fee` to the protocol treasury
PDA unless the signer is an admin (in which case the fee is waived;
admins may also pass a separate `leader: Pubkey` argument to open a
vault on someone else's behalf — that pubkey becomes
`Vault.leader`). The caller passes their desired `perf_fee_rate`
(capped at `registry.max_perf_fee_rate`), which is stamped onto the
vault and immutable thereafter. The vault is initialized empty
(`base_atoms`, `quote_atoms`, `total_shares`, `leader_shares`, `hwm`,
`frozen` all zero); the leader seeds inventory with their first
`Deposit` (see **Depositor operations** below).

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

Setup path. Writes the full `BookProfile` — all orders are
expressed relative to a single reference price, so the profile
itself is price-agnostic. Called after seeding the vault and any
time the leader wants to reshape their book. Also sets `FLUSH_BIT`
on `reference_price.stamp` with a single `u64` store, so the next
taker copies the new sizes into `Vault.remaining` instead of reusing
stale per-level allowances from the old profile.

### SetReferencePrice

Hot path. Reads `market.nonce`, writes `Vault.reference_price`
(two aligned `u64` stores: one for `market.nonce | FLUSH_BIT`
as `stamp`, one packing `(price, expiry)`), and increments
`market.nonce`. Setting `FLUSH_BIT` on `stamp` arms a pending
refill of `Vault.remaining`, deferred to the next taker — so the
leader write stays at two stores regardless of `N_LEVELS`. No
vault iteration, no reallocations, no profile touch — asm-optimized,
analogous to a propAMM reference-price update.

### FreezeVault

Admin-only. Sets `Vault.frozen = 1` on the target vault, after which
`SetReferencePrice` and `SetBookProfile` against that vault are
rejected at the authority check. Existing quotes expire naturally
via the TIF on `reference_price.expiry`; takers skip the vault via
the existing expiry check (no taker-side change needed).

Frozen vaults remain usable by depositors: `Withdraw` still works,
and `min_leader_share` is bypassed for the leader so they can exit
alongside everyone else. Once `total_shares == 0`, the vault sector
returns to the free list per the usual close path.

`FreezeVault` is the protocol's revocation lever against a
misbehaving leader. There is no separate "unfreeze" — frozen is
terminal for that vault. If the same leader pubkey wants to
re-enter, they pay `vault_open_fee` again and start a new vault.

## Depositor operations

Depositor instructions pass a pointer to the vault (validated by the
same bounds/alignment scheme described under **Authority & pointer
validation**, minus the leader-signer check) and the caller's
`Position` PDA.

Every depositor instruction begins by **crystallizing** the vault:
if `VPS_now > hwm`, mint the leader's accrued perf-fee shares and
reset `hwm`. This guarantees shares mint or burn at a post-fee VPS,
so flows never transfer leader-owed fee value to or from the caller.

### Deposit

Caller specifies a target `shares_out` and a max basket
`(max_base_in, max_quote_in)` for slippage protection. The
instruction computes the basket required at the current vault ratio:

```
base_in  = ceil(shares_out × base_atoms  / total_shares)
quote_in = ceil(shares_out × quote_atoms / total_shares)
```

Rounding up keeps any dust on the depositor's side, preserving VPS
for existing shareholders. The basket is transferred from the
depositor to the treasuries; `Position.shares` and
`Vault.total_shares` are incremented (and `Vault.leader_shares` too,
if the depositor is the leader).

**Skin-in-the-game check.** After mint, if the caller is not the
leader and `leader_shares × 10000 < min_leader_share × total_shares`,
the instruction reverts.

**Seeding (first deposit).** If `total_shares == 0`, the vault has
never been seeded. The first depositor **must** be the leader; the
instruction sets `total_shares := isqrt(base_in × quote_in)`,
`leader_shares := total_shares`, and `hwm := Q32.32(1.0)`.

Deposits against frozen vaults are rejected.

### Withdraw

Caller specifies `shares_in` to burn. The vault delivers a pro-rata
basket:

```
base_out  = floor(shares_in × base_atoms  / total_shares)
quote_out = floor(shares_in × quote_atoms / total_shares)
```

Rounding down keeps any dust in the vault for the benefit of
remaining shareholders. `Position.shares` and `Vault.total_shares`
are decremented (and `Vault.leader_shares` too, if the withdrawer is
the leader); the basket is transferred from the treasuries to the
caller.

If the caller is the leader against an **active** vault, the
post-burn ratio must remain at or above `min_leader_share`. This
check is **bypassed for frozen vaults**, allowing the leader to wind
down alongside depositors. `Position` accounts are closed when
`shares` drops to zero.

### Crystallize

Folds VPS gains above `hwm` into newly-minted leader shares and
resets `hwm`. Permissionless (the leader has the strongest
incentive), and runs implicitly at the start of every `Deposit` and
`Withdraw`. **Never runs on the taker hot path.**

## Order matching

There is no persistent order book account. Each take builds a fresh
**ephemeral book** on the SVM program heap, uses it to fill the taker,
and discards it when the instruction returns. Orders are materialized
just-in-time from each vault's `(reference_price.price, profile)` —
each level's absolute price is `reference_price.price + level.offset`
(subtract for bids, add for asks). Leaders only pay for cheap
reference-price updates between takes.

### Book construction

On every taker instruction:

1. **Walk** the active-vault doubly linked list from `head`.
1. **Range-check** the vault's `reference_price.price`. Drop the
   vault entirely if out-of-range (this is the deferred
   validation from the leader's hot path — a nonsense price
   renders the vault unmatchable here).
1. **Flush if armed.** If `FLUSH_BIT` is set on
   `reference_price.stamp`, copy `BookProfile` sizes into
   `Vault.remaining` and clear the bit with one `u64` store.
1. Iterate the relevant side of `profile` (asks for a buy
   taker, bids for a sell taker), adding `level.offset` to
   `reference_price.price` to get each absolute price.
1. **Push** each
   `(price, remaining.<side>[i], stamp & !FLUSH_BIT, vault_ptr, level_idx)`
   tuple onto a binary heap allocated on the program heap,
   skipping levels where the side's `remaining` is `0`. The heap is keyed
   by `(price, nonce)`: min-heap for asks, max-heap for bids.
   Nonce breaks price ties (older = wins) — `FLUSH_BIT` is
   masked off the stamp so a just-flushed vault doesn't sort
   younger than a previously-flushed one with the same
   underlying counter value.
1. **Pop** from the heap and fill the taker: decrement the taker's
   remaining size, decrement the popped level's
   `Vault.remaining.<side>[i]`, debit the vault's `base_atoms` /
   `quote_atoms`, and accrue the taker fee from
   `market.taker_fee_rate`. Continue until the taker is filled, the
   next heap top exceeds the taker's limit price, or the heap is
   drained.
1. **Tear down.** The heap buffer is freed with the transaction;
   debited inventory, `Vault.remaining` decrements, the cleared
   `FLUSH_BIT` on any flushed vault, and `market.nonce` persist
   to chain. Takers bump `market.nonce` per fill but never touch
   `reference_price.stamp` beyond clearing `FLUSH_BIT`.

### Crossed leader quotes

The protocol **does not** auto-match leaders against each other. If
Leader A's ask drifts below Leader B's bid (e.g. because A just
`SetReferencePrice`'d without observing B), nothing happens on chain
until the next taker arrives. A crossed book is an arbitrage
opportunity — any taker can profit from it — which gives leaders a
standing incentive to keep their reference prices honest without the
matching engine needing a leader-vs-leader pre-pass.
