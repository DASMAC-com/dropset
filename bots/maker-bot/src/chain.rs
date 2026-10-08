//! On-chain I/O — market discovery, the live vault read, the two quoting-path
//! sends, and the stale-quote kill stamp.
//!
//! Discovery mirrors the TUI (`tui/src/accounts.rs`): scan the program's
//! accounts for the `MarketHeader` discriminator, decode the single localnet
//! market through the slab-layout mirror, and read the pair's mint decimals so
//! inventory can be valued. The instruction builders are the SDK's `quoting`
//! helpers, signed and paid by the leader (its quote-authority is what gates
//! the hot/cold path); on localnet the leader airdrops its own fees.

use anyhow::{anyhow, Context as _, Result};
use dropset_sdk::accounts::MARKET_HEADER_DISCRIMINATOR;
use dropset_sdk::layout::MarketView as SlabView;
use dropset_sdk::price::Price;
use dropset_sdk::quoting::{set_liquidity_profile_ix, set_reference_price_ix, PROFILE_BYTES};
use dropset_sdk::DROPSET_ID;
use dropset_util::decimals::{atoms_ratio_to_human, human_to_atoms_ratio};
use solana_client::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use solana_transaction::Transaction;
use std::time::Duration;

use crate::cluster::Cluster;
use crate::context::{MarketAddrs, VaultSnapshot};

/// Decode scale for a `Price` to a float — `value × 10^9`, matching the SDK's
/// `quoting` module.
const PRICE_SCALE: u64 = 1_000_000_000;

/// SPL Token Mint `decimals` byte offset (after `COption<Pubkey>` authority +
/// `u64` supply).
const MINT_DECIMALS_OFFSET: usize = 44;

/// An `RpcClient` at `confirmed`, pointed at `url`.
pub fn rpc(url: &str) -> RpcClient {
    RpcClient::new_with_timeout_and_commitment(
        url.to_string(),
        Duration::from_secs(10),
        CommitmentConfig::confirmed(),
    )
}

/// The genesis hashes of the three public Solana clusters. Localnet mode
/// refuses all three and mainnet mode requires the first
/// ([`assert_cluster`]). Cross-checked against the Solana docs and the gill /
/// mpl-bubblegum SDKs.
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

/// Abort unless `client` is the cluster this run declared — the positive
/// genesis assertion, in both directions. Call once at startup, before the
/// leader key is loaded and before the first signed send.
///
/// Keyed on the cluster's genesis hash rather than the RPC host, so localnet is
/// allowed on any address (LAN, Docker) yet a port-forward or proxy that
/// tunnels a public cluster through a loopback URL still trips it. Localnet is
/// a *denylist* of the three public hashes, because a test validator mints a
/// fresh genesis per launch and so has no hash to require; mainnet is an
/// exact match.
pub fn assert_cluster(client: &RpcClient, cluster: Cluster) -> Result<()> {
    let genesis = client
        .get_genesis_hash()
        .context("get genesis hash")?
        .to_string();
    genesis_matches(&genesis, cluster)
}

/// [`assert_cluster`]'s decision, pure so both directions are unit-testable
/// without a validator.
fn genesis_matches(genesis: &str, cluster: Cluster) -> Result<()> {
    match (cluster, public_cluster(genesis)) {
        (Cluster::Localnet, Some(public)) => Err(anyhow!(
            "refusing to run in localnet mode against the {public} public cluster \
             (genesis {genesis}): demo mode signs with a committed role key and \
             must run only against a local test validator"
        )),
        (Cluster::Localnet, None) => Ok(()),
        (Cluster::Mainnet, Some("mainnet-beta")) => Ok(()),
        (Cluster::Mainnet, _) => Err(anyhow!(
            "refusing to run in mainnet mode: --rpc answers with genesis \
             {genesis}, not mainnet-beta's {MAINNET_GENESIS}"
        )),
    }
}

/// Airdrop `lamports` to `who` and block until it confirms (localnet faucet).
/// Used to fund the leader's fees, since it pays for its own quoting txns.
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

