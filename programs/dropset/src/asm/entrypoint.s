# cspell:word DISCRIM
# cspell:word lddw
# cspell:word ldxb
# cspell:word ldxdw
# cspell:word ldxw
# cspell:word stxdw
# cspell:word stxw
# Hybrid sBPF entrypoint for the dropset program.
#
# Short-circuits the `set_reference_price` discriminator (5) and stamps the
# vault's reference price inline, then exits; every other discriminator is
# forwarded to `__anchor_dispatch` (the dispatcher `#[program]` emits under
# the crate's `no-entrypoint` feature). Mirrors the solana-free
# `stamp_reference_price` kernel byte-for-byte — see
# `src/state/market/reference_price.rs`. Modeled on the anchor-next
# `prop-amm` oracle fast-path demo.
#
# Entry ABI (anchor-next asm): r1 = serialized accounts region
# (num_accounts at r1+0, then account records), r2 = instruction data
# (discriminator at r2+0). Account records use agave's aligned layout:
#   [88-byte header | data | MAX_PERMITTED_DATA_INCREASE(10240) | pad-to-8
#    | rent_epoch(8)]
# with header fields: +1 is_signer, +2 is_writable, +8 pubkey(32),
# +80 data_len(8), +88 data.
#
# Account order is [signer(0), market(1)]. The signer is required to carry
# NO data (data_len == 0) so the market record sits at a *static* input
# offset regardless of the market's (variable) size. Every offset below is
# pinned by the `offset_of!` assertion test so the assembly and the Rust
# layout cannot drift.

# --- instruction ---
.equ DISCRIM, 5
.equ IX_VAULT_IDX_OFF, 1          # u32, right after the 1-byte discriminator
.equ IX_PRICE_BITS_OFF, 5         # u32
.equ IX_QUOTE_SLOT_OFF, 9         # u32

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

# --- market data framing: [disc(8)][MarketHeader(237)][len:u32][pad][vaults] ---
# align_of::<Vault>() == 4 (Vault embeds the u32-aligned Price), so items
# start at align_up(8 + 237 + 4, 4) = 252, not 249.
.equ MARKET_NONCE_OFF, MARKET_DATA_OFF + 8       # MarketHeader.nonce (u64)
.equ MARKET_LEN_OFF, MARKET_DATA_OFF + 245       # slab len (u32)
.equ SLAB_ITEMS_OFF, 252                         # first Vault, within data
.equ VAULT_SIZE, 560

# --- Vault field offsets ---
.equ VAULT_QUOTE_AUTHORITY_OFF, 40
.equ RP_STAMP_OFF, 72             # reference_price.stamp (u64)
.equ RP_PRICE_OFF, 80             # reference_price.price (u32)
.equ RP_QUOTE_SLOT_OFF, 84        # reference_price.quote_slot (u32)

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

.global entrypoint

entrypoint:
    # Fast-path only our discriminator; forward everything else.
    ldxb r3, [r2 + 0]
    jne r3, DISCRIM, dispatch

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
    mov64 r6, r4
    mul64 r6, VAULT_SIZE
    add64 r6, SLAB_ITEMS_OFF             # r6 = vault offset within data
    mov64 r7, r6
    add64 r7, VAULT_SIZE                 # r7 = vault end within data
    ldxdw r8, [r1 + MARKET_DATA_LEN_OFF] # r8 = market data_len
    jgt r7, r8, err_invalid_sector       # idx >= capacity

    # Absolute pointer to the target vault (keeps subsequent loads/stores
    # within the i16 offset range whatever vault_idx is).
    mov64 r9, r1
    add64 r9, MARKET_DATA_OFF
    add64 r9, r6                         # r9 = &vault

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
    ldxdw r3, [r1 + MARKET_NONCE_OFF]    # r3 = old nonce
    lddw r4, FLUSH_BIT
    or64 r4, r3                          # r4 = old_nonce | FLUSH_BIT
    stxdw [r9 + RP_STAMP_OFF], r4
    add64 r3, 1
    stxdw [r1 + MARKET_NONCE_OFF], r3    # nonce += 1

    # Store the raw price and quote_slot (two adjacent u32s).
    ldxw r3, [r2 + IX_PRICE_BITS_OFF]
    stxw [r9 + RP_PRICE_OFF], r3
    ldxw r3, [r2 + IX_QUOTE_SLOT_OFF]
    stxw [r9 + RP_QUOTE_SLOT_OFF], r3

    mov64 r0, 0
    exit

dispatch:
    call __anchor_dispatch
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
