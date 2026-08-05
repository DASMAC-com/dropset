//! On-chain I/O — market discovery, self-funding, off-chain order sizing, and
//! the swap send.
//!
//! Discovery mirrors the maker-bot (`bots/maker-bot/src/chain.rs`): scan the
//! program's accounts for the `MarketHeader` discriminator and decode the
//! single localnet market through the slab-layout mirror. Funding mirrors the
//! TUI's localnet plumbing (`tui/src/chain.rs`): airdrop the taker its fee
//! SOL, create its two ATAs, and mint it starting inventory under the mock
//! mints' authority. Each order is **sized off-chain** before it is sent:
//! [`dropset_sdk::matching::simulate_swap`] turns a sampled notional into the
//! achievable `amount_in` / `min_out` at the live book — and caps it at a
//! fraction of the depth resting there, so a take scales with the book instead
//! of clearing it — and the swap itself is built with the generated
//! [`SwapBuilder`] and signed by the taker.

use anyhow::{anyhow, Context as _, Result};
use dropset_localnet_support::{
    associated_token_address, create_ata_idempotent_ix, mint_to_ix, SPL_TOKEN_PROGRAM_ID,
};
use dropset_sdk::accounts::MARKET_HEADER_DISCRIMINATOR;
use dropset_sdk::instructions::SwapBuilder;
use dropset_sdk::layout::MarketView;
use dropset_sdk::matching::{simulate_swap, SwapSide};
use dropset_sdk::price::Price;
use dropset_sdk::DROPSET_ID;
use solana_client::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use solana_transaction::Transaction;
use std::time::Duration;

use crate::context::MarketAddrs;
use crate::model::Order;

/// SPL Token Mint `decimals` byte offset (after `COption<Pubkey>` authority +
/// `u64` supply).
const MINT_DECIMALS_OFFSET: usize = 44;
/// SPL Token Account `amount` (`u64`) byte offset (after mint + owner).
const TOKEN_ACCOUNT_AMOUNT_OFFSET: usize = 64;

/// An `RpcClient` at `confirmed`, pointed at `url`.
pub fn rpc(url: &str) -> RpcClient {
    RpcClient::new_with_timeout_and_commitment(
        url.to_string(),
        Duration::from_secs(10),
        CommitmentConfig::confirmed(),
    )
}

/// The genesis hashes of the three public Solana clusters. `assert_localnet`
/// refuses to run against any of them — this bot signs with local keys,
/// including the committed localnet admin keypair as the mock-mint authority
/// (`config::DEFAULT_MINT_AUTHORITY_KEY`), so a real cluster behind `--rpc`
/// would mean real `MintTo` and `swap` sends. Cross-checked against the Solana
/// docs and the gill / mpl-bubblegum SDKs.
const MAINNET_GENESIS: &str = "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d";
const DEVNET_GENESIS: &str = "EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG";
const TESTNET_GENESIS: &str = "4uhcVJyU9pJkvQyS88uRDiswHXSCkY3zQawwpjk2NsNY";

/// The name of the public Solana cluster with this genesis hash, or `None` for
/// any other cluster (a localnet test validator mints a fresh genesis per
/// launch). Pure, so the denylist is unit-testable without a validator.
fn public_cluster(genesis: &str) -> Option<&'static str> {
    match genesis {
        MAINNET_GENESIS => Some("mainnet-beta"),
        DEVNET_GENESIS => Some("devnet"),
        TESTNET_GENESIS => Some("testnet"),
        _ => None,
    }
}

/// Abort unless `client` is a localnet validator. Keyed on the cluster's
/// genesis hash rather than the RPC host, so it allows a localnet on any
/// address (LAN, Docker) yet still trips on a port-forward / proxy that tunnels
/// a public cluster through a loopback URL. Call once at startup, before the
/// first signed send.
pub fn assert_localnet(client: &RpcClient) -> Result<()> {
    let genesis = client
        .get_genesis_hash()
        .context("get genesis hash")?
        .to_string();
    if let Some(cluster) = public_cluster(&genesis) {
        return Err(anyhow!(
            "refusing to run against the {cluster} public cluster (genesis \
             {genesis}): this localnet bot signs transactions with local keys \
             — including the committed localnet admin keypair as the mock-mint \
             authority — and must run only against a localnet test validator"
        ));
    }
    Ok(())
}