/// Discover every localnet market in one scan of the program's accounts for the
/// `MarketHeader` discriminator, reading each one's mints, treasuries, and the
/// pair's decimals. The supervisor matches these against the [`MARKETS`] roster
/// by base mint to find each bot's market.
///
/// [`MARKETS`]: crate::config::MARKETS
pub fn discover_markets(client: &RpcClient) -> Result<Vec<MarketAddrs>> {
    let accounts = client
        .get_program_accounts(&DROPSET_ID)
        .context("get_program_accounts")?;
    let mut markets = Vec::new();
    for (address, account) in &accounts {
        if account.data.len() < 8 || account.data[..8] != MARKET_HEADER_DISCRIMINATOR {
            continue;
        }
        // Skip (don't abort the whole scan on) an account that carries the
        // header discriminator but won't decode — one malformed market must not
        // hide the rest of the roster.
        let view = match SlabView::load(&account.data) {
            Ok(view) => view,
            Err(e) => {
                eprintln!("[discover] skipping undecodable market {address}: {e:?}");
                continue;
            }
        };
        let header = view.header;
        let base_mint = Pubkey::new_from_array(header.base_mint);
        let quote_mint = Pubkey::new_from_array(header.quote_mint);
        markets.push(MarketAddrs {
            market: *address,
            base_mint,
            quote_mint,
            base_treasury: Pubkey::new_from_array(header.base_treasury),
            quote_treasury: Pubkey::new_from_array(header.quote_treasury),
            base_decimals: mint_decimals(client, &base_mint).context("base mint decimals")?,
            quote_decimals: mint_decimals(client, &quote_mint).context("quote mint decimals")?,
        });
    }
    Ok(markets)
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

/// Read the bot's vault — the active sector whose quote authority is
/// `authority` (the leader). Matching by authority rather than a hardcoded
/// sector index makes the bot robust to whichever sector the bootstrap
/// happened to open. Reports the active sectors on a miss.
///
/// Note the reference's price-time nonce is deliberately *not* read for fill
/// detection: it bumps on every re-quote (the leader's own
/// `set_reference_price` / `set_liquidity_profile` arm a flush), so a change
/// doesn't imply a taker. The `emit_cpi!` `FillEvent` subscription (`fills`)
/// is the primary fill signal; this read reconciles it and is the fallback.
pub fn read_vault(
    client: &RpcClient,
    market: &Pubkey,
    authority: &Pubkey,
    base_decimals: u8,
    quote_decimals: u8,
) -> Result<VaultSnapshot> {
    let account = client.get_account(market).context("get market account")?;
    let view = SlabView::load(&account.data).map_err(|e| anyhow!("decode market: {e:?}"))?;

    let wanted = authority.to_bytes();
    let mut active = Vec::new();
    for (idx, vault) in view.active_vaults() {
        active.push(idx);
        if vault.quote_authority == wanted {
            let reference = vault.reference_price.price();
            let ratio = reference.quote_for_base(PRICE_SCALE) as f64 / PRICE_SCALE as f64;
            // The stored price is the atoms-ratio; lift it back to the human
            // quote-per-base for the snapshot.
            let reference_price = atoms_ratio_to_human(ratio, base_decimals, quote_decimals);
            return Ok(VaultSnapshot {
                sector_idx: idx,
                base_atoms: vault.base_atoms.get(),
                quote_atoms: vault.quote_atoms.get(),
                reference_price,
                // The program's own matching gate, shared rather than
                // re-derived — the kill stamp's whole effect is writing a price
                // that fails it, so this must not drift from the matcher.
                reference_valid: reference.is_matchable(),
                frozen: vault.frozen != 0,
            });
        }
    }
    Err(anyhow!(
        "no vault with quote authority {authority}; active sectors: {active:?}"
    ))
}

/// Stamp a new reference price (`set_reference_price`, hot path). `slot` is the
/// quote slot; it is not backdated on this path (§3 heartbeat invariant).
#[allow(clippy::too_many_arguments)]
pub fn set_reference_price(
    client: &RpcClient,
    leader: &Keypair,
    market: &Pubkey,
    vault_idx: u32,
    price: f64,
    base_decimals: u8,
    quote_decimals: u8,
    slot: u64,
) -> Result<Sent> {
    // The feeds report a human quote-per-base price; the engine stores the
    // atoms-ratio, so scale by the decimal gap before encoding.
    let ratio = human_to_atoms_ratio(price, base_decimals, quote_decimals);
    let reference = Price::from_value(ratio)
        .ok_or_else(|| anyhow!("price {price} (ratio {ratio}) out of range"))?;
    // Stamp the wall-clock datum every level's TIF is measured from. Host
    // clock (see `dropset_sdk::time`): a fresh quote must reset the whole
    // ladder's life, and paying an RPC for it on every tick of the
    // re-quote hot path buys accuracy the sysvar itself does not have.
    let ix = set_reference_price_ix(
        leader.pubkey(),
        *market,
        vault_idx,
        reference,
        slot,
        dropset_sdk::time::now_unix(),
    );
    send(client, leader, &[ix], 0)
}

/// Kill this vault's resting book by stamping the zero sentinel through the
/// ordinary hot path — the leader-authorized stand-in for the admin-only
/// `FreezeVault` (`model::invalidate`). Matching skips a vault whose reference
/// price fails `has_valid_reference_price()`, and zero fails it, so one cheap
/// instruction takes every level dark at once while leaving the
/// `LiquidityProfile` untouched; the next live [`set_reference_price`] re-arms
/// the same shape.
///
/// Sent with a priority fee (`micro_lamports` per compute unit) because this
/// races takers in the first blocks after the bot returns. `slot` is stamped as
/// usual for consistency, but nothing reads it while the price is invalid — the
/// per-vault gate rejects the vault before any expiry comparison.
pub fn invalidate_reference_price(
    client: &RpcClient,
    leader: &Keypair,
    market: &Pubkey,
    vault_idx: u32,
    slot: u64,
    micro_lamports: u64,
) -> Result<Sent> {
    // The datum is immaterial on this path — a zero reference price fails
    // `has_valid_reference_price()`, so matching skips the vault before any
    // level expiry is consulted — but stamp the real one anyway so the
    // stored record stays honest about when the kill was issued.
    let ix = set_reference_price_ix(
        leader.pubkey(),
        *market,
        vault_idx,
        Price::ZERO,
        slot,
        dropset_sdk::time::now_unix(),
    );
    send(client, leader, &[ix], micro_lamports)
}

/// Rewrite the quote ladder (`set_liquidity_profile`, cold path).
pub fn set_liquidity_profile(
    client: &RpcClient,
    leader: &Keypair,
    market: &Pubkey,
    vault_idx: u32,
    profile_bytes: [u8; PROFILE_BYTES],
) -> Result<Sent> {
    let ix = set_liquidity_profile_ix(leader.pubkey(), *market, vault_idx, profile_bytes);
    send(client, leader, &[ix], 0)
}

/// Current slot, for stamping the reference's `quote_slot`.
pub fn current_slot(client: &RpcClient) -> Result<u64> {
    client.get_slot().context("get_slot")
}

/// A confirmed quote write: its signature and what it cost the leader, the
/// row the quote-burn telemetry records (`maker_quote_writes`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sent {
    pub signature: String,
    /// The per-signature base fee.
    pub base_fee_lamports: u64,
    /// The compute-unit-price surcharge; zero on every path but the kill stamp.
    pub priority_fee_lamports: u64,
}

