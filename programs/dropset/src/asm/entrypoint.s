# cspell:word DISCRIM
# cspell:word lddw
# cspell:word ldxb
# cspell:word ldxdw
# cspell:word ldxw
# cspell:word stxdw
# cspell:word stxw
# Hybrid sBPF entrypoint for the dropset program.
#
# Short-circuits the two quote-write discriminators — `set_reference_price`
# (5) and `set_liquidity_profile` (6) — and writes the target vault inline,
# then exits; every other discriminator is forwarded to `__anchor_dispatch`
# (the dispatcher `#[program]` emits under the crate's `no-entrypoint`
# feature). Mirrors the solana-free kernels byte-for-byte — see
# `quote_write.rs`, `reference_price.rs` and `liquidity_profile.rs` under
# `src/state/market/`. (This file is embedded in a `global_asm!` template
# string, so curly braces must not appear anywhere in it — comments
# included — or they are read as operand placeholders.) Modeled on the
# anchor-next `prop-amm` oracle fast-path demo.
#
# Both discriminators share one preamble (layout integrity, sector bounds,
# the `quote_authority` compare, the nonce bump and flush arm) exactly as
# the Rust kernels share `quote_write.rs`, and diverge only at the payload:
# `set_reference_price` stores two u32s, `set_liquidity_profile` copies the
# 160-byte profile blob with `sol_memcpy_`. Neither validates its payload —
# matching skips an invalid price, and an over-cap ladder side is dropped at
# flush time.
#
# Entry ABI (anchor-next asm): r1 = serialized accounts region
# (num_accounts at r1+0, then account records), r2 = instruction data
# (discriminator at r2+0). Account records use agave's aligned layout:
#   [88-byte header | data | MAX_PERMITTED_DATA_INCREASE(10240) | pad-to-8
#    | rent_epoch(8)]
# with header fields: +1 is_signer, +2 is_writable, +8 pubkey(32),
# +80 data_len(8), +88 data.
#
# Account order is [signer(0), market(1)] on both paths. The signer is
# required to carry NO data (data_len == 0) so the market record sits at a
# *static* input offset regardless of the market's (variable) size. Every
# offset below is pinned by the `offset_of!` assertion test so the assembly
# and the Rust layout cannot drift.
#
# Register discipline through the shared preamble: r1 = accounts region,
# r2 = instruction data, r6 = the discriminator (held for the payload
# branch), r9 = absolute pointer to the target vault, r3/r4/r5 = scratch.
# `sol_memcpy_` clobbers r0-r5 and preserves r6-r9, and the profile branch
# exits immediately after it, so nothing needs stashing across the call.

# --- instructions ---
.equ DISCRIM_SET_REFERENCE_PRICE, 5
.equ DISCRIM_SET_LIQUIDITY_PROFILE, 6
.equ IX_VAULT_IDX_OFF, 1          # u32, right after the 1-byte discriminator
# set_reference_price payload
.equ IX_PRICE_BITS_OFF, 5         # u32
.equ IX_QUOTE_SLOT_OFF, 9         # u32
# set_liquidity_profile payload
.equ IX_PROFILE_OFF, 5            # [u8; 160], past disc(1) + vault_idx(4)

# --- account 0: signer ---
.equ SIGNER_IS_SIGNER_OFF, 9      # acct0_base(8) + header is_signer(1)
.equ SIGNER_PUBKEY_OFF, 16        # acct0_base(8) + header pubkey(8)
.equ SIGNER_DATA_LEN_OFF, 88      # acct0_base(8) + header data_len(80)

# --- account 1: market (signer empty -> static base 10344) ---
# 10344 = num_accounts(8) + header(88) + data_len 0 + DATA_INCREASE(10240)
#         + rent_epoch(8)
.equ MARKET_BASE, 10344
.equ MARKET_IS_WRITABLE_OFF, MARKET_BASE + 2
.equ MARKET_DATA_LEN_OFF, MARKET_BASE + 80
.equ MARKET_DATA_OFF, MARKET_BASE + 88

# --- market data framing: [disc(8)][MarketHeader(251)][len:u32][pad][vaults] ---
# align_of::<Vault>() == 4 (Vault embeds the u32-aligned Price), so items
# start at align_up(8 + 251 + 4, 4) = 264, not 263.
.equ MARKET_NONCE_OFF, MARKET_DATA_OFF + 8       # MarketHeader.nonce (u64)
.equ MARKET_LEN_OFF, MARKET_DATA_OFF + 259       # slab len (u32)
.equ SLAB_ITEMS_OFF, 264                         # first Vault, within data
.equ VAULT_SIZE, 560
.equ PROFILE_SIZE, 160                           # size_of::<LiquidityProfile>()

