//! Teardown / rent reclamation — the single source of truth.
//!
//! [`run`] reclaims every rent-bearing **application** artifact that currently
//! exists, in the spec's prescribed dependency order (architecture.md §
//! Account lifecycle and rent reclamation → Teardown ordering): a live market
//! is drained and closed first (per-depositor `force_withdraw_depositor` →
//! per-leader `force_withdraw_leader` → per-leg `close_market_treasury` →
//! `close_market`), then the registry fee vault and registry. The same `run`
//! is driven by the TUI's "Teardown & reclaim" action and by the headless
//! `dropset-teardown` binary, so there is one implementation, not two that can
//! drift.

use crate::accounts::{self, MarketView};
use crate::chain;
use crate::job::Logger;
use anyhow::{Context, Result};
use solana_client::rpc_client::RpcClient;
use solana_keypair::Keypair;
use solana_native_token::LAMPORTS_PER_SOL;
use solana_pubkey::Pubkey;
use solana_signer::Signer;

/// Reclaim every rent-bearing **application** artifact that currently exists,
/// in dependency order — so it works at any phase from `RegistryAbsent` on. A
/// live market is drained and closed first (per-depositor
/// `force_withdraw_depositor` → per-leader `force_withdraw_leader` → per-leg
/// `close_market_treasury` → `close_market`), then the registry fee vault and
/// registry. All rent is refunded to the wallet; returns the lamports delta in
/// the summary. Each layer is guarded by existence, so a partial bootstrap
/// tears down cleanly.
///
/// The **program itself is left deployed and upgradeable** — teardown resets
/// only on-chain state, never the program. Closing a program bricks its id on
/// the ledger forever (a fresh deploy would need a wiped validator), whereas
/// keeping it mirrors the mainnet workflow: the program keeps its address and
/// new logic is shipped by upgrading in place. After teardown the phase reads
/// `RegistryAbsent`, so `init` runs straight away against the still-deployed
/// program. Use the TUI's `Wipe` for a true clean slate.
pub fn run(client: &RpcClient, wallet: &Keypair, log: &Logger) -> Result<String> {
    let admin = wallet.pubkey();
    let state = accounts::poll(client, &admin, None);
    let before = client.get_balance(&admin).unwrap_or(0);

    // The `rent_recipient` of every close instruction must differ from the
    // admin signer (anchor-v2's duplicate-mutable-account rule), so reclaimed
    // rent is routed to an ephemeral sink and swept back to the wallet at the
    // end.
    let sink = Keypair::new();
    let sink_key = sink.pubkey();

    match &state.market {
        None => log.log("No market to tear down."),
        Some(market) => teardown_market(client, wallet, market, &sink_key, log)?,
    }

    match &state.registry {
        None => log.log("No registry to tear down."),
        Some(registry) => {
            log.log("close_registry_fee_vault");
            let ix = chain::build_close_registry_fee_vault_ix(
                &admin,
                &registry.fee_mint,
                &registry.fee_token_program,
                &sink_key,
            );
            let sig = chain::send(client, wallet, &[wallet], &[ix])
                .context("close_registry_fee_vault")?;
            log.log(format!("  {sig}"));

            log.log("close_registry");
            let ix = chain::build_close_registry_ix(&admin, &sink_key);
            let sig = chain::send(client, wallet, &[wallet], &[ix]).context("close_registry")?;
            log.log(format!("  {sig}"));
            log.accounts_changed();
        }
    }

    // Sweep the reclaimed rent from the sink back to the wallet.
    let sink_balance = client.get_balance(&sink_key).unwrap_or(0);
    if sink_balance > 0 {
        let ix = chain::system_transfer_ix(&sink_key, &admin, sink_balance);
        let sig =
            chain::send(client, wallet, &[wallet, &sink], &[ix]).context("sweep rent sink")?;
        log.log(format!(
            "swept {sink_balance} lamports from rent sink: {sig}"
        ));
    }

    let after = client.get_balance(&admin).unwrap_or(0);
    let reclaimed = after.saturating_sub(before);
    Ok(format!(
        "Teardown complete — reclaimed {:.4} SOL in rent (program left deployed)",
        reclaimed as f64 / LAMPORTS_PER_SOL as f64
    ))
}

/// Drain and close a live market: depositors → leaders → treasuries →
/// `close_market`, sending reclaimed rent to `rent_recipient`.
fn teardown_market(
    client: &RpcClient,
    wallet: &Keypair,
    market: &MarketView,
    rent_recipient: &Pubkey,
    log: &Logger,
) -> Result<()> {
    let admin = wallet.pubkey();
    for (sector, owner) in &market.depositors {
        log.log(format!(
            "force_withdraw_depositor sector {sector} owner {owner}"
        ));
        let ix = chain::build_force_withdraw_depositor_ix(
            &admin,
            &market.address,
            &market.base_mint,
            &market.quote_mint,
            &market.base_treasury,
            &market.quote_treasury,
            *sector,
            owner,
        );
        let sig = chain::send(client, wallet, &[wallet], &[ix])
            .with_context(|| format!("force_withdraw_depositor sector {sector}"))?;
        log.log(format!("  {sig}"));
        log.accounts_changed();
    }

    if market.live_vaults.is_empty() {
        log.log("No live vaults to drain.");
    }
    for (sector, leader) in &market.live_vaults {
        log.log(format!(
            "force_withdraw_leader sector {sector} leader {leader}"
        ));
        let ix = chain::build_force_withdraw_leader_ix(
            &admin,
            &market.address,
            &market.base_mint,
            &market.quote_mint,
            &market.base_treasury,
            &market.quote_treasury,
            *sector,
            leader,
        );
        let sig = chain::send(client, wallet, &[wallet], &[ix])
            .with_context(|| format!("force_withdraw_leader sector {sector}"))?;
        log.log(format!("  {sig}"));
        log.accounts_changed();
    }

    for (leg, mint, treasury) in [
        ("base", market.base_mint, market.base_treasury),
        ("quote", market.quote_mint, market.quote_treasury),
    ] {
        log.log(format!("close_market_treasury ({leg})"));
        let ix = chain::build_close_market_treasury_ix(
            &admin,
            &market.address,
            &mint,
            &treasury,
            rent_recipient,
        );
        let sig = chain::send(client, wallet, &[wallet], &[ix])
            .with_context(|| format!("close_market_treasury {leg}"))?;
        log.log(format!("  {sig}"));
    }

    log.log("close_market");
    let ix = chain::build_close_market_ix(
        &admin,
        &market.address,
        &market.base_treasury,
        &market.quote_treasury,
        rent_recipient,
    );
    let sig = chain::send(client, wallet, &[wallet], &[ix]).context("close_market")?;
    log.log(format!("  {sig}"));
    log.accounts_changed();
    Ok(())
}