/// The base fee the runtime charges per transaction signature.
const LAMPORTS_PER_SIGNATURE: u64 = 5_000;
/// The compute-unit limit the runtime assigns when a transaction requests
/// none: 200k per SBF-program instruction, plus 3k per builtin instruction —
/// the compute-unit-price instruction included — per SIMD-0170, capped per
/// transaction. No quote write requests a limit — see
/// `InvalidateConfig::priority_micro_lamports` for why — so its priority fee
/// is priced against these defaults.
const DEFAULT_PROGRAM_IX_COMPUTE_UNITS: u64 = 200_000;
const DEFAULT_BUILTIN_IX_COMPUTE_UNITS: u64 = 3_000;
const MAX_TX_COMPUTE_UNITS: u64 = 1_400_000;

/// What a transaction with `signatures` signers, `program_ixs` SBF-program
/// instructions and `builtin_ixs` builtin ones, at `micro_lamports` per
/// compute unit, is charged: `(base, priority)` lamports.
///
/// Computed from the fee schedule rather than read back with
/// `getTransaction`, which would add an RPC round trip to every write on the
/// quote path. The two agree because the runtime charges the priority fee on
/// the *requested* limit, not the units consumed, and every input is fixed
/// here at send time — the one way they can drift is a fee-schedule change
/// (a new SIMD) this function has not caught up with.
fn fee_burn(
    signatures: usize,
    program_ixs: usize,
    builtin_ixs: usize,
    micro_lamports: u64,
) -> (u64, u64) {
    let base = LAMPORTS_PER_SIGNATURE * signatures as u64;
    let limit = (DEFAULT_PROGRAM_IX_COMPUTE_UNITS * program_ixs as u64
        + DEFAULT_BUILTIN_IX_COMPUTE_UNITS * builtin_ixs as u64)
        .min(MAX_TX_COMPUTE_UNITS);
    let priority = (u128::from(micro_lamports) * u128::from(limit)).div_ceil(1_000_000) as u64;
    (base, priority)
}

