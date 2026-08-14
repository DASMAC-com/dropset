<!-- cspell:word clmm -->

<!-- cspell:word custodying -->

<!-- cspell:word drawdowns -->

<!-- cspell:word hyperliquid -->

<!-- cspell:word toggleable -->

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
the market's create-vault fee to call `CreateVault`). Outside depositors back
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

Vault creation is **permissionless**: any pubkey may call `CreateVault`
by paying the **market's** create-vault fee — `market.fee_config.atoms`
of `market.fee_config.mint` — to the Registry's fee ATA, keyed on
`get_associated_token_address_with_program_id` over
`(registry_pda, fee_config.mint, token_program)`. The token-program
seed is mandatory
(classic SPL Token and Token-2022 derive different ATAs for the same
mint) and is taken from the **fee mint's account `owner`** — the
caller passes the mint and its owning token program, validated
`token_program == fee_config.mint.owner` at `CreateVault`. No storage is
needed on the Registry itself. The fee
is **per market** — each `MarketHeader` carries its own `fee_config`,
seeded from `Registry.default_fee_config` at market creation and
tuned per market by an admin via `SetMarketFeeConfig`. Admins may
call `CreateVault` without paying, including on behalf of others (useful
for protocol-onboarded market makers). If a market's `fee_config.mint`
later changes, `SetMarketFeeConfig` creates the registry ATA for the
new mint going forward and prior fees stay in the old ATA; admins
sweep both.

The per-market cap on vault count (`max_vaults_per_market`) is set by
the cost to reconstruct the ephemeral order book during each take
and can be tuned across the protocol's lifecycle as CU budgets and
runtime performance evolve.

The byte-exact layout of both records is owned by
[`state/registry.rs`](../programs/dropset/src/state/registry.rs)
(`FeeConfig`, and the `RegistryHeader` + admin-`Set` slab tail that make
up `Registry`) and is canonicalized in the IDL. This section keeps only
the invariants and rationale, not the field-by-field types:

- **`FeeConfig`** pairs the fee `mint` with its owning `token_program`
  and an `atoms` amount. Carrying `token_program` alongside the mint is
  load-bearing: the registry fee ATA is derived by
  `get_associated_token_address_with_program_id` over
  `(registry_pda, fee_config.mint, token_program)`, and classic SPL Token and
  Token-2022 derive different ATAs for the same mint, so the owning
  program cannot be guessed — it is taken from the fee mint's account
  `owner` and validated `token_program == fee_config.mint.owner` at
  `CreateVault`. This fee account is distinct from a market's
  `base_treasury` / `quote_treasury`, which custody pooled trading
  inventory, not protocol fees.
- **`Registry`** holds the protocol-wide defaults stamped onto new
  markets — `default_fee_config`, `default_taker_fee`,
  `default_max_platform_fee` (default `100` bps = 1%; see **Order
  matching → Platform fee**), `default_min_leader_share` (default
  `50_000` = 5%; see **Vault →
  Skin-in-the-game floor**), and `max_vaults_per_market` (hard cap up
  to 255, enforced at `CreateVault`) — plus a live `market_count` and
  the admin allowlist (`Set<Pubkey>`). Each default only **seeds** the
  corresponding per-market value at `create_market`; admins tune
  markets and vaults downstream (`SetMarketFeeConfig`, `SetTakerFee`,
  `SetMaxPlatformFee`,
  `SetMinLeaderShare`, `FreezeVault`, `SetOutsideDepositsApproved`) and
  retune the defaults themselves — for future markets — via
  `SetRegistryDefaults` (the scalars `default_taker_fee` /
  `default_max_platform_fee` /
  `default_min_leader_share`) and `SetDefaultFeeConfig` (the ATA-bearing
  `default_fee_config`). Admins may also open vaults without paying the
  per-market create-vault fee.
  **Invariant:** `close_registry` requires `market_count == 0` — the
  only on-chain witness that no orphan markets remain, since the
  program cannot iterate all PDAs to verify by enumeration (see
  **Account lifecycle and rent reclamation**).

Notably absent: there is **no leader allowlist**. Banning a pubkey
would be trivially defeated by registering a fresh wallet, so the
protocol does not maintain one. Admin power is exercised per-vault via
`FreezeVault` (see **Leader operations**), and the non-refundable
per-market create-vault fee (`MarketHeader.fee_config`) acts as the only
material gate on fresh entry: every new wallet pays the fee again, so
spinning up replacements after a freeze has a real, repeated cost
rather than being free.

### Admin gating of market creation

Markets are **not** permissionless. The `create_market` instruction
must verify `signer ∈ registry.admins`
before allocating a `MarketHeader` and its treasuries, and on
success it increments `registry.market_count` by one (the symmetric
decrement happens in `close_market`; see **Account lifecycle and
rent reclamation → Teardown ordering**). This is the steady-state
rule — independent of the `admin-teardown` Cargo feature (which
gates *close*-side instructions); admin-gated *creation* is present
in every build. The check uses the same `AdminSet::admin_contains`
path that `add_admin` and `remove_admin` already use.

Rationale: a market commits the program to custodying two SPL
treasuries and exposing a fresh `(base_mint, quote_mint)` pair to
takers. Mistakes — wrong mint, wrong fee config — cannot be
unilaterally reversed by a leader, only by an admin via the
teardown path. Gating creation on the admin set keeps the set of
live markets bounded and curated. Vault creation inside an existing
market remains permissionless (see `CreateVault`).

## MarketHeader

The `MarketHeader` is a fixed-size record at the front of the market
account. It holds the market-wide counters and the active set of
vaults; the physical sector array sits immediately after the header
(see **Storage layout**).

The byte-exact layout is owned by
[`state/market/layout.rs`](../programs/dropset/src/state/market/layout.rs)
(`MarketHeader`) and canonicalized in the IDL. Conceptually the header
carries the market-wide `nonce`, the three DLL heads + `active_count`
that thread the vault sectors (see **Storage layout**), the
`outstanding_vault_depositors` counter, the per-market knobs
(`taker_fee`, `max_platform_fee`, `default_min_leader_share`,
`fee_config`) seeded from the
registry at creation and tunable downstream by admins, the base/quote
mints, and the two treasury accounts with their PDA bumps. The
load-bearing invariants and rationale:

- **Treasury custody invariant.**
  `base_treasury.amount >= Σ vault.base_atoms + accrued_base_fee_atoms`
  and
  `quote_treasury.amount >= Σ vault.quote_atoms + accrued_quote_fee_atoms`,
  summed across **every** vault on the market — active **and**
  tombstoned. Each `Deposit`, `Withdraw`, and fill moves atoms between a
  treasury and the caller's ATA while adjusting the matching vault's
  `base_atoms` / `quote_atoms` by the same delta — the two must stay
  aligned per instruction; a fill additionally books its taker fee to the
  accrued counter for the output leg (see **Fee model**). The treasury is
  the SPL **custody account**; its `.amount` is the market's *reserves*
  quantity. Because it sums active and tombstoned vaults, it is total
  inventory in custody, **not** matchable liquidity — and because it
  includes the accrued fee, it is not all depositor-owned either.

  The invariant is an **inequality**, and always had to be: the treasury
  is an ordinary token account, so anyone may transfer into it and no
  instruction can prevent that. Two things fill the gap — unsolicited
  transfers, and the exact-in fill residue (see **Take → Fill
  semantics**) — and both are the same thing to the protocol: atoms
  nobody has a claim on. `SweepResidual` recovers exactly that
  difference. What must never happen is the other direction: a treasury
  holding **less** than the sum of the claims against it cannot pay them
  all, which is the solvency bug this invariant exists to exclude.

- **Depositor-count witness.** `outstanding_vault_depositors` counts
  live `VaultDepositor` PDAs across every vault (active and tombstoned):
  incremented when an outside `Deposit` opens a fresh one, decremented
  when `Withdraw` (at `shares == 0`) or `force_withdraw_depositor`
  closes one — **not** on top-off. `close_market` requires it to be `0`;
  otherwise a `VaultDepositor` PDA could be orphaned against a closed
  market sector and its on-chain claim would silently zero on any
  subsequent `Withdraw` (see **Account lifecycle and rent
  reclamation**).

- **Per-market fee.** `fee_config` is seeded from
  `Registry.default_fee_config` and tunable via `SetMarketFeeConfig`;
  the fee is paid to the **Registry fee ATA** (not this market's
  treasuries) and waived for admin signers. Changing the mint makes
  `SetMarketFeeConfig` create the fresh registry ATA fees route to —
  admins sweep both.

- **Fee cap / floor seeding.** `taker_fee` is capped at ~6.55%
  (`Ppm16` max) and admin-mutable per market via `SetTakerFee`.
  `max_platform_fee` bounds the caller-declared platform fee (bps,
  `Bps16`, range-checked `<= BPS` on every write since the type is no
  bound) and is admin-mutable via `SetMaxPlatformFee` — see **Order
  matching → Platform fee**. Note the two fee knobs use different
  denominators: the taker fee is ppm, the platform-fee ceiling is bps.
  `default_min_leader_share` is stamped into each `Vault.min_leader_share`
  at `CreateVault`; mutating it affects only vaults opened afterward (see
  **Vault → Skin-in-the-game floor**).

- **Accrued protocol revenue.** `accrued_base_fee_atoms` /
  `accrued_quote_fee_atoms` are the running totals of taker fee charged on
  each leg — the summed `FillEvent.taker_fee_atoms`, hence the unit
  suffix (`taker_fee`, by contrast, is a **rate**). They are
  **authoritative**, not derived: nothing infers revenue from the gap
  between the treasury and the vault sum, which is what keeps that gap a
  checkable invariant instead of a tautology (see **Fee model**).

- **Layout changes need the markets recreated.** These two counters grew
  the header by 16 bytes, shifting every sector offset — and the header
  size is hardcoded by the quote-write kernels, the sBPF entrypoint, and
  the SDK mirrors. An account written by an earlier build still carries the
  same (per-type) `#[account]` discriminator, so it would pass every check
  and then be **decoded wrongly, silently**. There is no version byte to
  branch on, so any
  future header-size change means recreating each market rather than
  migrating it; that is free today because no live market exists (the first
  mainnet deploy and fill are both still pending), which is why the
  counters were added now rather than later.

Every quote a vault produces is identified by `MarketHeader.nonce`
at the moment of stamping — a global counter incremented on every
`SetReferencePrice`, `SetLiquidityProfile`, and taker fill. At match time,
levels at the same price are ranked by nonce: lower nonce = earlier
arrival = wins. This is the canonical CLOB **price-time priority**
rule, with the nonce standing in for "time" — slot timestamps would
be too coarse, since multiple events can land in the same slot.

### Fee model

Two distinct fees exist in the model, and they are not
interchangeable:

- **Taker fee** — **protocol revenue**. Per market, admin-set
  (`MarketHeader.taker_fee`, `SetTakerFee`), charged on every fill,
  accrued in the treasury, harvested later.
- **Platform fee** — **integrator revenue**. Caller-declared per swap and
  program-capped, paid out immediately to the integrator that routed the
  order. Not implemented yet; it is a separate future instruction-level
  change and touches none of the accounting below.

The rest of this section is the taker fee.

**Who bears it.** The taker, out of the **output** leg (base on a `Buy`,
quote on a `Sell`). The matched vault is debited its **full quoted
output** and therefore trades at exactly the price it quoted; the
treasury sends the taker `output − fee` and keeps the difference. LPs
earn from the leader's edge only, never from the protocol's fee.

**Where it lives.** Nowhere new. `market_base_treasury` /
`market_quote_treasury` are ATAs authorized by the market PDA holding
pooled inventory for every vault on the market — the fee atoms are
already physically in the treasury and never move on a fill. What the
accrued counters record is **who has a claim on them**:

```txt
treasury.amount >= Σ vault.<leg>_atoms + accrued_<leg>_fee_atoms
        ^^^^^^^          ^^^^^^^^^^^^^^^^   ^^^^^^^^^^^^^^^^
        custody          depositor claim     protocol claim
```