/// The self-CPI event-authority PDA — seeds `[b"__event_authority"]`.
fn event_authority() -> Pubkey {
    Pubkey::find_program_address(&[b"__event_authority"], &DROPSET_ID).0
}

/// Airdrop `lamports` to `who` and block until it confirms (localnet faucet).
pub fn airdrop(client: &RpcClient, who: &Pubkey, lamports: u64) -> Result<()> {
    let sig = client.request_airdrop(who, lamports).context("airdrop")?;
    for _ in 0..50 {
        if client.confirm_transaction(&sig).unwrap_or(false) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err(anyhow!("airdrop did not confirm in time"))
}

/// Discover a localnet market by scanning the program's accounts for the
/// `MarketHeader` discriminator, then read its mints, treasuries, and the
/// pair's decimals. With `target` set, only that exact market PDA matches (the
/// per-market path the TUI drives, so one instance trades the selected book);
/// otherwise the first market the scan turns up (the single-market default).
pub fn discover_market(client: &RpcClient, target: Option<Pubkey>) -> Result<MarketAddrs> {
    let accounts = client
        .get_program_accounts(&DROPSET_ID)
        .context("get_program_accounts")?;
    let (address, account) = accounts
        .iter()
        .find(|(addr, a)| {
            a.data.len() >= 8
                && a.data[..8] == MARKET_HEADER_DISCRIMINATOR
                && target.is_none_or(|t| *addr == t)
        })
        .ok_or_else(|| match target {
            Some(t) => anyhow!("market {t} not found — wrong address, or not bootstrapped?"),
            None => anyhow!("no market found — is the localnet bootstrapped?"),
        })?;

    let view = MarketView::load(&account.data).map_err(|e| anyhow!("decode market: {e:?}"))?;
    let header = view.header;
    let base_mint = Pubkey::new_from_array(header.base_mint);
    let quote_mint = Pubkey::new_from_array(header.quote_mint);

    Ok(MarketAddrs {
        market: *address,
        base_mint,
        quote_mint,
        base_treasury: Pubkey::new_from_array(header.base_treasury),
        quote_treasury: Pubkey::new_from_array(header.quote_treasury),
        base_decimals: mint_decimals(client, &base_mint).context("base mint decimals")?,
        quote_decimals: mint_decimals(client, &quote_mint).context("quote mint decimals")?,
    })
}

/// Read an SPL mint's `decimals`.
fn mint_decimals(client: &RpcClient, mint: &Pubkey) -> Result<u8> {
    let account = client.get_account(mint).context("get mint account")?;
    account
        .data
        .get(MINT_DECIMALS_OFFSET)
        .copied()
        .ok_or_else(|| anyhow!("mint account too small"))
}

/// Read an SPL token account's `amount` (atoms), or `0` if it doesn't exist
/// yet (an un-created ATA holds nothing).
fn token_balance(client: &RpcClient, ata: &Pubkey) -> Result<u64> {
    let Some(account) = client
        .get_account_with_commitment(ata, client.commitment())?
        .value
    else {
        return Ok(0);
    };
    let bytes = account
        .data
        .get(TOKEN_ACCOUNT_AMOUNT_OFFSET..TOKEN_ACCOUNT_AMOUNT_OFFSET + 8)
        .ok_or_else(|| anyhow!("token account too small"))?;
    Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
}

/// Create the associated token account for `(wallet, mint)` under the SPL
/// Token program idempotently (`CreateIdempotent`), paid by `payer`. Returns
/// the ATA.
pub fn create_ata_idempotent(
    client: &RpcClient,
    payer: &Keypair,
    wallet: &Pubkey,
    mint: &Pubkey,
) -> Result<Pubkey> {
    let ata = associated_token_address(wallet, mint, &SPL_TOKEN_PROGRAM_ID);
    let ix = create_ata_idempotent_ix(&payer.pubkey(), wallet, mint, &SPL_TOKEN_PROGRAM_ID);
    send(client, payer, &[payer], &[ix]).context("create ATA")?;
    Ok(ata)
}

/// Mint `amount` atoms of `mint` to `ata`; `authority` must be the mint
/// authority (SPL Token `MintTo`).
pub fn mint_to(
    client: &RpcClient,
    authority: &Keypair,
    mint: &Pubkey,
    ata: &Pubkey,
    amount: u64,
) -> Result<String> {
    let ix = mint_to_ix(&authority.pubkey(), mint, ata, amount);
    send(client, authority, &[authority], &[ix])
}

/// Whole-token count `tokens` expressed in atoms for `decimals`.
fn to_atoms(tokens: f64, decimals: u8) -> u64 {
    (tokens * 10f64.powi(decimals as i32)) as u64
}

/// Ensure the taker can trade: top up its SOL when low, create its two ATAs,
/// and refill either leg that has fallen below `min_tokens` back up to
/// `target_tokens` (minted under the mock-mint authority). Idempotent, so it
/// is safe to call every tick.
#[allow(clippy::too_many_arguments)]
pub fn ensure_funded(
    client: &RpcClient,
    taker: &Keypair,
    mint_authority: &Keypair,
    market: &MarketAddrs,
    airdrop_lamports: u64,
    min_lamports: u64,
    target_tokens: f64,
    min_tokens: f64,
) -> Result<()> {
    let balance = client
        .get_balance(&taker.pubkey())
        .context("taker balance")?;
    if balance < min_lamports {
        airdrop(client, &taker.pubkey(), airdrop_lamports)?;
    }

    let base_ata = create_ata_idempotent(client, taker, &taker.pubkey(), &market.base_mint)
        .context("taker base ATA")?;
    let quote_ata = create_ata_idempotent(client, taker, &taker.pubkey(), &market.quote_mint)
        .context("taker quote ATA")?;

    refill_leg(
        client,
        mint_authority,
        &market.base_mint,
        &base_ata,
        market.base_decimals,
        target_tokens,
        min_tokens,
    )
    .context("refill base leg")?;
    refill_leg(
        client,
        mint_authority,
        &market.quote_mint,
        &quote_ata,
        market.quote_decimals,
        target_tokens,
        min_tokens,
    )
    .context("refill quote leg")?;
    Ok(())
}

/// Mint `ata` back up to `target_tokens` when its balance is below
/// `min_tokens`.
fn refill_leg(
    client: &RpcClient,
    authority: &Keypair,
    mint: &Pubkey,
    ata: &Pubkey,
    decimals: u8,
    target_tokens: f64,
    min_tokens: f64,
) -> Result<()> {
    let balance = token_balance(client, ata)?;
    let min_atoms = to_atoms(min_tokens, decimals);
    if balance >= min_atoms {
        return Ok(());
    }
    let target_atoms = to_atoms(target_tokens, decimals);
    let deficit = target_atoms.saturating_sub(balance);
    if deficit > 0 {
        mint_to(client, authority, mint, ata, deficit)?;
    }
    Ok(())
}

/// A swap sized against the live book, ready to submit.
#[derive(Clone, Copy, Debug)]
pub struct SizedSwap {
    pub side: SwapSide,
    /// Exact input atoms (quote for a Buy, base for a Sell).
    pub amount_in: u64,
    /// Worst acceptable fill price, encoded `Price` bits.
    pub limit_price_bits: u32,
    /// Slippage floor on the output leg (atoms).
    pub min_out: u64,
    /// The simulator's expected net output at the limit (atoms) — for logging.
    pub expected_out: u64,
    /// Whether the depth cap shortened this take below its sampled notional —
    /// for logging, so a run that is constantly clamping is visible rather than
    /// looking like a flow of identically-sized orders.
    pub depth_capped: bool,
}

/// The market's current reference price (quote-per-base) as a float, taken
/// from the first active, validly-priced vault. `None` if no vault is quoting.
fn market_reference_price(view: &MarketView<'_>) -> Option<f64> {
    view.active_vaults().find_map(|(_, v)| {
        // The matcher's own gate, shared rather than re-derived: a maker that
        // has killed its book (the stale-quote invalidation stamps the zero
        // sentinel) must read here as "not quoting", so the taker skips the
        // order instead of sizing against a book it can't fill.
        let p = v.reference_price.price();
        p.is_matchable().then(|| p.to_f64())
    })
}

/// The taker input atoms this book can absorb on `side` without crossing
/// `limit_price` — the live depth a take is measured against.
///
/// Measured by running the *same* fill path the swap will take with an
/// unbounded input and reporting what it consumed, so every constraint the
/// engine applies is already folded in: level sizes, the per-side `size_bps`
/// gate, level expiry, and — the one that makes this worth a fill rather than
/// a book read — each vault's own inventory. `resting_levels` shares the
/// level collector, so it sees the first three; it runs no fill, so a sum over
/// it over-reports a drained vault, sizing the take against base the vault
/// cannot pay out.
fn takeable_depth_atoms(
    view: &MarketView<'_>,
    side: SwapSide,
    limit_price: Price,
    slot: u32,
) -> u64 {
    simulate_swap(view, side, u64::MAX, limit_price, slot).in_amount
}

/// The input-atom ceiling for one take: `fraction` of the book's
/// `depth_atoms`.
///
/// A non-finite or non-positive `fraction` disables the cap, so a mis-set knob
/// widens takes rather than silently zeroing every one of them; a `fraction`
/// at or above `1.0` is effectively a no-op, since the fill is depth-bounded
/// anyway. Whenever there is depth the ceiling stays at least one atom, so the
/// cap itself never zeroes a take on a very thin book; whether that one atom
/// actually fills is then the fill check's call, not an artifact of rounding
/// the ceiling down.
fn depth_cap_atoms(depth_atoms: u64, fraction: f64) -> u64 {
    if !fraction.is_finite() || fraction <= 0.0 {
        return u64::MAX;
    }
    if depth_atoms == 0 {
        return 0;
    }
    ((depth_atoms as f64 * fraction) as u64).max(1)
}

/// Size one sampled [`Order`] against `view`: convert its quote notional into
/// an `amount_in` for the chosen leg, derive the `limit_price_bits` from the
/// reference price and slippage tolerance, cap the take at
/// `max_depth_fraction` of the depth resting inside that bound, and floor
/// `min_out` below what the simulator says fills.
///
/// The depth cap is what keeps the flow proportional to the book. A sampled
/// notional is an absolute quote size, so against a thin book its tail can
/// clear several levels at once and visibly empty the side; against a deep one
/// the same size is invisible. Sizing off the depth actually resting inside the
/// limit price makes a take nibble the top of the book whether the maker is
/// quoting $100 or $1M, and shrinks takes automatically as its inventory
/// drains mid-run.
///
/// Pure: the book snapshot, slot, and market metadata are all passed in, so the
/// whole sizing decision is testable without a validator. Returns `None` when
/// the order can't be priced or wouldn't fill — no quoting vault, a zero-atom
/// size, an out-of-range limit price, or no liquidity inside the bound — so the
/// tick simply skips it.
pub fn size_against_book(
    view: &MarketView<'_>,
    market: &MarketAddrs,
    order: &Order,
    slippage: f64,
    max_depth_fraction: f64,
    slot: u32,
) -> Option<SizedSwap> {
    let price = market_reference_price(view)?;

    // Convert the quote notional into the input leg's atoms.
    let requested_in = match order.side {
        SwapSide::Buy => to_atoms(order.notional, market.quote_decimals),
        SwapSide::Sell => to_atoms(order.notional / price, market.base_decimals),
    };
    if requested_in == 0 {
        return None;
    }

    // Worst acceptable price: above the reference for a Buy, below for a Sell.
    let limit_value = match order.side {
        SwapSide::Buy => price * (1.0 + slippage),
        SwapSide::Sell => price * (1.0 - slippage),
    };
    let limit_price = Price::from_value(limit_value)?;

    // Clamp to a fraction of the depth inside that limit — see above.
    let depth = takeable_depth_atoms(view, order.side, limit_price, slot);
    let amount_in = requested_in.min(depth_cap_atoms(depth, max_depth_fraction));
    if amount_in == 0 {
        return None;
    }

    let quote = simulate_swap(view, order.side, amount_in, limit_price, slot);
    if quote.out_amount == 0 {
        return None;
    }
    // Floor `min_out` below the simulated output so a benign book move between
    // sizing and execution doesn't trip the on-chain slippage check — but keep
    // it at least 1, since `min_out == 0` opts out of the on-chain soft-revert
    // entirely (swap.rs), which would drop slippage protection on a dust order.
    let min_out = ((quote.out_amount as f64 * (1.0 - slippage)) as u64).max(1);

    Some(SizedSwap {
        side: order.side,
        amount_in,
        limit_price_bits: limit_price.as_u32(),
        min_out,
        expected_out: quote.out_amount,
        depth_capped: amount_in < requested_in,
    })
}

/// Read the live book and slot, then size `order` against them with
/// [`size_against_book`]. The IO half of the sizing step.
pub fn size_order(
    client: &RpcClient,
    market: &MarketAddrs,
    order: &Order,
    slippage: f64,
    max_depth_fraction: f64,
) -> Result<Option<SizedSwap>> {
    let account = client
        .get_account(&market.market)
        .context("get market account")?;
    let view = MarketView::load(&account.data).map_err(|e| anyhow!("decode market: {e:?}"))?;
    let slot = client.get_slot().context("get_slot")? as u32;
    Ok(size_against_book(
        &view,
        market,
        order,
        slippage,
        max_depth_fraction,
        slot,
    ))
}

/// Build and send a `swap`, signed and paid by the taker. Returns the
/// transaction signature.
pub fn send_swap(
    client: &RpcClient,
    taker: &Keypair,
    market: &MarketAddrs,
    swap: &SizedSwap,
) -> Result<String> {
    let ix = SwapBuilder::new()
        .taker(taker.pubkey())
        .market(market.market)
        .base_mint(market.base_mint)
        .quote_mint(market.quote_mint)
        .base_token_program(SPL_TOKEN_PROGRAM_ID)
        .quote_token_program(SPL_TOKEN_PROGRAM_ID)
        .taker_base_ata(associated_token_address(
            &taker.pubkey(),
            &market.base_mint,
            &SPL_TOKEN_PROGRAM_ID,
        ))
        .taker_quote_ata(associated_token_address(
            &taker.pubkey(),
            &market.quote_mint,
            &SPL_TOKEN_PROGRAM_ID,
        ))
        .market_base_treasury(market.base_treasury)
        .market_quote_treasury(market.quote_treasury)
        .event_authority(event_authority())
        .program(DROPSET_ID)
        .side(swap.side as u8)
        .amount_in(swap.amount_in)
        .limit_price_bits(swap.limit_price_bits)
        .min_out(swap.min_out)
        .instruction();
    send(client, taker, &[taker], &[ix])
}

/// Sign `ixs` with `signers` (fee payer = `payer`) and send, confirming at the
/// client's commitment. On failure, re-simulate to recover the program logs a
/// `ClientError` drops for a custom-program error (state is unchanged after a
/// failed send, so the re-simulation reproduces the same error).
fn send(
    client: &RpcClient,
    payer: &Keypair,
    signers: &[&Keypair],
    ixs: &[Instruction],
) -> Result<String> {
    let blockhash = client.get_latest_blockhash().context("blockhash")?;
    let tx = Transaction::new_signed_with_payer(ixs, Some(&payer.pubkey()), signers, blockhash);
    match client.send_and_confirm_transaction(&tx) {
        Ok(sig) => Ok(sig.to_string()),
        Err(err) => {
            let logs = client
                .simulate_transaction(&tx)
                .ok()
                .and_then(|r| r.value.logs)
                .filter(|l| !l.is_empty())
                .map(|l| format!("\n{}", l.join("\n")))
                .unwrap_or_default();
            Err(anyhow!("{err}{logs}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;
    use dropset_sdk::layout::{MarketHeader, Vault, NULL_SECTOR, VAULT_ALIGN};

    /// Both legs of the synthetic market use USDC-style decimals, so a human
    /// price of 1.0 is also an atoms-ratio of 1.0 and the arithmetic below
    /// reads directly.
    const DECIMALS: u8 = 6;
    /// The slot every sizing test prices at — before the fixture's level
    /// expiry, so its levels are live.
    const SLOT: u32 = 10;
    /// Per-leg vault inventory ample enough that a fixture's depth is set by
    /// its level sizes — unless a test deliberately starves it, which is what
    /// distinguishes the depth probe from a sum over the resting levels.
    const INVENTORY_ATOMS: u64 = 10_000_000;

    /// The encoded price `1.0 + offset_ppm / 1e6`. `px(0)` is exactly 1.0 —
    /// the reference every fixture quotes at, so with both legs at the same
    /// decimals a base atom and a quote atom are interchangeable and the
    /// arithmetic below reads directly.
    fn px(offset_ppm: u32) -> Price {
        // The significand is the price scaled by 10^7, so 1 ppm is 10 units.
        Price::encode(10_000_000 + 10 * offset_ppm, 0).expect("encodable price")
    }

    /// `MarketAddrs` for the synthetic market — only the decimals matter to
    /// [`size_against_book`]; the addresses are never dereferenced.
    fn addrs() -> MarketAddrs {
        MarketAddrs {
            market: Pubkey::new_unique(),
            base_mint: Pubkey::from([1u8; 32]),
            quote_mint: Pubkey::from([2u8; 32]),
            base_treasury: Pubkey::new_unique(),
            quote_treasury: Pubkey::new_unique(),
            base_decimals: DECIMALS,
            quote_decimals: DECIMALS,
        }
    }

    /// A one-vault market account buffer whose reference price is 1.0, quoting
    /// the given `asks` and `bids` as `(price, size)` pairs — an ask's size is
    /// base atoms, a bid's is quote atoms — and holding `inventory_atoms` of
    /// each leg. Mirrors the on-chain slab layout (8-byte discriminator,
    /// header, slab length, then `VAULT_ALIGN`-aligned sectors) the way the
    /// SDK's own adapter tests do. An empty slice leaves that side empty.
    fn market_bytes(asks: &[(Price, u64)], bids: &[(Price, u64)], inventory_atoms: u64) -> Vec<u8> {
        let mut header = MarketHeader::zeroed();
        header.head = 0u32.into();
        header.active_count = 1u32.into();
        header.base_mint = [1u8; 32];
        header.quote_mint = [2u8; 32];

        let mut v = Vault::zeroed();
        v.next = NULL_SECTOR.into();
        v.prev = NULL_SECTOR.into();
        v.leader = [9u8; 32]; // non-zero ⇒ active, not on the free list
        v.reference_price.price = px(0).as_u32().into();
        v.reference_price.stamp = 1u64.into(); // nonce 1, FLUSH_BIT clear ⇒ read `remaining`
        v.base_atoms = inventory_atoms.into();
        v.quote_atoms = inventory_atoms.into();
        for (i, &(price, size)) in asks.iter().enumerate() {
            v.remaining.asks[i].price = price.as_u32().into();
            v.remaining.asks[i].size = size.into();
            v.remaining.asks[i].expires_at = 1_000u32.into();
        }
        for (i, &(price, size)) in bids.iter().enumerate() {
            v.remaining.bids[i].price = price.as_u32().into();
            v.remaining.bids[i].size = size.into();
            v.remaining.bids[i].expires_at = 1_000u32.into();
        }

        let mut buf = vec![0u8; 8]; // discriminator (unchecked by `load`)
        buf.extend_from_slice(bytemuck::bytes_of(&header));
        buf.extend_from_slice(&1u32.to_le_bytes()); // slab length: one sector
        while !buf.len().is_multiple_of(VAULT_ALIGN) {
            buf.push(0);
        }
        buf.extend_from_slice(bytemuck::bytes_of(&v));
        buf
    }

    /// Size `notional` on `side` against `asks` / `bids` with
    /// `inventory_atoms` per leg, at the default 1% slippage and `fraction`
    /// depth cap.
    fn size_book(
        side: SwapSide,
        notional: f64,
        asks: &[(Price, u64)],
        bids: &[(Price, u64)],
        inventory_atoms: u64,
        fraction: f64,
    ) -> Option<SizedSwap> {
        let data = market_bytes(asks, bids, inventory_atoms);
        let view = MarketView::load(&data).expect("synthetic market decodes");
        let order = Order { side, notional };
        size_against_book(&view, &addrs(), &order, 0.01, fraction, SLOT)
    }

    /// The common case: one level per side at the reference price, with ample
    /// inventory — `ask_base` base atoms offered, `bid_quote` quote atoms bid.
    fn size(
        side: SwapSide,
        notional: f64,
        ask_base: u64,
        bid_quote: u64,
        fraction: f64,
    ) -> Option<SizedSwap> {
        let asks: &[(Price, u64)] = if ask_base == 0 {
            &[]
        } else {
            &[(px(0), ask_base)]
        };
        let bids: &[(Price, u64)] = if bid_quote == 0 {
            &[]
        } else {
            &[(px(0), bid_quote)]
        };
        size_book(side, notional, asks, bids, INVENTORY_ATOMS, fraction)
    }

    /// The cap is a plain fraction of depth, floors at one atom while depth
    /// remains, and is disabled by a non-positive or non-finite fraction — a
    /// mis-set knob must widen takes, never zero them all.
    #[test]
    fn depth_cap_is_a_fraction_of_depth() {
        assert_eq!(depth_cap_atoms(1_000_000, 0.25), 250_000);
        assert_eq!(depth_cap_atoms(1_000_000, 1.0), 1_000_000);
        // Rounds down, but never below a single atom while there is depth.
        assert_eq!(depth_cap_atoms(3, 0.25), 1);
        assert_eq!(depth_cap_atoms(0, 0.25), 0, "no depth ⇒ nothing takeable");
        assert_eq!(depth_cap_atoms(1_000_000, 0.0), u64::MAX, "cap disabled");
        assert_eq!(depth_cap_atoms(1_000_000, -1.0), u64::MAX, "cap disabled");
        assert_eq!(depth_cap_atoms(1_000_000, f64::NAN), u64::MAX);
    }

    /// A tail-sized take against a shallow book is clamped to the configured
    /// fraction of its depth rather than clearing the level — the bug this
    /// cap exists to prevent — and says so via `depth_capped`.
    #[test]
    fn oversized_take_is_clamped_to_a_fraction_of_depth() {
        // $1.00 of asks resting; a $100 notional would eat all of it.
        let swap = size(SwapSide::Buy, 100.0, 1_000_000, 0, 0.25).expect("fills");
        assert_eq!(swap.amount_in, 250_000, "a quarter of the $1.00 resting");
        assert!(swap.depth_capped);
        // The level survives the take: what filled is well under what rested.
        assert!(swap.expected_out < 1_000_000);
    }

    /// A take that already fits inside its share of the book passes through at
    /// its sampled size — the cap is a ceiling, not a target.
    #[test]
    fn take_within_depth_is_untouched() {
        let swap = size(SwapSide::Buy, 0.1, 1_000_000, 0, 0.25).expect("fills");
        assert_eq!(swap.amount_in, 100_000, "the sampled $0.10, unclamped");
        assert!(!swap.depth_capped);
    }

    /// Depth tracks the vault's **inventory** when that is what binds, not the
    /// level size the book advertises. This is the case that distinguishes
    /// measuring depth by an unbounded fill from summing the resting levels:
    /// the level offers a full $1.00, but the vault holds only $0.40 of base to
    /// pay out, so a resting-levels sum would over-report by 2.5× and size the
    /// take against liquidity the engine would refuse.
    #[test]
    fn depth_follows_inventory_when_it_binds() {
        let asks = [(px(0), 1_000_000)];
        let starved = size_book(SwapSide::Buy, 100.0, &asks, &[], 400_000, 0.25).expect("fills");
        assert_eq!(
            starved.amount_in, 100_000,
            "a quarter of the $0.40 the vault can actually pay out",
        );

        // Same book, ample inventory: now the level size binds and the cap is
        // 2.5× larger — so the assertion above really is inventory-driven.
        let ample =
            size_book(SwapSide::Buy, 100.0, &asks, &[], INVENTORY_ATOMS, 0.25).expect("fills");
        assert_eq!(ample.amount_in, 250_000);
    }

    /// Depth spans every level inside the limit price and stops at the first
    /// one outside it. Both halves matter: a tail take is meant to be able to
    /// reach past the top rung (the issue's "clear several levels" case), while
    /// a rung beyond the slippage bound is not depth the take can have.
    #[test]
    fn depth_spans_levels_inside_the_limit_only() {
        // Reference 1.0 at 1% slippage ⇒ limit 1.01. The 1.005 rung is inside;
        // the 1.02 rung is not.
        let asks = [
            (px(0), 300_000),
            (px(5_000), 300_000),
            (px(20_000), 300_000),
        ];
        let swap =
            size_book(SwapSide::Buy, 100.0, &asks, &[], INVENTORY_ATOMS, 0.25).expect("fills");
        // Inside-the-limit depth in quote atoms: 300_000 @1.0 + 300_000 @1.005.
        let depth = 300_000 + 301_500;
        assert_eq!(swap.amount_in, depth / 4);
        assert!(swap.depth_capped);

        // Counting the 1.02 rung too would inflate depth by ~half again, so
        // this pins the limit-price filter, not just the summing.
        let unbounded = 300_000 + 301_500 + 306_000;
        assert_ne!(swap.amount_in, unbounded / 4);
    }

    /// The whole point: the same sampled notional scales with the book. A 10×
    /// deeper book absorbs a 10× larger take, so the cap needs no retuning per
    /// market or as the maker's inventory grows.
    #[test]
    fn cap_scales_with_book_depth() {
        let thin = size(SwapSide::Buy, 100.0, 1_000_000, 0, 0.25).expect("fills");
        let deep = size(SwapSide::Buy, 100.0, 10_000_000, 0, 0.25).expect("fills");
        assert_eq!(deep.amount_in, thin.amount_in * 10);
    }

    /// The Sell leg is capped symmetrically, against the bids' depth converted
    /// to the base atoms a Sell actually pays in.
    #[test]
    fn sell_is_clamped_against_bid_depth() {
        let swap = size(SwapSide::Sell, 100.0, 0, 1_000_000, 0.25).expect("fills");
        assert_eq!(swap.amount_in, 250_000);
        assert!(swap.depth_capped);
    }

    /// Nothing resting on the taken side means no depth and so no take — the
    /// tick skips the order instead of sending a swap that cannot fill.
    #[test]
    fn empty_side_is_unfillable() {
        assert!(size(SwapSide::Sell, 10.0, 1_000_000, 0, 0.25).is_none());
        assert!(size(SwapSide::Buy, 10.0, 0, 1_000_000, 0.25).is_none());
    }

    /// With the cap disabled the take reverts to its sampled size, so the knob
    /// is a true opt-out (and the pre-cap behavior stays reachable).
    #[test]
    fn disabled_cap_restores_the_sampled_size() {
        let swap = size(SwapSide::Buy, 100.0, 1_000_000, 0, 0.0).expect("fills");
        assert_eq!(swap.amount_in, 100_000_000, "the full sampled $100");
        assert!(!swap.depth_capped);
    }

    /// Atom conversion respects decimals and truncates toward zero.
    #[test]
    fn to_atoms_scales_by_decimals() {
        assert_eq!(to_atoms(1.0, 6), 1_000_000);
        assert_eq!(to_atoms(0.73, 6), 730_000);
        assert_eq!(to_atoms(2.5, 0), 2);
    }

    /// The event-authority PDA matches the program's `[b"__event_authority"]`
    /// seed (the same one the SDK's own adapters derive).
    #[test]
    fn event_authority_is_canonical() {
        assert_eq!(
            event_authority(),
            Pubkey::find_program_address(&[b"__event_authority"], &DROPSET_ID).0
        );
    }

    /// The public clusters are named (and so rejected); any other genesis — a
    /// fresh test-validator's — reads as localnet and passes.
    #[test]
    fn public_clusters_are_named_localnet_passes() {
        assert_eq!(public_cluster(MAINNET_GENESIS), Some("mainnet-beta"));
        assert_eq!(public_cluster(DEVNET_GENESIS), Some("devnet"));
        assert_eq!(public_cluster(TESTNET_GENESIS), Some("testnet"));
        assert_eq!(public_cluster("11111111111111111111111111111111"), None);
    }
}