/// Sign `ixs` with the leader (fee payer and only signer) and send,
/// confirming at the client's commitment. A non-zero `micro_lamports` prepends
/// the compute-unit price instruction. On failure, re-simulate to recover the
/// program logs a `ClientError` drops for a custom-program error.
fn send(
    client: &RpcClient,
    leader: &Keypair,
    ixs: &[Instruction],
    micro_lamports: u64,
) -> Result<Sent> {
    let mut all = Vec::with_capacity(ixs.len() + 1);
    if micro_lamports > 0 {
        all.push(ComputeBudgetInstruction::set_compute_unit_price(
            micro_lamports,
        ));
    }
    all.extend_from_slice(ixs);
    let blockhash = client.get_latest_blockhash().context("blockhash")?;
    let tx = Transaction::new_signed_with_payer(&all, Some(&leader.pubkey()), &[leader], blockhash);
    match client.send_and_confirm_transaction(&tx) {
        Ok(sig) => {
            let (base_fee_lamports, priority_fee_lamports) = fee_burn(
                tx.signatures.len(),
                ixs.len(),
                all.len() - ixs.len(),
                micro_lamports,
            );
            Ok(Sent {
                signature: sig.to_string(),
                base_fee_lamports,
                priority_fee_lamports,
            })
        }
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

    /// The public clusters are named (and so rejected); any other genesis — a
    /// fresh test-validator's — reads as localnet and passes.
    #[test]
    fn public_clusters_are_named_localnet_passes() {
        assert_eq!(public_cluster(MAINNET_GENESIS), Some("mainnet-beta"));
        assert_eq!(public_cluster(DEVNET_GENESIS), Some("devnet"));
        assert_eq!(public_cluster(TESTNET_GENESIS), Some("testnet"));
        assert_eq!(public_cluster("11111111111111111111111111111111"), None);
    }

    /// The assertion holds in both directions: localnet mode refuses every
    /// public cluster, mainnet mode refuses everything but mainnet-beta —
    /// including a local validator, which is the misconfigured `--rpc` case.
    #[test]
    fn the_genesis_assertion_runs_in_both_directions() {
        let local = "11111111111111111111111111111111";
        assert!(genesis_matches(local, Cluster::Localnet).is_ok());
        assert!(genesis_matches(MAINNET_GENESIS, Cluster::Localnet).is_err());
        assert!(genesis_matches(DEVNET_GENESIS, Cluster::Localnet).is_err());
        assert!(genesis_matches(MAINNET_GENESIS, Cluster::Mainnet).is_ok());
        assert!(genesis_matches(local, Cluster::Mainnet).is_err());
        assert!(genesis_matches(DEVNET_GENESIS, Cluster::Mainnet).is_err());
        assert!(genesis_matches(TESTNET_GENESIS, Cluster::Mainnet).is_err());
    }

    /// One signer, one program instruction: the base fee alone at no priority,
    /// and the kill stamp's surcharge priced on the default limit — 200k for
    /// the program instruction plus 3k for the price instruction itself.
    #[test]
    fn the_fee_burn_follows_the_fee_schedule() {
        assert_eq!(fee_burn(1, 1, 0, 0), (5_000, 0));
        // 100_000 micro-lamports × 203_000 CU / 1e6 = 20_300 lamports.
        assert_eq!(fee_burn(1, 1, 1, 100_000), (5_000, 20_300));
        // A fractional remainder rounds up, as the runtime charges it.
        assert_eq!(fee_burn(1, 1, 1, 1), (5_000, 1));
        // Signers and program instructions both multiply.
        assert_eq!(fee_burn(2, 2, 0, 100_000), (10_000, 40_000));
        // The limit caps at the per-transaction maximum.
        assert_eq!(fee_burn(1, 10, 1, 1_000_000), (5_000, 1_400_000));
    }

    /// The kill stamp's whole effect rests on `Price::ZERO` failing the same
    /// gate the matching engine applies, while every price on the demo roster
    /// passes it — so pin both directions against the roster's actual span
    /// (EURC ~$1.14 down to IDRX ~$0.000056) rather than trusting the sentinel.
    #[test]
    fn the_kill_price_is_unmatchable_and_roster_prices_are_not() {
        assert!(!Price::ZERO.is_matchable());
        for human in [1.14, 1.235, 0.000056] {
            let price = Price::from_value(human).expect("encodable");
            assert!(price.is_matchable(), "{human} should match");
        }
        // `from_value(0.0)` is the sentinel too, so the ordinary encoder can't
        // accidentally produce a live-looking zero.
        assert!(!Price::from_value(0.0).expect("zero encodes").is_matchable());
    }
}