(The slack is atoms **nobody** claims — an unsolicited transfer or an
exact-in fill residue — which `SweepResidual` collects. See **Treasury
custody invariant** above.)

Booking the fee to the counter rather than leaving it inside the vault
is load-bearing twice over. Left in the vault it would (1) raise
depositor NAV pro-rata, making a protocol fee an LP fee in substance,
and (2) inflate `L = isqrt(base · quote)`, so `Realize` would read a
slice of protocol revenue as leader edge and mint the leader a
performance fee on it (see **Vault → High-water mark and performance
fee**).

**Rounding.** Down — the taker keeps the dust, matching the kernel's
existing `taker_fee_atoms` behavior.

**Rollback.** The `min_out` soft-revert and the nonce-overflow hard
error restore both accumulators to their pre-swap values along with
vault inventory and level sizes (see **Minimum-output guard**). A swap
that does not commit accrues nothing.

**Harvest — deferred, except at teardown.** No instruction moves accrued
fee out of a **live** market. Fees accrue safely and stay fully
recoverable, so nothing is lost by waiting on the destination decision
(an admin-supplied token account versus a fixed protocol PDA; the latter
is only worth the machinery once a token or a defined treasury policy
exists).

The one path that does pay accrued fee out is
`close_market_treasury`, which drains the treasury to a supplied
`token_recipient` immediately before closing it (see **Account lifecycle
and rent reclamation → Teardown ordering**). That is not the steady-state
harvest arriving early — it fires only on a market being destroyed, on a
feature-gated instruction absent from the final immutable build — it is
what makes teardown *possible*: with no harvest and a `SweepResidual`
that subtracts the accrued counters by design, a market that ever charged
a fee would otherwise hold a treasury balance no instruction could clear,
and an empty-account close requirement would reject it forever.

#### Residual sweep

`SweepResidual` (admin-only) pays out the **residual**:

```txt
residual = treasury.amount − Σ vault.<leg>_atoms − accrued_<leg>_fee_atoms
```

A residual is expected, not exceptional: the exact-in fill semantics
(see **Take → Fill semantics**) deliberately leave the input no level
could price here, so the bucket accrues on ordinary taker-bound swaps
rather than only on a stray transfer. What the instruction still cannot
tell you is *which* kind of atom it is holding — routine residue, or a
rounding error, a share-math slip, or a botched rollback that stranded
or leaked atoms. So it remains a weak bug alarm alongside being a
collection path, and the sharper check is the direction: the residual
must never go **negative**, i.e. the treasury must always cover the sum
of the claims against it. Because a call emits its event even when it
sweeps nothing, it doubles as an on-chain read-out of the invariant's
three terms.

*Considered and rejected:* defining the protocol fee **as** the residual
and dropping the counters. That is genuinely the more elegant design —
no counters, nothing to unwind on the soft-revert path, and donations and
dust swept for free — but it converts the checkable invariant into a
tautology: any drift *is* fee by definition. A leak of depositor
principal would be indistinguishable from revenue, harvested away, with
no test able to fail and no on-chain way to notice. The counters keep the
invariant a real check; this sweep preserves the recovery property the
residual idea was after.

That recovery property covers both legitimately non-zero cases: anyone
can transfer tokens **directly** to a treasury ATA, and an exact-in take
leaves change no level could price. Either way the atoms are otherwise
stranded forever, since no vault has a claim a `Withdraw` could pay out.

Mechanics and bounds:

- Admin-gated (`signer ∈ registry.admins`), one leg per call. The mint
  must be one of the market's two legs (`NotAMarketTreasury` otherwise) —
  a non-leg market-owned ATA has no inventory field and no counter to
  subtract, so its whole balance would read as residual.
- The vault sum runs over the **whole slab**, not just the active DLL: a
  tombstoned vault still holds depositor claims, and a reclaimed sector
  could carry rounding dust. Over-counting only ever *understates* the
  residual, erring toward leaving atoms in custody. At most 255 sectors
  (`max_vaults_per_market` is a `u8`, default 10) on a cold admin path.
- The accrued counters are **subtracted, never touched**. This is not a
  harvest.
- The subtraction **saturates at zero**. A Token-2022 mint with a
  transfer-fee extension delivers less than was sent, which can push the
  treasury *below* the claimed sum; then there is simply nothing to
  sweep, and the emitted event carries all three terms so an operator can
  see the shortfall. That extension already threatens the custody
  invariant and is not introduced by fee accrual.

### Market closure

Closing a market is feature-gated (see **Account lifecycle and rent
reclamation**) and refunds the entire `MarketHeader` + vault-slab
rent — vault sectors are inline storage and carry no separate rent,
so there is intentionally no per-vault close instruction. The slab
itself is closed as one allocation when the market closes.

`close_market` requires `signer ∈ registry.admins` and the following
on-chain pre-conditions:

- `outstanding_vault_depositors == 0` — every `VaultDepositor` PDA
  on this market has been closed (use `force_withdraw_depositor`
  to clear stragglers — see **Depositor positions and cost basis →
  Admin force-withdraw**). Counter enforcement is required because
  the program cannot iterate all `VaultDepositor` PDAs on-chain.
- `base_treasury` and `quote_treasury` are both closed — and
  `close_market_treasury` in turn requires both `vault.base_atoms == 0` /
  `vault.quote_atoms == 0` on every vault on the market **and**
  `active_count == 0`, so every depositor and leader is necessarily paid
  out first and every sector reclaimed. It checks the vaults' claim
  rather than the ATA balance, which is what lets the accrued protocol
  fee through to the drain.

Once those hold, the market's lamports are transferred to a
`rent_recipient` account and the account data is zeroed and
de-assigned from the program. `registry.market_count` is
decremented by one in the same instruction.

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

Each `Vault` carries two pointer fields (`next`, `prev`; see **Vault**) —
they thread whichever list the vault is currently on (active,
tombstone, or free). `MarketHeader`
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
- **Insert (`CreateVault`)** → pop the free list if non-empty, else
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

The byte-exact layout of the vault sector and its inline records
(`ReferencePrice`, `LiquidityProfile`, the materialized `Remaining` /
`Position`) is owned by
[`state/market/layout.rs`](../programs/dropset/src/state/market/layout.rs)
(`Vault`, `ReferencePrice`, `Remaining`, `Position`) and canonicalized
in the IDL. A vault carries the `leader` and `quote_authority`, the
`reference_price`, pooled `base_atoms` / `quote_atoms`, the share
bookkeeping (`total_shares`, `leader_shares`, `hwm`, `perf_fee_rate`,
`min_leader_share`), the `frozen` / `allow_outside_depositors` /
`outside_deposits_approved` / `tombstoned` flags, the `profile` ladder,
and the materialized `remaining`. DLL pointers (`next` / `prev`) thread
it into one of three lists (see **Storage layout**). The load-bearing
invariants and rationale:

- **Inventory backs the book; treasury holds the atoms.** `base_atoms`
  backs asks, `quote_atoms` backs bids; both are pooled across the
  leader and outside depositors. The physical balance lives in the
  market-wide treasuries under the custody invariant (see
  **MarketHeader**) — the vault field is the per-vault bookkeeping
  share of it.
- **Share-accounting invariant (I6).**
  `leader_shares + Σ VaultDepositor.shares == total_shares`. Both stakes are
  non-SPL protocol bookkeeping (see **Shares**); `leader_shares` increments on
  leader `Deposit` and on `Realize` perf-fee accrual and decrements on
  leader `Withdraw`.
- **HWM monotonicity.** `hwm` is value-per-share (`L / total_shares`)
  as Q32.32 and **never decreases** — the performance fee accrues only
  when VPS exceeds the mark (see **High-water mark and performance
  fee**). `perf_fee_rate` is set at `CreateVault` and immutable.
- **Skin-in-the-game floor.** `min_leader_share` is the value enforced
  at `Deposit` / leader `Withdraw`; stamped from
  `MarketHeader.default_min_leader_share` at `CreateVault`, admin-
  overridable per vault via `SetMinLeaderShare` (see **Skin-in-the-game
  floor**).
- **Two-key outside-deposit gate.** An outside `Deposit` requires
  **both** `allow_outside_depositors` (leader opt-in,
  `SetAllowOutsideDepositors`) **and** `outside_deposits_approved`
  (admin sign-off, `SetOutsideDepositsApproved`); a fresh vault has the
  latter off, so it cannot take outside baskets until an admin approves
  it. Flipping either flag off still lets pre-existing outside
  depositors `Withdraw`. `frozen` and `tombstoned` are covered in
  **Frozen and tombstoned vaults**.
- **`quote_authority` is always populated.** At `CreateVault` the
  caller may pass one; otherwise the protocol stamps `leader`, so the
  hot-path auth check is a single compare. Rotated via
  `SetQuoteAuthority` (leader-only).