# --- Vault field offsets ---
.equ VAULT_QUOTE_AUTHORITY_OFF, 40
.equ RP_STAMP_OFF, 72             # reference_price.stamp (u64)
.equ RP_PRICE_OFF, 80             # reference_price.price (u32)
.equ RP_QUOTE_SLOT_OFF, 84        # reference_price.quote_slot (u32)
.equ VAULT_PROFILE_OFF, 144       # profile (LiquidityProfile, PROFILE_SIZE B)

# --- constants ---
.equ FLUSH_BIT, 0x8000000000000000

# --- error codes ---
# Domain codes equal the anchor #[error_code] Custom values (variant + 6000)
# so the fast path and the reference build surface the same code.
.equ E_UNAUTHORIZED, 6005         # DropsetError::Unauthorized
.equ E_INVALID_SECTOR, 6010       # DropsetError::InvalidSectorIndex
# Structural codes are asm-specific (the reference build surfaces anchor's
# built-in account errors instead — parity maps them, doesn't equate them).
.equ E_FEW_ACCOUNTS, 101
.equ E_NOT_SIGNER, 102
.equ E_SIGNER_HAS_DATA, 103
.equ E_MARKET_NOT_WRITABLE, 104
#
# One further asm-only asymmetry, deliberately UNGUARDED: neither branch
# bounds the instruction-data length before reading its payload. This is a
# settled decision, not an outstanding gap — these paths trust the market
# maker to call them correctly, and the audit below is why that is safe.
#
# SURPLUS ix data is a non-event: every read is a fixed width at a fixed
# offset, so nothing scans and no read is length-derived. The shared
# preamble takes vault_idx at +1; disc 5 then reads two more u32s (max
# extent ix_data + 13) and disc 6 the 160-byte blob at +5 (max extent
# ix_data + 165). Bytes past that are simply never read.
#
# A TRUNCATED payload only ever harms its own caller. The reference build
# rejects one at anchor deserialization; here a short disc-6 call makes the
# 160-byte copy read past the ix-data region. The input region ends at
# ix_data + len + program_id(32), so a length under 133 faults
# (AccessViolation, the caller's own tx fails) while 133-164 copies the
# trailing program-id bytes — public data — into the caller's own ladder
# and succeeds silently. Every SDK builder emits the full 165 bytes.
#
# Neither case can be turned into an injection. The copy length is the
# PROFILE_SIZE constant, never payload-derived. The destination is the
# fixed (vault + VAULT_PROFILE_OFF, PROFILE_SIZE) window, and the one
# attacker-controlled input to it — vault_idx — cannot select a sector the
# caller does not already own: it is bounds-checked twice (idx < slab len,
# then vault end <= data_len), and the resulting sector's quote_authority
# has been compared against the signer over all 32 bytes. So malformed ix
# data cannot reach another vault, the header beyond the nonce, or any
# accounting field.
#
# Nor can a garbled ladder brick the market for anyone else. Level is
# all-Pod, so arbitrary bytes are a *valid* LiquidityProfile (no invalid
# bit patterns, no UB), and every match-time consumer of it is a total
# function: materialize_remaining zeroes an over-cap side out of the book
# rather than aborting the take (it is the sole enforcement of
# Σ size_bps <= BPS), level_fill_atoms falls back to 0, flush_level_price
# cannot fail (saturating subtraction plus Price::ZERO fallbacks — an
# extreme price_offset yields a bogus-but-valid price, never a trap), and
# side_size_sums saturates. No panic and no Err, so no taker is ever
# blocked.
#
# The blast radius is therefore exactly one leader garbling their own
# quotes, self-healing the moment they resubmit a valid ladder — worth less
# than the leader-hot-path CU a length check would cost.

.global entrypoint

entrypoint:
    # Fast-path the two quote writes; forward everything else. r6 keeps the
    # discriminator for the payload branch at the end of the preamble.
    ldxb r6, [r2 + 0]
    jeq r6, DISCRIM_SET_REFERENCE_PRICE, quote_write
    jeq r6, DISCRIM_SET_LIQUIDITY_PROFILE, quote_write

dispatch:
    call __anchor_dispatch
    exit