**`ReferencePrice` — stamp encoding.** `stamp` packs `market.nonce` at
the last `SetReferencePrice` / `SetLiquidityProfile`, OR'd with
`FLUSH_BIT` (`1 << 63`) as the "flush pending" flag. Both quote-mutating
instructions arm the bit; the first taker to match this vault
materializes the ladder + inventory into `remaining` and clears it.
Takers mask off `FLUSH_BIT` before comparing the low 63 bits for
price-time priority (63 bits never wrap over the market's lifetime). The
`price` is range-checked by the taker at match time (leader prices are
**not** validated on write). The two datums are leader-supplied and
stored raw for the same reason: a stale or future datum only shortens or
lengthens the liveness of the leader's *own* levels — self-grief, not an
exploit, with match-time expiry the enforcement point — so neither is
validated on write either.

`quote_slot` and `quote_unix` are the **expiry datums**, one per domain:
the slot the quote was "as of", and the same instant in unix seconds.
Per-level effective expiry is a pair, one deadline off each datum:

```text
quote_slot + level.expiry_offset_slots
quote_unix + level.expiry_offset_secs
```

A level rests only while **both** are in the future. See **Expiry — the
dual gate** below.

**`Remaining` / `Position` — materialized levels.** Per-side arrays of
`N_LEVELS` `Position`s (absolute `price`, atom-sized `size`, and one
absolute deadline per expiry domain — `expires_at_unix` in unix seconds,
`expires_at_slot` in slots), computed from `profile` + inventory by
the first taker after a flush (see **LiquidityProfile → Flush** for the
formulas); subsequent takers read them directly and decrement `size` on
fills.

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
`(price, nonce)` heap keys, including the bid-side `Price::INFINITY − price`
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

The 32-bit width is load-bearing: it keeps `price` adjacent to the two
`u32` expiry datums on the `SetReferencePrice` hot path (three
contiguous stores) and keeps every materialized `Position.price`
compact.

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
it: `Deposit` (leader path) and the internal `realize_in_place`
accrual add; `Withdraw` (leader path) subtracts under leader
signature. No path
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

**Not a standalone instruction — an internal step.** There is no
`realize` discriminant; the program exposes no permissionless
`Realize` entrypoint. Instead `realize_in_place`
([`state/market/accrual.rs`](../programs/dropset/src/state/market/accrual.rs))
runs implicitly at the start of every `Deposit` and `Withdraw`
(including the leader paths and the feature-gated admin
force-withdraw paths), so outside flows always cross at a post-fee
VPS and never transfer leader-owed fee value to or from the caller.
**Never runs on the taker hot path.** Touches no SPL accounts — perf
fee accrual is purely on-vault bookkeeping.

A standalone permissionless `realize` — callable by an indexer or
keeper to pin HWM at an arbitrary moment between basket flows — is
**not** implemented. The leader and depositors already trigger
accrual on every `Deposit` / `Withdraw`, so HWM is pinned at each
flow without a separate entrypoint.

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
`CreateVault`; an admin can override any level downstream — the market
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
  stays on the active DLL; existing levels die off as their deadlines
  pass. Terminal — no "unfreeze".
- **Tombstoned** — leader's intended lifecycle exit. Vault is
  unlinked from active matching immediately.

Either state ends at **Reclaim** (see **Storage layout**): the
final `Withdraw` that drives `total_shares` to 0 unlinks the vault
from its current DLL, zeroes `vault.leader` and `vault.quote_authority`
so the emptiness marker holds, and pushes the sector onto the free
list. The same leader pubkey may then `CreateVault` afresh — paying
the create-vault fee again — on this or any other market.

When the `admin-teardown` Cargo feature is enabled (see **Account
lifecycle and rent reclamation**), an admin may additionally
`force_withdraw_depositor` against any `VaultDepositor` and
`force_withdraw_leader` against any vault's `leader_shares`,
regardless of the vault's state — active, frozen, or tombstoned.
Together these widen the "only the owner can move their stake"
property held by every production build (outside funds and
leader funds both), in service of being able to drain markets to
zero between testnet / early-mainnet deploy cycles without
requiring leader cooperation. The feature is absent from the final
immutable build, and with it absent the original property is
restored.

## LiquidityProfile

Each level carries a `price_offset` in **ppm** (1_000_000 = 100%)
from `reference_price.price`, a `size_bps` as fraction of vault
inventory in **basis points** (10000 = 100%), and one expiry offset
per domain — `expiry_offset_secs` after `quote_unix` and
`expiry_offset_slots` after `quote_slot`. The two scales differ on
purpose:
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

The byte-exact layout is owned by
[`state/market/layout.rs`](../programs/dropset/src/state/market/layout.rs)
(`LiquidityProfile`, `Level`) and canonicalized in the IDL: per-side
arrays of `N_LEVELS` `Level`s, each a
`(price_offset, size_bps, expiry_offset_secs, expiry_offset_slots)`
tuple, top of book first. The load-bearing invariants:

- **Per-side size cap.** `Σ size_bps ≤ 10000` per side. The sum at
  exactly `10000` fully commits that leg; a lower sum leaves a reserve.
  The invariant is enforced **at match time only**: a side whose sum
  exceeds `10000` is skipped (its levels don't materialize) rather than
  aborting the take — see **Order matching → Book construction**.
  `SetLiquidityProfile` stores an over-cap ladder without complaint.
- **Unit asymmetry.** `size_bps` is a fraction of the matching
  inventory leg — `base_atoms` for asks, `quote_atoms` for bids — so a
  materialized **bid** size is denominated in quote atoms (the leader
  allocates their quote pool across bid prices), an **ask** size in
  base atoms.
- **Implicit direction.** `price_offset` is a ppm spread from
  `reference_price.price` whose sign is implicit: bids subtract, asks
  add. The two expiry offsets are measured from their own datums. All
  three materialize to absolute values at flush (see below).

### Flush

When `SetReferencePrice` or `SetLiquidityProfile` arms the `FLUSH_BIT` on
`reference_price.stamp`, the next taker to hit this vault performs a
one-time materialization across all levels into `Vault.remaining`:

```text
// PPM = 1_000_000. Let a = asks[i], b = bids[i].
// deadline(datum, off) = 0 if off == 0 else datum +sat off

asks_remaining[i].size       = base_atoms × a.size_bps / 10000  // base
asks_remaining[i].price      = ref.price × (PPM + a.price_offset) / PPM
asks_remaining[i].expires_at_unix = deadline(ref.quote_unix,
                                             a.expiry_offset_secs)
asks_remaining[i].expires_at_slot = deadline(ref.quote_slot,
                                             a.expiry_offset_slots)

bids_remaining[i].size       = quote_atoms × b.size_bps / 10000  // quote
bids_remaining[i].price      = ref.price × (PPM −sat b.price_offset) / PPM
bids_remaining[i].expires_at_unix = deadline(ref.quote_unix,
                                             b.expiry_offset_secs)
bids_remaining[i].expires_at_slot = deadline(ref.quote_slot,
                                             b.expiry_offset_slots)
```

**Zero is dead, in either domain.** `deadline` maps a zero offset to
zero rather than to the bare datum, so "no life in this domain" survives
the addition instead of collapsing onto the datum — a leader stamping a
future datum cannot give a zero-life level a deadline still ahead of the
clock. Folding the check in at flush time (once per quote) also keeps
the taker's per-level gate a single unconditional compare per domain. A
ladder armed before any reference price is all-zero and dead by the same
encoding.

**Both datums come from the stored quote, never the live clock.** This
flush runs lazily inside the *first taker's swap* after `FLUSH_BIT`
arms, so a clock read here would be **attacker-scheduled**: in the halt
scenario the first post-restart taker is precisely the pick-off flow,
and its own transaction would refresh the very quote it is picking off.
Staleness must anchor at quote-write time. The constraint applies to
both domains, and a future refactor must not "optimize" either stamp
into the flush.

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
out at match time. The per-side `Σ size_bps ≤ 10000` invariant is
applied here at flush: a side whose sum exceeds `10000` has its
`remaining` sizes written as zero (thrown out of matching, see **Order
matching → Book construction**), so on any side that *is* materialized
the sum never exceeds the inventory leg and no runtime clamp is needed.
`FLUSH_BIT` is then cleared with one `u64` store.

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
  top-of-book short offsets and deep levels much longer ones, so flush
  cadence is graded by depth instead of forced to the top-of-book rate.
  Having both domains per level is what makes the *tight* end
  expressible: the cluster clock is second-denominated and accurate
  only to a few seconds, which floors any wall TIF at ~15 s, so a
  sub-second dead-man tail behind the quoter's latest stamp can only be
  said in slots.

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
`("vault_depositor", market, sector_idx, owner)`. The vault is
identified by its `(market, sector_idx)` rather than a single derived
vault address, because vaults are slab sectors inside the market
account, not standalone PDAs; including `sector_idx` in the seeds means
a sector recycled across vault lifetimes derives a fresh
`VaultDepositor` address. It is the authoritative on-chain record of
both the depositor's claim and what they paid for it:

The byte-exact layout is owned by
[`state/vault_depositor.rs`](../programs/dropset/src/state/vault_depositor.rs)
(`VaultDepositorHeader`) and canonicalized in the IDL. It binds
`(market, sector_idx, owner)` — all PDA-seed inputs, so the account is
**non-transferable** (no authority field to reassign) — and carries the
`shares` claim plus the cost-basis fields. The load-bearing invariants:

- **Share claim (I6 term).** `shares` is this depositor's term of the
  vault invariant `leader_shares + Σ VaultDepositor.shares == total_shares`.
- **Two principal measures.** `net_deposits` is the quote-denominated
  basis of the **remaining** position
  (`Σ (quote_in + base_in × entry_ref)` over deposits, reduced by the
  withdrawn slice on `Withdraw`) — the basis the unrealized PnL is measured
  against.
  `gross_deposited` is **monotonic** lifetime contributions, **never
  reduced on withdraw** — the stable denominator for an all-time
  return %.
- **Realized-PnL decomposition.** `realized_pnl` (signed — a withdrawal
  can realize a loss) splits as
  `realized_yield + realized_fx == realized_pnl`. Discarded when the account
  closes at zero shares.

The cost basis is a shares-weighted average over deposits:
`entry_ref_price` (a `Price`, the average reference quote-per-base) and
`entry_vps` (average VPS, `L / total_shares`, Q32.32 like `Vault.hwm`).
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

#### Admin force-withdraw (feature-gated)

The `force_withdraw_depositor` instruction lives behind the
`admin-teardown` Cargo feature (see **Account lifecycle and rent
reclamation**) and lets an admin pay a depositor out and close
their `VaultDepositor` PDA without the depositor's signature. Its
sole purpose is to make markets fully drainable for the testnet /
early-mainnet redeploy cycle; it is absent from the final
immutable build.

- **Signer.** `signer ∈ registry.admins`.
- **Vault state.** Active, frozen, or tombstoned. **Reclaimed**
  sectors (on the free DLL, `leader == Pubkey::default()`) carry
  no depositors by invariant — by the prescribed teardown ordering,
  every `VaultDepositor` is closed before the vault is allowed to
  reach Reclaimed, so this case does not arise. The instruction
  rejects Reclaimed sectors as a defense-in-depth check.
- **Effect.** Mechanically identical to outside-depositor
  `Withdraw` (see below) with two differences:
  - the signer check is `signer ∈ registry.admins` instead of
    "signer matches the `VaultDepositor` PDA seeds";
  - the payout target is the depositor's `(base_mint, quote_mint)`
    ATAs, created via `init_if_needed` if absent. The admin is
    never a possible payout target — funds always land with the
    `owner`, not the caller.
- **ATA rent.** If `init_if_needed` allocates a fresh ATA for the
  `owner`, the calling admin pays the ~2.04 mSOL ATA rent. This is
  a deliberate operational cost on the teardown wallet, not on the
  depositor — by symmetry with the rest of the teardown surface,
  the admin running the wind-down bears all the new-account rent.
- **Rent (PDA).** The closed `VaultDepositor` PDA's lamports are
  refunded to its `owner`, the same as the existing close-on-empty
  path. The depositor's PDA rent is always the depositor's, even
  when the admin initiated the close.
- **Counter.** `MarketHeader.outstanding_vault_depositors` is
  decremented by one — the same decrement the close-on-empty path
  performs.

The instruction is the same logic as `Withdraw` for the depositor
slice `shares_in = VaultDepositor.shares` — same basket math, same
realized-PnL accumulator updates, same vault `total_shares` /
`base_atoms` / `quote_atoms` decrements — just with the signer
gate widened. There is no separate accounting path.

A sibling `force_withdraw_leader` covers the leader's stake, which
lives in `vault.leader_shares` rather than on a `VaultDepositor`
PDA and is therefore not reachable via the depositor variant. Same
admin gate, same payout-to-stake-holder rule (funds always land
with `vault.leader`, never with the calling admin), and the same
admin-pays-ATA-rent rule on `init_if_needed`. Vault state: active,
frozen, or tombstoned — Reclaimed sectors are rejected (the
sector's leader pubkey has already been zeroed). Mechanically it
is the same logic as leader-path `Withdraw` for
`shares_in = vault.leader_shares` with the signer check widened to
`signer ∈ registry.admins`. Because the leader has no PDA, there
is no PDA rent to refund — the only state change is the
share/atom decrement and, once `total_shares == 0`, the vault
sector reclaims to the free DLL.

## Caller mechanics

Every instruction that targets a specific vault — leader-callable,
outside-depositor-callable, and admin — takes a `vault_idx: u32`
argument naming the target sector by its index into the market's
slab tail, avoiding any list walk. Before touching the vault, the
program performs three checks. The first two are the same for every
caller; the third is the per-ix authority gate.

1. **Bounds.** `vault_idx` is resolved through the bounds-checked
   `VaultAccess` accessor
   ([`state/market/access.rs`](../programs/dropset/src/state/market/access.rs)):
   `read_vault` / `mutate_vault` index the slab via
   `.get(vault_idx as usize)` and reject an out-of-range index with
   `InvalidSectorIndex`. Because the slab is a `[Vault]` indexed by
   element, a valid index always lands on a real vault boundary, so
   no separate pointer-alignment check is needed.
1. **Occupancy.** `vault.is_occupied()` — the `leader` field doubles
   as the free-list emptiness marker (`Address::default()` means "on
   the free list / unassigned"), so an operation against a reclaimed
   sector is rejected with `VaultEmpty` before any authority compare.
1. **Authority.** Differs by instruction:
   - **Quote-mutating** (`SetReferencePrice`, `SetLiquidityProfile`):
     `vault.quote_authority == signer` — a single compare, and the
     *only* domain check either path makes. No branching for the unset
     case (`quote_authority` is always populated; see
     `SetQuoteAuthority`), and neither path re-reads `frozen` or
     occupancy: the freeze is enforced at match time, and a write to a
     free-listed sector is **inert** (see below). Both skip the
     bounds/occupancy accessor above too — they work the market bytes
     directly and bounds-check the sector inline against
     `min(len, capacity)`, returning the same `InvalidSectorIndex`.

     Note the compare does **not** reject a free-listed sector.
     `reclaim_sector` zeroes only `leader` — the emptiness marker
     `is_occupied()` reads — so a freed sector keeps its former
     `quote_authority` until `allocate_sector` re-zeroes the whole struct
     on reuse, and that ex-authority's compare still passes. What makes it
     harmless is the blast radius: a quote write touches only
     `market.nonce`, the sector's `reference_price`, and its `profile` —
     never the `next` / `prev` links that thread the free list — and
     matching walks the active DLL only, so a free sector never enters the
     book. Advancing the nonce is not an attack either: it only pushes the
     *next* quote to a later (worse) time priority, and never reorders
     quotes already stamped.

   - **Leader-only** (`SetQuoteAuthority`,
     `SetAllowOutsideDepositors`, `CloseVault`):
     `vault.leader == signer`.

   - **Deposit**: leader path requires `vault.leader == signer`;
     otherwise outside path requires both
     `vault.allow_outside_depositors == 1` (leader opt-in) and
     `vault.outside_deposits_approved == 1` (admin approval).

   - **Withdraw**: leader path requires `vault.leader == signer`;
     otherwise outside path requires a `VaultDepositor` PDA seeded by
     `(market, sector_idx, signer)` with `shares >= shares_in` (the seeds
     bind the account to the signer, proving ownership). See
     **Vault → Frozen and tombstoned vaults** for the wind-down
     behavior on non-active vaults.

   - **Permissionless.** There is no permissionless vault-targeting
     *instruction*: perf-fee accrual (`realize_in_place`) is an
     internal step the `Deposit` / `Withdraw` handlers invoke, not a
     standalone entrypoint with its own discriminant (see
     **Vault → Realize**).

   - **Admin-only** (`FreezeVault`, `SetOutsideDepositsApproved`,
     `SetMinLeaderShare`, `SetMarketFeeConfig`):
     `signer ∈ registry.admins`.

No discriminant tag is needed: the slab tail is homogeneous, so a
bounds-checked index (1) unambiguously identifies a `Vault`, the
occupancy check (2) rejects a free-list sector, and the per-ix
authority gate (3) then runs against a known-live vault.

**Addressing: slab index, not raw pointer.** An earlier design
addressed the target vault by a raw pointer into the market
account's input-buffer region. That scheme would have required the
leader's signer account to carry **zero account data** — any
variable-size payload on it would shift downstream offsets and break
the static addressing the pointer math assumed — plus explicit
in-bounds and alignment checks on every call. It was **dropped** in
favor of the `vault_idx: u32` slab index above: indexing a `[Vault]`
is bounds-checked by the slice accessor, lands on a vault boundary
by construction (no alignment check), and needs no zero-data
precondition on the leader account.

**Zero-data signer on the two quote writes.** The one exception, and
only in the `asm-entrypoint` build: `SetReferencePrice` and
`SetLiquidityProfile` require their signer to carry `data_len == 0`,
rejecting otherwise with an asm-specific structural code. Nothing about
the *addressing* needs it — `vault_idx` still indexes the slab — but
pinning the signer's size keeps the **market's account record** at a
static input-buffer offset, so the assembly's market offsets are
assemble-time constants rather than arithmetic off a runtime `data_len`.
A keypair wallet carries no data, so every ordinary caller satisfies it;
what it does exclude is a data-carrying PDA delegated as
`quote_authority` and signing via CPI, which the reference build would
accept. That asymmetry is why the structural guards are deliberately
outside the Rust↔ASM parity contract (see **SetReferencePrice → ASM
fast path**) — they are mapped, not equated. No other instruction and no
other account has a zero-data requirement.

### Admin authority

The protocol has two authorization tiers; everything in this spec
that is not a permissionless instruction falls under one of them.

1. **Upgrade authority.** The pubkey set as the BPF Upgradeable
   Loader's upgrade authority on the program's `ProgramData`
   account. Gates `init` only — used once at program genesis to
   create the Registry, after which the upgrade authority is no
   longer consulted by any instruction.
1. **Registry admin set.** The `admins` field on the Registry,
   mutated by `add_admin` / `remove_admin`. Gates every
   instruction with a `signer ∈ registry.admins` check, listed
   here so the full set is visible in one place:
   - Steady-state: `add_admin`, `remove_admin`, `create_market`
     (see **Registry → Admin gating of market creation**),
     `FreezeVault`, `SetOutsideDepositsApproved`,
     `SetMinLeaderShare`, `SetMarketFeeConfig`, `SweepResidual`, and a
     waived `CreateVault` fee path.
   - Feature-gated under `admin-teardown` (see **Account
     lifecycle and rent reclamation**): `force_withdraw_depositor`,
     `force_withdraw_leader`, `close_market_treasury`,
     `close_market`, `close_registry_fee_vault`, `close_registry`.

The upgrade authority is intentionally narrow — exactly one
instruction — so that post-init the admin set is the sole
governance surface, and the only way to grow or shrink that
surface is `add_admin` / `remove_admin` signed by an existing
admin.

### Error surface

The behavior-defining preconditions above each map to a named
`DropsetError` variant ([`errors.rs`](../programs/dropset/src/errors.rs)).
The variants are part of the stable, IDL-surfaced ABI — clients
match on them — so the load-bearing ones are listed here against the
precondition they enforce. This is not the full enum (arithmetic and
internal-consistency codes such as `MathOverflow` and
`CorruptVaultList` are omitted); `errors.rs` and the IDL are
authoritative.

| Precondition that fails                                     | `DropsetError`                 |
| ----------------------------------------------------------- | ------------------------------ |
| `vault_idx` past the slab tail                              | `InvalidSectorIndex`           |
| Target sector is on the free list (`leader == default`)     | `VaultEmpty`                   |
| Quote-mutating ix against a frozen vault                    | `VaultFrozen`                  |
| `CloseVault` against an already-tombstoned vault            | `VaultAlreadyTombstoned`       |
| Operation disallowed against a tombstoned vault             | `VaultTombstoned`              |
| Signer is not the vault's `quote_authority` / `leader`      | `Unauthorized`                 |
| First (seeding) deposit not signed by the leader            | `SeedingRequiresLeader`        |
| Seeding deposit missing the base or quote leg               | `SeedingRequiresBothLegs`      |
| Non-seeding deposit not sizing exactly one leg              | `SingleLegRequired`            |
| Derived basket exceeds the caller's slippage bounds         | `BasketSlippage`               |
| Operation would breach the vault's `min_leader_share` floor | `MinLeaderShareViolated`       |
| Outside deposit, leader opt-in off                          | `OutsideDepositorsNotAllowed`  |
| Outside deposit, admin approval off                         | `OutsideDepositorsNotApproved` |
| Withdraw of more shares than the caller holds               | `InsufficientShares`           |
| `VaultDepositor` PDA ≠ `(market, sector, owner)` seeds      | `VaultDepositorMismatch`       |
| Reference price unset where a basis read needs it           | `ReferencePriceNotSet`         |
| Swap `limit_price` bits not a well-formed `Price`           | `InvalidPrice`                 |
| Swap `amount_in == 0`                                       | `InvalidAmountIn`              |
| Swap `side` neither Buy nor Sell                            | `InvalidSwapSide`              |
| Swap `limit_price` sentinel wrong for the side              | `InvalidLimitPrice`            |
| Close / sweep mint is not a market base/quote leg           | `NotAMarketTreasury`           |
| Teardown ix invoked with the `admin-teardown` feature off   | `TeardownDisabled`             |

## Leader operations

A leader joins a market by calling `CreateVault` to allocate a vault
sector (paying the market's create-vault fee, `market.fee_config`), then
seeding the vault with their
first `Deposit`, then `SetLiquidityProfile` to lay down their bid/ask
ladder as offsets from a reference price. From there, steady-state
activity is just `SetReferencePrice` on the hot path — sliding the
whole ladder by updating a single anchor price. `SetLiquidityProfile` can
be re-called to reshape the ladder as needed.

Authority gates and pointer validation are uniform across all
instructions in this section; see **Caller mechanics**.

### CreateVault

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
- `quote_authority: Address` — **must not be `Address::default()`**;
  the zero address is rejected with `Unauthorized`. Having no private
  key, it would quote-brick the vault, since `SetReferencePrice` /
  `SetLiquidityProfile` gate on `signer == quote_authority`. (The
  free-list emptiness marker is `Vault.leader`, not this field — see
  **Storage layout**.) A caller
  that wants no separate delegation passes the leader's own pubkey
  rather than a `None`/default sentinel. Rotatable post-open via
  `SetQuoteAuthority`.
- `allow_outside_depositors: bool` — toggleable post-open via
  `SetAllowOutsideDepositors`.

**`leader` resolution.** Every `CreateVault` takes a
`leader_override: Address` argument. A non-admin caller must pass
either `Address::default()` (use signer) or their own pubkey —
otherwise the instruction rejects with `LeaderOverrideNotAllowed`.
Admins may pass any pubkey; that pubkey is stamped as
`Vault.leader`. Passing `Address::default()` on the admin path uses
the admin signer as the leader (same as the non-admin path).

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
`vaults.len() == registry.max_vaults_per_market`, `CreateVault` fails
and the caller must wait for an existing vault to close.

### SetLiquidityProfile

Setup-and-reshape path. Writes the full `LiquidityProfile` — all levels
expressed in ppm/bps and slot offsets, never absolute. Called after
seeding the vault and any time the leader wants to reshape their
ladder.

**No write-time validation.** The profile bytes are stored raw. The one
domain guard is that the signer equals the target vault's
`quote_authority` — the same single gate `SetReferencePrice` applies,
because the two share one kernel (see **ASM fast path** below).

**Per-side collateral invariant.**

```text
Σ bids[i].size_bps ≤ 10000
Σ asks[i].size_bps ≤ 10000
```

A sum of exactly 10000 commits the full inventory leg across the
ladder; smaller sums leave an unallocated reserve. The invariant is
enforced **at match time only**: a side whose sum exceeds 10000 is
skipped during book construction (its levels don't materialize) rather
than aborting the taker's swap — see **Order matching → Book
construction**. That skip is what makes an over-cap side safe, so the
write path does not re-check it; off-chain, the SDK simulator and the
maker bot's ladder builder mirror the same sum, so an honest leader
never arms a dark side by accident.

**No reference-price pre-condition.** A ladder may be written before the
vault's first `SetReferencePrice`. The profile is purely relative (ppm
offsets from the reference price), but a price-less vault fails
`has_valid_reference_price()` and is skipped whole *before* the flush
block, so the ladder never materializes to garbage absolute prices and
`FLUSH_BIT` simply stays armed until a real price lands. The natural
lifecycle order is still open → seed via `Deposit` →
`SetReferencePrice` → `SetLiquidityProfile`; arming it out of order is
inert self-grief, not a state the protocol has to reject.

Occupancy and `frozen` are likewise not re-read: a write to a free-listed
sector is inert (it cannot touch the free-list links or re-enter the book
— see **Caller mechanics → Authority**), and re-quoting a frozen vault is
a no-op because matching skips it.

The instruction reads and increments `market.nonce`, writes
the old value (OR'd with `FLUSH_BIT`) to `reference_price.stamp`,
and leaves `reference_price.price` and `reference_price.quote_slot`
unchanged. Bumping the nonce on reshape means the new ladder takes
fresh time priority at match time; otherwise a leader could quietly
reshape into a more aggressive ladder while keeping a stale stamp
that beats fresher quotes from other vaults at the same price. The
next taker re-materializes `Vault.remaining` from the new profile
and current inventory.

**ASM fast path.** Like `SetReferencePrice`, this discriminator is
handled in the hand-written sBPF entrypoint (`src/asm/entrypoint.s`) in
the default `asm-entrypoint` build, sharing that file's preamble with
its sibling and diverging only at the payload: one `sol_memcpy_` of the
160-byte profile blob from the instruction data into `Vault.profile`,
metered at `max(10, len / 250)` compute units versus roughly 40 for a
hand-rolled chunked copy. It mirrors the solana-free
`write_liquidity_profile` kernel
(`state/market/liquidity_profile.rs`) byte-for-byte, over the shared
`quote_write` half. On litesvm the fast path costs ~59 CU versus ~324
for the Rust entrypoint — a ~82% saving, and only 12 CU more than the
two-store `SetReferencePrice` path despite writing 160 bytes.
`tests/asm_parity.rs` deploys the reference build beside it and asserts
both write the same bytes to the same offsets — including that the write
moves *nothing* outside `market.nonce`, the target sector's
`reference_price.stamp`, and its `profile`.

The fast path does **not** bound the instruction-data length before the
copy — a settled decision, not an outstanding gap, since these paths
trust the market maker to call them correctly. Surplus bytes are a
non-event: every read is a fixed width at a fixed offset — `vault_idx` at
`+1`, then the 160-byte blob at `+5`, a maximum extent of
`ix_data + 165` — so nothing scans and bytes past that are never read. A
truncated call does diverge from the reference
build (which rejects it at deserialization), but only ever harms its own
caller: under 133 bytes it faults and the caller's own transaction fails;
at 133–164 it copies the trailing program-id bytes — public data — into
the caller's own ladder and succeeds silently. Every SDK builder emits
the full 165 bytes.

Neither case can be turned into an injection. The copy length is a
constant, never payload-derived, and the destination is the fixed
`(vault + 144, 160)` window. The one attacker-controlled input to that
destination — `vault_idx` — cannot select a sector the caller does not
already own: it is bounds-checked twice (the index against the slab
length, then the vault's end against `data_len`), and the resulting
sector's `quote_authority` has been compared against the signer over all
32 bytes. So malformed data cannot touch another vault, the header beyond
the nonce, or any accounting field.

Nor can a garbled ladder brick the market for anyone else. `Level` is
all-`Pod`, so arbitrary bytes are a *valid* `LiquidityProfile` — no
invalid bit patterns, no UB — and every match-time consumer of one is a
total function: `materialize_remaining` zeroes an over-cap side out of
the book rather than aborting the take, `level_fill_atoms` falls back to
`0`, and `flush_level_price` cannot fail (saturating subtraction plus
`Price::ZERO` fallbacks). No panic and no `Err`, so no
taker is ever blocked. The blast radius is exactly one leader garbling
their own quotes, self-healing on their next valid submit — worth less
than the hot-path CU a length check would cost. Like the other
structural asymmetries, it is mapped by the parity tests, not equated.

### SetReferencePrice

Hot path. Takes
`(vault_idx: u32, price_bits: u32, quote_slot: u32, quote_unix: u32)`
from the leader over just two accounts — the signer and the market.
`price_bits` is the raw `Price` encoding; `quote_slot` and `quote_unix`
are the two expiry datums (see **Expiry — the dual gate**). All three
are written verbatim, with **no** write-time validation. The one domain
guard is that the signer equals the target vault's `quote_authority`.

Both datums are **leader-supplied rather than read from the `Clock`
sysvar**, which is what keeps this path syscall-free: a program-side
clock read would cost ~100+ CU on a handler otherwise measured in tens.
Only the taker path, which already loads `Clock`, ever reads them back.

Storing an invalid `price` or a stale datum is fund-safe, not just
"skipped": matching gates every vault on `has_valid_reference_price()`
and re-checks each level's price and expiry, so an invalid price parks
the vault out of the book and a bad slot just never matures the levels
(self-grief). Value extraction never reads the reference price at all —
the HWM / perf-fee kernel is `L = isqrt(base·quote)` over inventory, so
a bogus quote cannot move a leader's high-water mark. Crystallized-PnL
reporting reads it, but only to split *reported* realized PnL into FX vs
yield; the tokens withdrawn are a price-independent pro-rata inventory
slice. `Deposit` keeps its own `ZERO` / `INFINITY` `entry_ref` guard
independently. Dropping the write-time checks (price validity, the
sentinels, the clock sysvar and its future/backdate bounds, the
occupancy and frozen gates) therefore moves no risk while removing the
clock account and most of the hot-path CU.

Reads `market.nonce`, writes `Vault.reference_price` as the `stamp`
(`market.nonce | FLUSH_BIT`) plus the three payload `u32`s — `price`,
`quote_slot`, `quote_unix` — laid out contiguously so they land as
adjacent stores. Increments `market.nonce`. Setting `FLUSH_BIT`
arms a pending materialization of `Vault.remaining`, deferred to the
next taker — so the leader write stays at two stores regardless of
`N_LEVELS`. No vault iteration, no reallocations, no profile touch.

**ASM fast path.** Because this is the steady-state hot path, the default
build (the `asm-entrypoint` feature, on by default) handles this
discriminator in a hand-written sBPF entrypoint (`src/asm/entrypoint.s`)
that short-circuits it ahead of Anchor's dispatcher and `call`s the
dispatcher for everything but the two quote writes (`SetLiquidityProfile`
shares the same preamble there). It mirrors the solana-free
`stamp_reference_price` kernel byte-for-byte; the reference build
(feature-off, `dropset_ref.so`) runs the same kernel through the plain
Anchor entrypoint and serves as the parity oracle (`tests/asm_parity.rs`
deploys both and asserts identical stamps and domain error codes). On
litesvm the fast path costs ~49 CU versus ~260 for the Rust entrypoint —
a ~81% saving. (Adding the `quote_unix` datum cost exactly the one extra
load/store pair it predicted: ~47 → ~49.) The offsets the assembly
hardcodes are pinned against the live layout by an `offset_of!` test, so
a `layout.rs` change breaks the build rather than silently mis-stamping.

Off-chain pre-signing: because both datums are supplied by the leader
rather than read from the clock, a quote can be signed at slot N and
relayed at slot M > N, with on-chain expiry math anchored to N.

#### Expiry — the dual gate

A level rests only while it is inside **both** of its deadlines:

```text
live  ⇔  now_slot < expires_at_slot  ∧  now_unix < expires_at_unix
```

with `now_slot = Clock.slot` and `now_unix = Clock.unix_timestamp`
(clamped into `u32`: a negative sysvar value would wrap into the far
future and resurrect every expired level). Taking the **min of two
leader-supplied bounds** is never worse than either alone, and the two
answer different failure modes:

- The **wall** bound is what survives a cluster halt. Slots stop ticking
  while the cluster is down, so a slot-only ladder returns at restart
  with its full budget intact against a pre-halt price — hours of price
  movement delivered into one block, against spreads that assume price
  continuity.
- The **slot** bound is what gives a tight level a *fast* deadline,
  which the wall domain cannot express at the resolution required (see
  the ~15 s floor under **LiquidityProfile → Flush**).

**Safety.** Both datums can only shorten or lengthen the life of the
leader's *own* levels, so the write path keeps its no-validation stance
and no trust boundary moves. Note the lemma's *shape* differs from a
stored absolute expiry: because every offset is measured **from** its
datum, a far-future datum **extends** the leader's own level lives
rather than merely opting out of the protection. That is still
self-harm-only — a leader can already choose arbitrarily long offsets —
but it is the reason the argument is stated in terms of "the leader's
own quotes" rather than "can only shorten".

**Reshape extends wall life.** `SetLiquidityProfile` writes offsets
against the *existing* datums, so a reshape without a fresh price
lengthens each level's remaining life. That is the leader's own choice;
it is documented, not gated.

**Honest limits.** This is a staleness cap, not halt immunity, and the
binding domain depends on the cluster's clock:

- Under **today's** clock, `unix_timestamp` is a stake-weighted *median*
  of vote-transaction timestamps, each projected to the current slot and
  clamped to an epoch-start-anchored PoH expectation (+25% fast / −150%
  slow). It is clamped, never rejected — there is no block-level
  timestamp stamp and no consensus rejection. The clamp is also what
  allows a post-restart *jump* equal to the accumulated fast headroom,
  which is what kills a pre-halt book in block 1.
- Under a **leader-stamped** clock (the SIMD-0363 direction — closed
  stale, implemented in the Alpenglow feature branch, and *unratified*;
  SIMD-0326 removes the vote transactions today's clock is derived
  from), there is no post-restart jump: cluster time recovers an
  outage-sized deficit at roughly 2× pace. Wall-TIF halt protection
  weakens there, and the slot conjunct — slots resume ticking at
  restart — becomes the fast protection instead. That design also
  carries its own caveat: a run of byzantine or bribed consecutive
  leaders can hold the clock back by Δ, with ~Δ recovery.

The dual gate is therefore robust under **both** regimes, which is why
it is adopted rather than either single-domain design. A residual
remains for slot-*unbounded* deep tiers under a 0363-style clock (they
survive roughly half their wall TIF of post-restart chain time at
pre-halt prices); that is a tier-policy question for the ladder retune,
not a layout one. Avoid hardcoding a slot duration in either docs or bot
math — SIMD-0525 stages slots 400 → 200 ms without touching
`unix_timestamp`; prefer cluster-provided parameters.

The unconditional mitigation — bot startup / reconnect quote
invalidation — is independent of all of this and stays required.

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
the active DLL (existing levels still match until their deadlines
pass) but cannot be re-quoted. There is no "unfreeze" — to
re-enter, the same leader pubkey pays the create-vault fee again and
starts a new vault. See **Vault → Frozen and tombstoned vaults** for
full state semantics and the comparison with `CloseVault`.

### SetOutsideDepositsApproved

Admin-only. Writes `Vault.outside_deposits_approved = flag`. This is
the admin's half of the two-key gate on outside deposits: a vault
takes outside baskets only when an admin has approved it
(`outside_deposits_approved == 1`) **and** the leader has opted in
(`allow_outside_depositors == 1`). New vaults start unapproved
(`outside_deposits_approved == 0` at `CreateVault`), so an admin must
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
`CreateVault`. This is the per-vault skin-in-the-game lever: lowering it
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

Admin-only. Overwrites `MarketHeader.fee_config` (the per-`CreateVault`
fee: `mint` and `atoms`), seeded at market creation from
`Registry.default_fee_config`. Use it to retune the create-vault fee on
a single market — raise or lower the amount, or switch the fee to a
different mint — while every other market stays at its own value.

The admin passes the new mint **and its owning token program**, which
the instruction validates as `token_program == mint.owner` so the
stored pair is always a mint backed by its real, classifiable token
program (classic SPL Token or Token-2022). Both are written into
`fee_config`, and `CreateVault` pins its `fee_token_program` account to
the stored `token_program` — so this check keeps the stored pair
self-consistent, rejecting a mint/program mismatch at configuration
time. Admins should configure only mints **without** the
Token-2022 transfer-fee extension, since that extension would deliver
less than `atoms` into the registry fee ATA.

Changing the mint routes future fees to a fresh registry ATA
(`get_associated_token_address_with_program_id` over
`(registry_pda, mint, token_program)`). `CreateVault` charges into that
ATA but does **not** create it, so the instruction creates it here
eagerly — `init_if_needed`, admin as rent payer — meaning the fee
destination provably exists the moment the config is set; without it the
next `CreateVault` on the market would fail to load the missing account.
The ATA program's `InitializeAccount3` CPI also rejects a non-mint /
wrong-program payload outright, a stronger backstop than the
`mint::token_program` constraint. Fees already collected stay in the
prior ATA and admins sweep both. Takes effect on the next `CreateVault`;
vaults already open are unaffected (the fee is charged only at open
time).

### SetTakerFee

Admin-only. Writes `MarketHeader.taker_fee = value` (ppm, `Ppm16`),
overriding the rate stamped from `Registry.default_taker_fee` at
`create_market`. The taker fee is read on the swap hot path, so this is
the lever that retunes a live market's taker schedule after launch. It
is a market-wide knob — not per-vault — so it takes no `vault_idx`.
`Ppm16` is a `u16`, so the ~6.55% cap is the type's own maximum: no
value can exceed it and the instruction performs no range check. Takes
effect on the next swap; in-flight quotes are unaffected.

### SetMaxPlatformFee

Admin-only. Writes `MarketHeader.max_platform_fee = value` (bps,
`Bps16`), overriding the ceiling stamped from
`Registry.default_max_platform_fee` at `create_market`. This is the
ceiling on the caller-declared `platform_fee_bps` (see **Order matching
→ Platform fee**), read on the swap hot path, and — since the fee is
permissionless — the only thing bounding how much of a taker's output
any router may skim. Market-wide, so no `vault_idx`.

Unlike `SetTakerFee` this **does** range-check: `Bps16` is a `u16`,
which reaches 65_535 — over 6× `BPS` — so the type is no bound at all.
`value <= 10_000` is enforced, with exactly `BPS` allowed (an admin
declining to place any ceiling below 100%) and `0` allowed (platform
fees turned off on this market, every non-zero declaration rejected).
Takes effect on the next swap.

### SetRegistryDefaults

Admin-only. Retunes the registry-wide scalar defaults stamped onto
**future** markets — `default_taker_fee` (`Ppm16`),
`default_max_platform_fee` (`Bps16`), and `default_min_leader_share`
(`Ppm32`) — each passed as an `Option`, so an admin can move one default
without restating the others (`None` leaves a field untouched). Like
`SetMarketFeeConfig`, the write is **non-retroactive**: it changes only
what the next `create_market` stamps, never the values live markets were
created with. Retune those per market via `SetTakerFee` /
`SetMaxPlatformFee` / `SetMinLeaderShare`. `default_min_leader_share` is
range-checked (`<= 1_000_000` ppm, exactly `PPM` allowed for a
leader-only book), mirroring `SetMinLeaderShare`;
`default_max_platform_fee` is range-checked (`<= 10_000` bps) for the
`Bps16` reason above, so an over-range ceiling can't be stamped onto
every market created afterwards; `default_taker_fee` needs no check for
the `Ppm16` reason above.

The registry's third default, `default_fee_config`, is **not** covered
here: mutating a fee config must eagerly create the registry fee ATA for
a new mint (see `SetMarketFeeConfig`), so it lives in its own ATA-bearing
instruction — `SetDefaultFeeConfig`, below — rather than as an `Option`
field on this pure-header writer.

### SetDefaultFeeConfig

Admin-only. Overwrites `Registry.default_fee_config` (the create-vault
fee future markets inherit: `mint`, owning `token_program`, and `atoms`),
seeded once at `init`. It is the registry-level mirror of the per-market
`SetMarketFeeConfig`: same `(mint, token_program)` validation
(`token_program == mint.owner`, via the `mint::token_program` constraint),
same eager `init_if_needed` of the registry fee ATA for the new mint
(admin pays rent). The eager ATA is load-bearing: `create_market` loads
the registry fee ATA for `default_fee_config.mint` but does **not** create
it, so re-pointing the default to a fresh mint without its ATA would brick
the next `create_market` — the same hazard `SetMarketFeeConfig` guards at
the market level. As with the other registry-default levers the write is
**non-retroactive**: it changes only what the next `create_market` stamps,
never the `fee_config` of markets already created (retune those via
`SetMarketFeeConfig`). Admins should configure only mints **without** the
Token-2022 transfer-fee extension, for the reason given under
`SetMarketFeeConfig`.

### SweepResidual

Admin-only, always on (**not** teardown-gated: exact-in fill residue and
unsolicited transfers both strand atoms on a live market). Takes no
arguments; the accounts are the
admin, the registry, the market, one leg's `mint` + owning token program,
that leg's treasury ATA, and a destination token account. It transfers out
`treasury.amount − Σ vault.<leg>_atoms − accrued_<leg>_fee_atoms`, saturating at
zero, and emits `SweepResidualEvent` with all three terms even when it
sweeps nothing.

Semantics, bounds, and why the residual is routine collection rather than
only a bug alarm: **MarketHeader → Fee model → Residual sweep**. It is
deliberately **not** a fee harvest — the accrued counters are subtracted,
never touched.

## Depositor operations

`Deposit` and `Withdraw` use the same pointer validation as leader
ix (see **Caller mechanics**), and the same instruction discriminants
for both the leader and outside depositors. The path splits
internally on `signer == vault.leader`: the leader updates
`Vault.leader_shares` directly, while outside depositors update
`shares` on their `VaultDepositor` account (PDA seeded by
`("vault_depositor", market, sector_idx, owner)`; see **Depositor
positions and cost basis**). The `VaultDepositor` account is required on the
outside path — `init_if_needed` on `Deposit`, `close`-on-empty on
`Withdraw` — and **omitted on the leader path**. No SPL share mint or
ATA exists anymore; shares are pure on-vault bookkeeping on both
paths.

Both `Deposit` and `Withdraw` realize the vault first — see
**Vault → Realize**.

### Deposit

Caller sizes the deposit by **one leg** — a base amount *or* a quote
amount — and passes a max basket `(max_base_in, max_quote_in)` for
slippage protection. The args are `vault_idx: u32` (which vault on
the market) plus two scalar legs `base_in: u64, quote_in: u64` (and
`max_base_in: u64, max_quote_in: u64`):
the depositor commits the leg they hold by setting it non-zero and
leaves the other at `0` ("add 1,000 USDC" →
`quote_in = 1_000e6, base_in = 0`), and the matching leg follows from
the vault's current ratio, mirroring the linked inputs in the deposit
UI. Single-leg-ness is a
**runtime** invariant, not a type-level enum: the handler enforces
`require!((base_in > 0) ^ (quote_in > 0), SingleLegRequired)`, so
exactly one leg must be non-zero on a (non-seeding) outside deposit.
The sized leg fixes `shares_out`, and the basket is then derived from
`shares_out` at the current ratio:

```text
shares_out =
  floor(base_in  × total_shares / base_atoms)    // base_in  > 0
  floor(quote_in × total_shares / quote_atoms)   // quote_in > 0

base_in_final  = ceil(shares_out × base_atoms  / total_shares)
quote_in_final = ceil(shares_out × quote_atoms / total_shares)
```

`shares_out` is rounded **down** and the basket **up**, so the
depositor always backs their minted shares with a full pro-rata
basket; any rounding dust stays on the depositor's side (their sized
leg is an upper bound — `base_in_final ≤ base_in` when sizing by base,
and symmetrically for quote), preserving VPS for existing depositors
(invariant I1). The instruction reverts with `BasketSlippage` if
`base_in_final > max_base_in` or `quote_in_final > max_quote_in` — the
ratio moved beyond the caller's tolerance. The basket is transferred
from the depositor to the treasuries, then:

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
  **Leader operations → SetOutsideDepositsApproved**. The outside
  path also requires the vault's `ReferencePrice` to be set —
  `reference_price.price` must not be the unset sentinel (`0` or
  `u32::MAX`), else the deposit reverts with `ReferencePriceNotSet`.
  The depositor's `entry_ref_price` basis is captured from that
  price; entering against an unset reference would silently collapse
  the cost-basis math (`quote_for_base(ZERO, base) == 0`), so it is
  rejected up front. The leader path has no such gate — seeding sets
  the inventory ratio directly without recording a depositor basis.

**Instruction split.** The two paths are wired as separate
instructions in the program: `deposit_leader` / `withdraw_leader`
omit the `VaultDepositor` account entirely (the leader has no PDA),
and `deposit` / `withdraw` carry the PDA + basis tracking + close-
on-empty path for outside depositors. Each handler rejects the
opposite signer (the outside variants reject
`signer == vault.leader`, the leader variants reject any other
signer). The
outside-path PDA is closed back to the depositor on zero-share exit
and `MarketHeader.outstanding_vault_depositors` decremented, so the
spec's `close_market` invariant is reachable.

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

Caller specifies `shares_in` to burn alongside slippage bounds
`(min_base_out, min_quote_out)`. The vault delivers a pro-rata
basket:

```text
slice_base  = floor(shares_in × base_atoms  / total_shares)
slice_quote = floor(shares_in × quote_atoms / total_shares)
```

Rounding down keeps any dust in the vault for the benefit of
remaining depositors. The instruction reverts with `BasketSlippage`
if `slice_base < min_base_out` or `slice_quote < min_quote_out` — the
floored basket undershot the caller's tolerance because the vault's
ratio or `total_shares` moved (this mirrors the Deposit
`max_base_in` / `max_quote_in` guard above). Both withdraw variants
(`withdraw` and `withdraw_leader`) enforce the same two bounds.
Then:

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

The taker instruction is exposed on-chain as `swap` — that is its name in
code, the IDL, and the SDK. This document calls it "the take" as a role
name (and "the swap hot path" elsewhere); both name the same instruction.

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
   **Per-side size gate.** Before materializing, sum each side's
   `size_bps`; a side whose `Σ size_bps > 10000` is thrown out of
   matching — its `remaining` sizes are written as zero, so the collect
   step below drops every level on that side — while the other side (and
   every other vault) still matches. This is the authoritative home of
   the `Σ size_bps ≤ 10000` invariant: an over-cap side is skipped, just
   as an out-of-range reference price skips a whole vault; it does **not**
   abort the take (so one corrupt vault can't DoS every taker). The
   stored `LiquidityProfile` bytes are left untouched, so a leader's
   ladder self-heals on their next valid `SetLiquidityProfile`.

1. Iterate the relevant side of `remaining` (asks for a buy taker,
   bids for a sell taker).

1. **Collect** each live level as a
   `(price_key, price, stamp & !FLUSH_BIT, sector_idx, level_idx, size)`
   entry, pushing it onto a `Vec` allocated on the program heap and
   skipping levels where `remaining.size == 0`, either deadline has
   passed (`now_slot >= remaining.expires_at_slot` or
   `now_unix >= remaining.expires_at_unix`), or the price is a sentinel
   (`ZERO` / `INFINITY` / invalid). `price_key` is the `u32` sort key:
   `price.as_u32()` for asks (lowest price is best) and
   `price.bid_key()` for bids (which maps the highest price to the
   lowest key, so a single ascending sort serves both sides without a
   per-compare branch). `FLUSH_BIT` is masked off the stamp before it
   becomes the `nonce` key, so a just-flushed vault doesn't sort
   younger than a previously-flushed one with the same underlying
   nonce.

1. **Sort** the collected `Vec` once by
   `(price_key, nonce, sector_idx, level_idx)`: best price first, then
   oldest quote (lowest nonce) on equal-price ties, then lowest sector
   and level index as a final deterministic tiebreak. A single
   `sort_by_key` over this materialized snapshot reproduces the spec's
   cross-vault price-time priority. A binary min-heap with `pop_min`
   was the originally-researched structure (see **Implementation notes**
   below), but at `N_LEVELS = 8` the whole book is at most
   `max_vaults_per_market × 8` entries, so one sort of a flat `Vec` is
   simpler than maintaining heap order and carries no meaningful CU cost
   at this scale; the heap design is retained only as a rejected
   alternative.

1. **Walk** the sorted `Vec` front-to-back and compute each fill.
   Units depend on side:
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
   vault holds, and the debit is the **gross** output leg — the taker
   fee is not left in the vault. (An entry with `vault_leg == 0` yields
   a zero fill; the loop moves on.) Decrement the taker's unfilled
   amount, decrement the level's `Vault.remaining.<side>[i].size` by
   the fill, and add the taker fee (from `market.taker_fee`) to the
   output leg's `accrued_<leg>_fee_atoms` on the header — see **MarketHeader →
   Fee model**. Because the `Vec`
   is sorted best-price-first, the first entry whose price crosses the
   taker's limit lets the walk `break` immediately — every later entry
   crosses too. Continue until the taker is filled, a level crosses
   the limit price, or the `Vec` is exhausted.

#### Fill semantics — the take is exact-in

A take means "I put in these tokens." Both conversions above round
toward zero, so the largest whole number of output atoms a taker's
budget buys generally prices back to slightly **less** than that
budget. That change cannot be spent at any later level — every later
level is priced worse, so it converts to zero output there — and the
engine therefore **consumes it** rather than handing it back:

- Whenever the **taker's own budget** is the binding cap on a leg, the
  walk ends there and the taker's whole remaining input is transferred.
  The vault is credited only the priced input leg; the difference is
  the **residue**, bounded by the input cost of one output atom.
- Whenever something else stops the walk — thin depth, an empty vault,
  or the limit price — the unspent budget is still the taker's and is
  never transferred. A partial fill stays a partial fill.

The residue is booked to **neither** vault inventory nor an
`accrued_<leg>_fee_atoms` counter. It is not revenue and not a fee: it
is "someone sent in more than the engine could price," so it takes the
same path an unsolicited transfer does and sits in the treasury as
unattributed residual, recovered by `SweepResidual` (see **Treasury and
custody** below). Crediting the matched vault instead would hand one
leader a windfall the price it quoted did not earn, and lift depositor
NAV through `L = isqrt(base · quote)`.

The residue scales with the **output** token's granularity: at most one
invisible atom into a 6-decimal token, but up to roughly one cent of
input into a 2-decimal one, since a single output atom there costs a
whole unit of the input asset.

1. **Tear down.** The `Vec` buffer is freed with the transaction;
   debited inventory, `Vault.remaining.size` decrements, the cleared
   `FLUSH_BIT` on any flushed vault, the `accrued_<leg>_fee_atoms` increments,
   and `market.nonce` persist to
   chain. Takers bump `market.nonce` per fill but never touch
   `reference_price.stamp` beyond clearing `FLUSH_BIT`, and never
   touch `Vault.remaining.price` or either of its expiry deadlines.

### Implementation notes — heap and capacity

The ephemeral book is **heap-allocated**, not stacked: its length isn't
known until the active-DLL walk finishes, and even a worst-case buffer
would overflow the ~4 KiB SBPF stack frame. These constraints bound the
design:

- **Entry size.** Each collected entry (step 5 above) is ~32 B at
  8-byte alignment — the `price_key` (`u32`) plus the original `price`
  (kept for the fill math, so the sort reads the key and never decodes a
  `Price` twice), the masked `nonce` (`u64`), `sector_idx` / `level_idx`
  (`u32` each), and `size` (`u64`).
- **Capacity.** The book is bounded at `max_vaults_per_market × N_LEVELS`
  entries. At the default cap (10 vaults × `N_LEVELS = 8` = 80 entries ×
  ~32 B ≈ **2.5 KiB**) it sits comfortably inside the **32 KiB default
  program heap** — no `RequestHeapFrame` is needed. Only a market
  configured near the `u8` vault ceiling (255 × 8 × 32 B ≈ 64 KiB) would
  exceed the default heap and need a frame request (8 CU per extra
  32 KiB page).
- **Bump allocator, no reclaim.** The runtime's default allocator is a
  bump allocator whose `dealloc` is a no-op, so each `Vec` doubling
  permanently leaks the old buffer for the rest of the instruction —
  tolerable here only because the entry count is small and bounded. If
  the book ever grew large, pre-size with
  `Vec::with_capacity(vaults × N_LEVELS)` for one up-front allocation.
- **Tear-down is free.** The `Vec` is dropped when the VM tears down the
  entire heap region at instruction return — zero work, zero CU.

The source-grounded version of these constraints — SVM heap mechanics,
the allocator, the matching recipe, and the runtime limits that govern
emit fidelity (see **Events and emission**), each with a file:line
permalink — lives in
[`docs/research/svm-heap-emit-cpi.md`](research/svm-heap-emit-cpi.md),
which also keeps the rejected `BinaryHeap`/`pop_min` matcher as a
considered alternative.

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

### Minimum-output guard (`min_out`)

The take instruction accepts a `min_out: u64` for SDK
composability. The matcher snapshots every touched sector's
inventory + per-level `remaining.size` + `market.nonce` + both
`accrued_<leg>_fee_atoms` counters before
mutating, runs the full fill loop, then checks whether the
achievable net output — after **both** the taker fee and the
caller-declared platform fee, i.e. what actually reaches the
taker's token account — meets `min_out`. On
failure the snapshots are walked in reverse to restore exact
pre-swap state, the accumulators are rolled back to their pre-swap
values (a swap that does not commit must not leave phantom accrued
fee — see **MarketHeader → Fee model**), `FLUSH_BIT` is re-armed on
every vault the
matcher flushed during the walk, and the instruction returns
without emitting events or firing CPI transfers. No error — the
surrounding transaction survives so a bundle of instructions that
includes the swap doesn't unravel when no liquidity is available
at the caller's price.

`min_out == 0` is the legacy "any fill counts" behavior: a
zero-fill swap still soft-reverts (no events, no transfers), but
a partial fill with `total_out > 0` always commits. Frozen vaults
are skipped from the matching set entirely so a leader-initiated
freeze takes effect from the next taker instruction rather than
waiting for per-level expiry.

### Platform fee (caller-declared)

Two distinct fees come off the output leg, in a fixed order:
`fill → taker fee → platform fee`.

- **Taker fee** — protocol revenue. Per market, admin-set
  (`MarketHeader.taker_fee`, ppm), read on the hot path.
- **Platform fee** — integrator revenue. Declared per swap by the
  caller as `platform_fee_bps` (bps, not ppm), bounded by
  `MarketHeader.max_platform_fee`, and paid out immediately.

The platform fee exists so a frontend or router earns a kickback for
routing flow to Dropset. Without it the eCLOB route is the one path
that earns an integrator nothing, which inverts the incentive to
route to our own book — including for our own frontend.

**Permissionless.** Any caller may declare a fee and name any
beneficiary; the program-enforced ceiling, not an onboarding step, is
what bounds the *fee*. There is no per-integrator state and no
allowlist.

One thing the ceiling does **not** bound: the beneficiary is
caller-chosen and never signs, so naming a fresh one each swap costs
the taker another rent-exempt balance for the new fee account, and
those lamports are recoverable by whoever closes it. A hostile
transaction builder can therefore extract SOL from its own takers
outside both the ceiling and `min_out`, which are denominated in
output-leg atoms. The taker signs, so this is consent-bounded rather
than an escalation — but the ceiling is not the whole story.

**Zero new state.** The fee is transferred in the same transaction as
the fill, to the beneficiary's token account for the swap's *output*
mint (base on a Buy, quote on a Sell). Nothing accrues, so there is no
claim instruction and nothing to reconcile. (Rejected: on-chain
accrual with a later claim — it would spare integrators a pre-created
ATA per output mint, at the cost of new state, a claim instruction,
and its own invariant.)

**The fee account is created on demand.** The optional account group
is `(platform_fee_authority, platform_fee_ata)`; a swap with no
integrator passes neither and declares `0`, so the direct paths (the
TUI, the taker bot, the `sdk/rs` router adapter, and the tests) carry
no fee plumbing. When a fee *is*
declared, the handler CPIs the ATA program's `create_idempotent` with
the runtime-selected output mint — which both derives the canonical
address (rejecting any other account) and creates it when missing,
with the taker funding the ~0.002 SOL rent once per
`(beneficiary, mint)` pair. This is deliberately *unlike* DFlow, whose
`/order` rejects a request when the fee account doesn't exist.

Because the output leg is side-dependent and Anchor's account
constraints are static, there is no mint to bind the fee ATA to at
macro-expansion time — so it carries no `associated_token::*`
constraints and no `init_if_needed`. Deferring to the CPI is both the
only form that can key off the leg chosen at runtime and the stronger
check, since the ATA program is the authority on its own derivation.

**Rounding** is down, so the taker keeps the dust — matching the taker
fee. A rate whose fee rounds to zero atoms transfers nothing, creates
no account, and emits no event.

**The treasury invariant is untouched.** The two payouts — taker and
beneficiary — sum to the output leg net of the taker fee, which is
exactly what a single taker transfer paid out before the split. Note
that this is *less* than the matching loop debited from vault
inventory: the loop debits the **gross** output and books the taker fee
into `accrued_<leg>_fee_atoms`, so paying out `gross − accrued` is
precisely what keeps
`treasury.amount >= Σ vault.<leg>_atoms + accrued_<leg>_fee_atoms`
holding with no new slack on the output leg. (The exact-in residue is
an *input*-leg effect and is unaffected by how the output is split.)
The platform fee only *splits* that same outbound transfer
between two destinations; it moves no vault state and accrues none of
its own, which is why it needs no `min_out` rollback entry — it is
computed after that gate, from no vault state.

Worth stating plainly: this fee is charged to the taker, not to the
LPs. The vault trades at exactly the price it quoted and books the
same inventory either way; what the taker gives up is what the
integrator gains.

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

**Emission points.** Whether an instruction emits follows from what it
does, not from a hand-kept roster — the `emit_cpi!` call sites in
[`lib.rs`](../programs/dropset/src/lib.rs) are the source of truth, and
each instruction's own spec section above records whether it emits. The
governing principle: an instruction emits exactly when it changes
economically-material state that an off-chain indexer **cannot**
reconstruct from end-of-slot coalesced account diffs. That is every
token-moving or share-changing flow (`CreateVault`, the deposit and
withdraw family — leader and force-withdraw variants included, each
paired with a `Realize` when it crystallizes fees; the force-withdraw
pair only on `admin-teardown` builds, inert on the immutable deploy),
the per-leg `FillEvent` that is the take's only event, the
vault-lifecycle changes (`CloseVault`, `FreezeVault`), the admin
retuning levers (`SetMinLeaderShare`, `SetMarketFeeConfig`,
`SetTakerFee`, `SetRegistryDefaults`), and `SweepResidual` — which emits
on **every** call, including the zero-sweep case, because the three terms
it reports are a diagnosis an account diff cannot supply (see
**MarketHeader → Fee model → Residual sweep**). Everything whose entire
effect is already
recoverable from a single account diff emits **nothing**: the leader
quote-refresh pair (`SetReferencePrice`, `SetLiquidityProfile`) on the
hot path, the vault-config setters (`SetQuoteAuthority`,
`SetAllowOutsideDepositors`, `SetOutsideDepositsApproved`), the registry
and market bootstrap
(`Init`, `AddAdmin`, `RemoveAdmin`, `CreateMarket`), and the
rent-returning `close_*` teardown instructions.

**Per-emit cost.** Each `emit_cpi!` runs as a self-CPI: ~1000 CU
invocation overhead + `data_len/250` CU for the payload. The hard
ceiling is **64 inner instructions per transaction**
(`MAX_INSTRUCTION_TRACE_LENGTH`), not bytes. With per-leg emit (one
`emit_cpi!` per matched leg; see **Granularity** below), this count —
not the per-CPI 10 KiB data cap — is what bounds a take: it can record
at most `64 − (top-level ix + token CPIs)` legs in one transaction (a
single `FillEvent` is ~208 B, nowhere near the data cap).

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

**Granularity — per-leg emit, every leg recorded.** A single take can
sweep many levels across many vaults (the sorted-`Vec` fill loop in
**Order matching**). The emit model is **resolved: per-leg emit** —
every matched `(sector_idx, level_idx)` leg is its own `emit_cpi!`
`FillEvent`, accumulated by the matcher and dispatched one at a time in
heap-pop (match) order, so a leg's inner-instruction index is a stable
ordinal. There is **no separate take-level event**: per-take figures
(total fill, average price, total fee) are derived off-chain by grouping
the legs of one transaction. An aggregated `FillBatch` was considered
and rejected — per-leg emit is the simplest fixed-size record, each
`FillEvent` sits far inside the per-CPI 10 KiB data cap so a leg never
splits, and the only ceiling is the 64-instruction trace count above
(not event size). Because fills ride as inner-instruction data rather
than logs, no leg is ever silently truncated; a sweep that would exceed
the trace-count ceiling fails deterministically rather than dropping a
leg.

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
`CreateVault`, `Realize`) use the default `#[event]` — they benefit from
dynamic fields and emit too rarely for bytemuck to pay back.

**Schema source of truth.** This section specifies the emit *mechanism*;
the event *schema* (the field-by-field layouts) is owned by the
program's `#[event]` structs and **canonicalized in the generated IDL**,
which off-chain clients are generated from and the self-CPI instruction
data decodes against. Default-mode events
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

## Account lifecycle and rent reclamation

For live mainnet testing the program must support full teardown:
every account it ever created can be closed and its rent reclaimed,
so the program can be upgrade-redeployed against the same program
id between cycles. The teardown instructions described here are
**feature-gated** behind a Cargo feature `admin-teardown`, compiled
into testnet / early-mainnet builds and omitted from the final
immutable deploy. Authorization while the feature is enabled is the
existing Registry admin set (see **Caller mechanics → Admin
authority**); a reader of the final-build IDL will find these
instructions absent.

### Rent-holding accounts

This is the canonical inventory of every account dropset can create
that holds rent, and the close path for each. "Holds rent" means
SOL lamports that come back on close — accounts inlined into a
parent (vault sectors inside a market slab) do not hold separate
rent and close with their parent.

| #   | Account                                                                        | Owner program          | Holds rent?                                  | Close path                                                                                                                                                                                                                                                                                                           | Rent recipient                            |
| --- | ------------------------------------------------------------------------------ | ---------------------- | -------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------- |
| 1   | **Registry** PDA, seeds `[b"registry"]`                                        | dropset                | yes (variable: 8 + header + admin slab tail) | `close_registry` (admin, feature-gated). Pre-condition: `market_count == 0`, zero admins beyond the caller, fee vault closed.                                                                                                                                                                                        | passed-in `rent_recipient`                |
| 2   | **Registry fee vault** ATA, `ata(registry, fee_mint, tp)`                      | SPL Token / Token-2022 | yes (~165–170 B)                             | `close_registry_fee_vault` (admin, feature-gated). Pre-condition: no live markets (`market_count == 0`). Collected fees are drained to a passed-in `token_recipient` first, then CPI `CloseAccount` signed by Registry PDA.                                                                                          | passed-in `rent_recipient`                |
| 3   | **Market** PDA (`MarketHeader` + vault slab inline)                            | dropset                | yes (large; grows with vault count)          | `close_market` (admin, feature-gated). Pre-conditions: `outstanding_vault_depositors == 0`, both treasuries closed.                                                                                                                                                                                                  | passed-in `rent_recipient`                |
| 4   | **Vault** sectors inside the market slab                                       | n/a (inline)           | **no separate rent** — covered by Market's   | closed implicitly by `close_market`; there is no per-vault close instruction. Reclaim (to the free DLL) does not refund any rent.                                                                                                                                                                                    | n/a                                       |
| 5   | **Base treasury** ATA, derived from market PDA                                 | SPL Token / Token-2022 | yes                                          | `close_market_treasury` (admin, feature-gated). Pre-conditions: no vault claims the leg (`Σ vault.base_atoms == 0`) **and** no vault is live (`active_count == 0`). Remaining balance — accrued fee, unsolicited transfers — drained to a passed-in `token_recipient`, then CPI `CloseAccount` signed by market PDA. | passed-in `rent_recipient`                |
| 6   | **Quote treasury** ATA, derived from market PDA                                | SPL Token / Token-2022 | yes                                          | same instruction, run for the quote leg.                                                                                                                                                                                                                                                                             | passed-in `rent_recipient`                |
| 7   | **VaultDepositor** PDA, seeds `("vault_depositor", market, sector_idx, owner)` | dropset                | yes                                          | (a) existing close-on-empty in `Withdraw` when `shares == 0`; (b) `force_withdraw_depositor` (see **Depositor positions and cost basis → Admin force-withdraw**). Either path decrements `MarketHeader.outstanding_vault_depositors`.                                                                                | depositor `owner` — their PDA, their rent |

Everything outside this table (`system_program`, `token_program`,
`ProgramData`, the program executable itself) is not program state
and is out of scope for teardown.

### Teardown ordering

Teardown follows a dependency order — each step's pre-condition is
satisfied by the prior step. Skipping ahead errors out by the
pre-conditions listed in the table above rather than producing an
inconsistent state.

Per market, in order:

1. **Force-withdraw every `VaultDepositor`.** Admin runs
   `force_withdraw_depositor` against each depositor on each vault
   in the market. Each call pays the depositor, closes their PDA
   (rent to the depositor), and decrements
   `MarketHeader.outstanding_vault_depositors`. The market is
   ready for step 4 only when this counter reads zero.

1. **Force-withdraw every leader.** Admin runs
   `force_withdraw_leader` against each vault. The leader's stake
   lives in `vault.leader_shares` (no separate PDA), so this is the
   only way to drain the vault to zero without leader cooperation.
   Each call pays the leader's slice to the leader's
   `(base_mint, quote_mint)` ATAs and zeros `leader_shares`.
   With outside shares
   already zero from step 1, `total_shares` hits zero and the vault
   sector reclaims to the free DLL — but that is just an in-slab
   pointer move, not a separate rent refund. After this step the
   treasury invariants in **MarketHeader** guarantee
   `base_treasury.amount >= accrued_base_fee_atoms` and
   `quote_treasury.amount >= accrued_quote_fee_atoms` — equal on a
   market that neither charged a taker fee nor accumulated any
   unattributed residual, and above it by whatever residual the
   preceding `SweepResidual` runs did not collect. Step 3's
   drain-on-close carries that surplus out regardless, which is why
   teardown does not require the market to be swept clean first.

1. **Close both treasuries.** `close_market_treasury` for the
   base leg, again for the quote leg. Each call requires **two**
   pre-conditions, transfers whatever the treasury still holds to a
   passed-in `token_recipient`, zeros that leg's
   `accrued_<leg>_fee_atoms`, and then closes the ATA:

   - **No vault claims the leg** (`Σ vault.<leg>_atoms == 0`) — so
     depositor principal can never be routed to `token_recipient`.
   - **No vault is live** (`active_count == 0`) — the witness that the
     market is actually at end of life, which step 2 establishes by
     reclaiming every sector.

   The second is not redundant. A leg's claim reaches zero during
   ordinary trading — a vault bought out of its base entirely sits at
   `Σ base_atoms == 0` while still quoting — so the claim check alone
   would let an admin harvest that leg's accrued fees and destroy the ATA
   under a live market, bricking the leg (nothing re-creates a treasury
   for a market that already exists).

   Draining rather than demanding an empty account is what makes this
   step reachable at all. Two balances legitimately survive steps 1–2 and
   no other instruction can move either: the leg's
   `accrued_<leg>_fee_atoms` (the harvest is deferred, and
   `SweepResidual` subtracts the counter rather than paying it out) and
   any **unsolicited transfer** to the ATA — which, since anyone can send
   one, would otherwise be a teardown griefing vector.

1. **Close the market.** `close_market` reclaims the entire
   `MarketHeader` + vault slab rent in one shot, and decrements
   `registry.market_count` by one.

Repeat 1–4 for every market on the registry. Then, once every
market is gone:

1. **Close fee vault(s).** `close_registry_fee_vault` per fee ATA. Each
   call requires `market_count == 0` — the step ordering above already
   establishes it — then drains the vault's collected market-creation
   fees to a passed-in `token_recipient` and closes it. The drain is the
   same shape as the treasuries', and for the same reason: **no**
   instruction moves tokens out of a registry fee ATA, so a single
   collected fee under an empty-account rule would strand the balance and
   block the close (and with it the redeploy) permanently.

   The market-count gate is what draining costs. It replaces the
   empty-account requirement as the handler's state pre-condition, and it
   is load-bearing in both directions: the balance a live registry holds
   is fee revenue this instruction now pays out, and `create_vault` and
   `create_market` both take the fee ATA as a plain constrained account,
   so destroying it under a live market breaks vault creation outright.

   If a market's `fee_config.mint` or the registry's
   `default_fee_config.mint` changed during the program's life, more than
   one fee ATA may exist; close each. The set of historical fee mints is
   **not enumerated on-chain** — the admin maintains it off-chain from
   `SetMarketFeeConfig` and `SetDefaultFeeConfig` events (both create a
   registry fee ATA for the new mint).

1. **Remove all but one admin.** Existing `remove_admin` enforces
   "never empty" — `close_registry` is the only path that drops
   the last admin.

1. **Close the registry.** `close_registry` requires
   `registry.market_count == 0` (witness that step 4 ran for
   every market), zeros the slab, and refunds the Registry PDA's
   lamports to `rent_recipient`.

After the last step the program has zero on-chain state and the upgrade
authority can redeploy a fresh binary at the same program id.

### Bootstrap tolerates pre-existing treasury ATAs

Three accounts the bootstrap path creates are associated token
accounts: the registry fee vault (`ata(registry, fee_mint, tp)`) and
a market's two treasuries (`ata(market, base_mint, tp)` and the quote
leg). All three are created with `init_if_needed`, not `init`, and
adopting a pre-existing account there is deliberate.

ATAs are **permissionlessly creatable by anyone**, and each of these
addresses is a pure function of seeds the program publishes — the
registry is the fixed `[b"registry"]`, and a market PDA is derived
from `(base_mint, quote_mint)`. So every one of them is computable
*before the account exists*, and a stranger can create it for the
cost of rent. Under a plain `init` constraint that permanently
bricked the instruction: an announced-but-uncreated pair could never
be opened, and after a teardown the re-`init` above became a race a
griefer wins — defeating the redeploy-at-the-same-id workflow this
whole section exists to support.

Adoption is safe because the ATA address itself commits to
`(mint, authority, token_program)`. An account at that address is
either the canonical ATA owned by the expected PDA, or it fails the
derivation check — there is no variant where the program adopts an
account a third party still controls. The single-shot guarantees are
unaffected: they rest on the **Registry** and **Market** PDAs, which
are program-owned and so creatable only by this program signing
their seeds. A second `init` or a duplicate `create_market` is still
rejected.

The one behavioral difference is that an adopted ATA may arrive
holding a balance, since a squatter can pre-fund it. Those atoms are
credited to no vault and to no fee accrual, and the recovery path
differs by leg:

- a **market treasury**'s balance is unclaimed residual, recovered by
  `sweep_residual` while the market is live and by
  `close_market_treasury`'s drain at teardown;
- the **registry fee vault**'s balance is *not* reachable by
  `sweep_residual`, which is market-scoped — it pins
  `associated_token::authority = market` and rejects any mint that is
  not one of that market's legs. It rides along with the collected
  create-vault fees and leaves via `close_registry_fee_vault`, on the
  `admin-teardown` surface, which counts it in the `collected` total
  that close reports.

### Feature gating

Every instruction introduced above (`close_registry`,
`close_registry_fee_vault`, `close_market`, `close_market_treasury`,
`force_withdraw_depositor`, `force_withdraw_leader`) lives behind
`#[cfg(feature = "admin-teardown")]`. The feature is enabled for
testnet and early-mainnet builds — when redeploy-from-scratch is a
live operational tool — and disabled for the final immutable build,
at which point the only way to remove a market is the steady-state
leader-driven `CloseVault` / drain path and the protocol's custody
guarantee returns to "no admin can pull a depositor's funds."

With the feature absent there is correspondingly no admin path to
close the Registry. This is the intended final state: with the
upgrade authority revoked (so `init` is unreachable) and the
teardown surface gone (so `close_registry` is unreachable), the
program becomes immutable and its on-chain state — Registry,
markets, and any open vaults — is permanent. The
`market_count` and `outstanding_vault_depositors` counters
introduced above are kept in the production build anyway:
removing them would create a layout fork between builds and would
buy nothing, since with the close-side instructions gone they
simply read but never gate.

Admin-gated **market creation** (see **Registry → Admin gating of
market creation**) is *not* part of this feature — it is steady-state
behavior and present in every build, and `registry.market_count` is
incremented in every build.