# --- shared quote-write preamble (mirrors quote_write.rs) ---
quote_write:
    # Layout integrity: need [signer, market].
    ldxdw r3, [r1 + 0]
    jlt r3, 2, err_few_accounts
    ldxb r3, [r1 + SIGNER_IS_SIGNER_OFF]
    jeq r3, 0, err_not_signer
    # Signer must carry no data, so the market record stays at MARKET_BASE.
    ldxdw r3, [r1 + SIGNER_DATA_LEN_OFF]
    jne r3, 0, err_signer_has_data
    ldxb r3, [r1 + MARKET_IS_WRITABLE_OFF]
    jeq r3, 0, err_market_not_writable

    # vault_idx bounds: reject unless idx < min(len, capacity), matching
    # Slab::as_mut_slice's effective_len. Split to avoid a division.
    ldxw r4, [r2 + IX_VAULT_IDX_OFF]     # r4 = vault_idx
    ldxw r5, [r1 + MARKET_LEN_OFF]       # r5 = slab len
    jge r4, r5, err_invalid_sector       # idx >= len
    mul64 r4, VAULT_SIZE
    add64 r4, SLAB_ITEMS_OFF             # r4 = vault offset within data
    mov64 r5, r4
    add64 r5, VAULT_SIZE                 # r5 = vault end within data
    ldxdw r3, [r1 + MARKET_DATA_LEN_OFF] # r3 = market data_len
    jgt r5, r3, err_invalid_sector       # idx >= capacity

    # Absolute pointer to the target vault (keeps subsequent loads/stores
    # within the i16 offset range whatever vault_idx is).
    mov64 r9, r1
    add64 r9, MARKET_DATA_OFF
    add64 r9, r4                         # r9 = &vault

    # Only domain guard: signer.key == vault.quote_authority (4x u64).
    ldxdw r3, [r1 + SIGNER_PUBKEY_OFF + 0]
    ldxdw r4, [r9 + VAULT_QUOTE_AUTHORITY_OFF + 0]
    jne r3, r4, err_unauthorized
    ldxdw r3, [r1 + SIGNER_PUBKEY_OFF + 8]
    ldxdw r4, [r9 + VAULT_QUOTE_AUTHORITY_OFF + 8]
    jne r3, r4, err_unauthorized
    ldxdw r3, [r1 + SIGNER_PUBKEY_OFF + 16]
    ldxdw r4, [r9 + VAULT_QUOTE_AUTHORITY_OFF + 16]
    jne r3, r4, err_unauthorized
    ldxdw r3, [r1 + SIGNER_PUBKEY_OFF + 24]
    ldxdw r4, [r9 + VAULT_QUOTE_AUTHORITY_OFF + 24]
    jne r3, r4, err_unauthorized

    # Bump the nonce; stamp carries the OLD nonce OR'd with the flush bit.
    # Leaves price / quote_slot alone — each payload writes its own field.
    ldxdw r3, [r1 + MARKET_NONCE_OFF]    # r3 = old nonce
    lddw r4, FLUSH_BIT
    or64 r4, r3                          # r4 = old_nonce | FLUSH_BIT
    stxdw [r9 + RP_STAMP_OFF], r4
    add64 r3, 1
    stxdw [r1 + MARKET_NONCE_OFF], r3    # nonce += 1

    jeq r6, DISCRIM_SET_LIQUIDITY_PROFILE, write_profile

# --- set_reference_price payload (mirrors reference_price.rs) ---
    # Store the raw price and quote_slot (two adjacent u32s).
    ldxw r3, [r2 + IX_PRICE_BITS_OFF]
    stxw [r9 + RP_PRICE_OFF], r3
    ldxw r3, [r2 + IX_QUOTE_SLOT_OFF]
    stxw [r9 + RP_QUOTE_SLOT_OFF], r3

    mov64 r0, 0
    exit

# --- set_liquidity_profile payload (mirrors liquidity_profile.rs) ---
write_profile:
    # One `sol_memcpy_` of the whole 160-byte blob: the syscall is metered
    # at max(10, len / 250) CU, so ~10 CU against ~40 for the 20 hand-rolled
    # ldxdw/stxdw pairs a chunked copy would need. dst is the program-owned
    # writable market data, src the readable instruction-data region — the
    # two never overlap (dst precedes src in the input buffer).
    #
    # The source length is NOT bounded against the instruction-data length —
    # see the note under the error codes above for why that is safe and
    # deliberate.
    add64 r2, IX_PROFILE_OFF             # r2 = &ix.profile_bytes  (src)
    mov64 r1, r9
    add64 r1, VAULT_PROFILE_OFF          # r1 = &vault.profile     (dst)
    mov64 r3, PROFILE_SIZE               # r3 = len
    call sol_memcpy_

    mov64 r0, 0
    exit

err_few_accounts:
    mov64 r0, E_FEW_ACCOUNTS
    exit
err_not_signer:
    mov64 r0, E_NOT_SIGNER
    exit
err_signer_has_data:
    mov64 r0, E_SIGNER_HAS_DATA
    exit
err_market_not_writable:
    mov64 r0, E_MARKET_NOT_WRITABLE
    exit
err_unauthorized:
    mov64 r0, E_UNAUTHORIZED
    exit
err_invalid_sector:
    mov64 r0, E_INVALID_SECTOR
    exit
