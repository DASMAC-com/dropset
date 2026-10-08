//! Leader deposit and withdraw — the vault leader moving its own stake in and
//! out after the seed, on either cluster.
//!
//! Split in two halves around the operator's typed confirmation. [`prepare`]
//! reads the leader's vault fresh and sizes a [`Ticket`] with the program's own
//! share arithmetic; the panel shows it and waits for `yes`. [`execute`] then
//! sends **exactly** the confirmed bounds. It re-reads only to refuse when the
//! vault it was confirmed against is gone or recycled, never to re-size — so a
//! vault that moved past those bounds in between is rejected by the program
//! (`BasketSlippage`), not quietly re-priced behind the confirmation.
//!
//! The first deposit into an empty vault is not here: that is the two-leg
//! seed, `Action::Deposit`. A top-up is single-leg by the program's rule, and
//! this module supplies the quote leg and lets the program derive the base.

use crate::accounts::{self, MarketView, VaultSeat, VaultStake};
use crate::chain;
use crate::cluster::Cluster;
use crate::job::Logger;
use crate::market::{self, MintKey, PairConfig};
use anyhow::{bail, Context, Result};
use dropset_math_core::share::{compute_pro_rata_slice, single_leg_basket, BasketError};
use solana_client::rpc_client::RpcClient;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use std::path::Path;

/// Tolerance on every confirmed bound, in bps of the previewed amount: the
/// base cap on a deposit sits this far above the quoted leg, and the floors
/// on a withdraw this far below the quoted slice. It absorbs the
/// performance-fee realize the program runs first (which only mints shares,
/// so it can only shrink a slice) and swaps landing between read and send.
pub const SLIPPAGE_BPS: u64 = 100;

const BPS: u128 = 10_000;

/// Which way the leader's stake moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaderOp {
    Deposit,
    Withdraw,
}

impl LeaderOp {
    pub fn label(self) -> &'static str {
        match self {
            LeaderOp::Deposit => "Leader deposit",
            LeaderOp::Withdraw => "Leader withdraw",
        }
    }

    /// What the amount prompt asks the operator to type.
    pub fn amount_hint(self) -> &'static str {
        match self {
            LeaderOp::Deposit => "whole quote units",
            LeaderOp::Withdraw => "% of the leader's stake (1-100)",
        }
    }
}

/// The sized instruction arguments — what the confirmation shows and what is
/// sent, unchanged. Every amount is in atoms (shares for `shares_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    Deposit {
        quote_in: u64,
        /// The base leg the program will derive, as of the read.
        base_in: u64,
        /// The cap sent as `max_base_in` — `base_in` plus the tolerance.
        max_base_in: u64,
        shares_out: u64,
    },
    Withdraw {
        shares_in: u64,
        base_out: u64,
        quote_out: u64,
        /// The floors sent as `min_*_out` — the slice less the tolerance.
        min_base_out: u64,
        min_quote_out: u64,
    },
}

/// A sized, not-yet-sent leader operation on one vault.
#[derive(Clone, Debug)]
pub struct Ticket {
    pub op: LeaderOp,
    pub market: MarketView,
    pub base_symbol: &'static str,
    pub quote_symbol: &'static str,
    pub seat: VaultSeat,
    pub leader: Pubkey,
    pub plan: Plan,
}

impl Ticket {
    /// The confirmation's body, one line each — what the operator says `yes`
    /// to. Human units beside atoms, so the decimals are not the operator's
    /// mental arithmetic.
    pub fn summary(&self) -> Vec<String> {
        let mut lines = vec![
            format!(
                "market  {}/{}  vault #{}",
                self.base_symbol, self.quote_symbol, self.seat.seq
            ),
            format!("leader  {}", self.leader),
        ];
        lines.extend(plan_lines(
            &self.plan,
            (self.base_symbol, self.market.base_decimals),
            (self.quote_symbol, self.market.quote_decimals),
            self.seat.stake.leader_shares,
        ));
        lines
    }
}

/// The amount lines of a confirmation, separated from [`Ticket::summary`] so
/// the text the operator says `yes` to is testable without a `MarketView`.
/// `base` / `quote` are each `(symbol, decimals)`.
fn plan_lines(
    plan: &Plan,
    (b, bd): (&str, u8),
    (q, qd): (&str, u8),
    leader_shares: u64,
) -> Vec<String> {
    match *plan {
        Plan::Deposit {
            quote_in,
            base_in,
            max_base_in,
            shares_out,
        } => vec![
            format!("in      {} {q}", human(quote_in, qd)),
            format!(
                "        {} {b}  (cap {})",
                human(base_in, bd),
                human(max_base_in, bd)
            ),
            // Not a bound: the instruction carries no `min_shares_out`, so
            // this is the read's figure, and the program mints what the
            // vault's ratio gives at send time.
            format!("shares  ~+{shares_out} (estimate)"),
        ],
        Plan::Withdraw {
            shares_in,
            base_out,
            quote_out,
            min_base_out,
            min_quote_out,
        } => vec![
            format!("shares  -{shares_in} of {leader_shares}"),
            format!(
                "out     {} {b}  (min {})",
                human(base_out, bd),
                human(min_base_out, bd)
            ),
            format!(
                "        {} {q}  (min {})",
                human(quote_out, qd),
                human(min_quote_out, qd)
            ),
            // Two program rules the read cannot settle: fee shares minted by
            // the realize that runs first stay behind even at 100%, and an
            // active vault refuses a draw below the min-leader-share floor.
            "note    pre-fee-realize; the min-leader-share floor may refuse".to_string(),
        ],
    }
}

/// `atoms` at `decimals`, as a decimal string — exact, no float. A decimals
/// count past `u64`'s range (no real mint) falls back to raw atoms rather
/// than panicking in the middle of drawing the confirmation.
fn human(atoms: u64, decimals: u8) -> String {
    let Some(scale) = 10u64.checked_pow(u32::from(decimals)) else {
        return format!("{atoms} atoms");
    };
    let (whole, frac) = (atoms / scale, atoms % scale);
    if decimals == 0 {
        whole.to_string()
    } else {
        format!("{whole}.{frac:0width$}", width = usize::from(decimals))
    }
}

fn less_tolerance(x: u64) -> u64 {
    (u128::from(x) * (BPS - u128::from(SLIPPAGE_BPS)) / BPS) as u64
}

fn plus_tolerance(x: u64) -> u64 {
    let bump = (u128::from(x) * u128::from(SLIPPAGE_BPS)).div_ceil(BPS);
    (u128::from(x) + bump).min(u128::from(u64::MAX)) as u64
}

/// Size a quote-leg top-up of `quote_in` atoms against `stake`.
pub fn plan_deposit(stake: &VaultStake, quote_in: u64) -> Result<Plan> {
    if stake.total_shares == 0 {
        bail!("the vault is unseeded — its first deposit is the seed, not a top-up");
    }
    if quote_in == 0 {
        bail!("deposit at least one quote atom");
    }
    // `single_leg_basket` divides by the leg's inventory unguarded, and a
    // taker sell can drain a seeded vault's quote leg to exactly zero — so
    // this refusal is what stops a panic on the event loop.
    if stake.quote_atoms == 0 {
        bail!("the vault holds no quote — a quote-leg top-up cannot size against it");
    }
    let (shares_out, base_in, quote_final) = single_leg_basket(
        stake.total_shares,
        stake.base_atoms,
        stake.quote_atoms,
        0,
        quote_in,
        u64::MAX,
        quote_in,
    )
    .map_err(|e| match e {
        // Zero shares out reports as overflow; say what it means here.
        BasketError::MathOverflow => anyhow::anyhow!("the amount is too small to buy one share"),
        other => anyhow::anyhow!("the deposit does not size: {other:?}"),
    })?;
    Ok(Plan::Deposit {
        quote_in: quote_final,
        base_in,
        max_base_in: plus_tolerance(base_in),
        shares_out,
    })
}

/// Size a withdraw of `percent` (1–100) of the leader's own shares.
pub fn plan_withdraw(stake: &VaultStake, percent: u64) -> Result<Plan> {
    if !(1..=100).contains(&percent) {
        bail!("withdraw between 1 and 100 percent of the stake");
    }
    if stake.leader_shares == 0 || stake.total_shares == 0 {
        bail!("the leader holds no shares in this vault");
    }
    let shares_in = (u128::from(stake.leader_shares) * u128::from(percent) / 100) as u64;
    if shares_in == 0 {
        bail!("{percent}% of the stake rounds to zero shares");
    }
    let (base_out, quote_out) = compute_pro_rata_slice(
        shares_in,
        stake.total_shares,
        stake.base_atoms,
        stake.quote_atoms,
    );
    Ok(Plan::Withdraw {
        shares_in,
        base_out,
        quote_out,
        min_base_out: less_tolerance(base_out),
        min_quote_out: less_tolerance(quote_out),
    })
}

/// The one vault `leader` leads on `market`, read fresh — refusing none or
/// several, since guessing which stake to move is not this command's call.
fn led_seat(client: &RpcClient, market: &Pubkey, leader: &Pubkey) -> Result<VaultSeat> {
    let seats = accounts::vault_seats_fresh(client, market)?.context("market not found")?;
    match accounts::seats_led_by(&seats, leader)[..] {
        [seat] => Ok(seat),
        [] => bail!("leader {leader} leads no vault on this market"),
        ref many => bail!(
            "leader {leader} leads {} vaults on this market — refusing to guess",
            many.len()
        ),
    }
}

/// Read the leader's vault on `market` fresh and size `op` for `amount` —
/// whole quote units on a deposit, a percentage on a withdraw. Sends nothing.
pub fn prepare(
    client: &RpcClient,
    repo_root: &Path,
    cluster: Cluster,
    market: &MarketView,
    leader: &Pubkey,
    op: LeaderOp,
    amount: u64,
) -> Result<Ticket> {
    let config = market::config_for(repo_root, cluster, &market.base_mint)
        .context("the selected market is not in this cluster's roster")?;
    let seat = led_seat(client, &market.address, leader)?;
    let plan = match op {
        LeaderOp::Deposit => {
            let atoms = 10u64
                .checked_pow(u32::from(market.quote_decimals))
                .and_then(|scale| amount.checked_mul(scale))
                .context("amount overflows the quote mint's atoms")?;
            plan_deposit(&seat.stake, atoms)?
        }
        LeaderOp::Withdraw => plan_withdraw(&seat.stake, amount)?,
    };
    Ok(Ticket {
        op,
        market: market.clone(),
        base_symbol: config.base.symbol,
        quote_symbol: config.quote.symbol,
        seat,
        leader: *leader,
        plan,
    })
}

/// Send the confirmed `ticket`, signed by `leader` and paid by `wallet`.
///
/// Refuses, nothing sent, when the vault is no longer the one confirmed (its
/// sector recycled to another vault, or it is gone). On a deposit, the legs
/// must be in the leader's ATAs: localnet's mock mints are topped up to the
/// confirmed bounds (the admin is their authority); a real mint is checked,
/// never minted, so an underfunded leader is refused before the send.
pub fn execute(
    client: &RpcClient,
    wallet: &Keypair,
    leader: &Keypair,
    repo_root: &Path,
    cluster: Cluster,
    ticket: &Ticket,
    log: &Logger,
) -> Result<String> {
    if leader.pubkey() != ticket.leader {
        bail!("the signing leader is not the one confirmed — nothing was sent");
    }
    let now = led_seat(client, &ticket.market.address, &ticket.leader)?;
    if (now.idx, now.seq) != (ticket.seat.idx, ticket.seat.seq) {
        bail!(
            "vault #{} is no longer at sector {} — nothing was sent; re-run to re-size",
            ticket.seat.seq,
            ticket.seat.idx
        );
    }
    let m = &ticket.market;
    let ix = match ticket.plan {
        Plan::Deposit {
            quote_in,
            max_base_in,
            ..
        } => {
            fund_leader(
                client,
                wallet,
                leader,
                repo_root,
                cluster,
                ticket,
                max_base_in,
                quote_in,
                log,
            )?;
            chain::build_deposit_leader_ix(
                &ticket.leader,
                &m.address,
                &m.base_mint,
                &m.quote_mint,
                &m.base_treasury,
                &m.quote_treasury,
                ticket.seat.idx,
                (0, quote_in),
                (max_base_in, quote_in),
            )
        }
        Plan::Withdraw {
            shares_in,
            min_base_out,
            min_quote_out,
            ..
        } => chain::build_withdraw_leader_ix(
            &ticket.leader,
            &m.address,
            &m.base_mint,
            &m.quote_mint,
            &m.base_treasury,
            &m.quote_treasury,
            ticket.seat.idx,
            shares_in,
            min_base_out,
            min_quote_out,
        ),
    };
    // The program reads both leader ATAs as existing accounts and creates
    // neither, so bundle the idempotent creates into the same transaction.
    let mut ixs: Vec<_> = [&m.base_mint, &m.quote_mint]
        .into_iter()
        .map(|mint| {
            chain::ata_with_create_ix(
                &wallet.pubkey(),
                &ticket.leader,
                mint,
                &chain::SPL_TOKEN_PROGRAM_ID,
            )
            .1
        })
        .collect();
    ixs.push(ix);
    let label = match ticket.op {
        LeaderOp::Deposit => "deposit_leader",
        LeaderOp::Withdraw => "withdraw_leader",
    };
    chain::send_logged(client, wallet, &[wallet, leader], &ixs, label, log).context(label)?;
    log.accounts_changed();
    Ok(format!(
        "{} on {} vault #{} landed",
        ticket.op.label(),
        ticket.base_symbol,
        ticket.seat.seq
    ))
}

/// Put `(base, quote)` atoms in the leader's ATAs for a deposit, or refuse.
#[allow(clippy::too_many_arguments)]
fn fund_leader(
    client: &RpcClient,
    wallet: &Keypair,
    leader: &Keypair,
    repo_root: &Path,
    cluster: Cluster,
    ticket: &Ticket,
    base: u64,
    quote: u64,
    log: &Logger,
) -> Result<()> {
    let config = market::config_for(repo_root, cluster, &ticket.market.base_mint)
        .context("the market left the roster")?;
    let can_mint = can_mint(cluster, config);
    for (mint, need, symbol) in [
        (&ticket.market.base_mint, base, ticket.base_symbol),
        (&ticket.market.quote_mint, quote, ticket.quote_symbol),
    ] {
        let held = chain::token_balance(client, &leader.pubkey(), mint)
            .with_context(|| format!("read the leader's {symbol} balance"))?;
        if held >= need {
            continue;
        }
        if !can_mint {
            bail!(
                "leader {} holds {held} {symbol} atoms, the deposit may need {need} — \
                 fund it first; nothing was sent",
                leader.pubkey()
            );
        }
        let ata = chain::create_ata_idempotent(client, wallet, &leader.pubkey(), mint)
            .with_context(|| format!("leader {symbol} ATA"))?;
        chain::mint_to(client, wallet, mint, &ata, need - held)
            .with_context(|| format!("mint {symbol} to the leader"))?;
        log.log(format!(
            "minted {} {symbol} atoms to the leader",
            need - held
        ));
        // Balances moved even if the deposit then fails to send.
        log.accounts_changed();
    }
    Ok(())
}

/// Whether a deposit may mint the shortfall instead of refusing: only on
/// localnet, and only when **both** legs are mock mints the admin wallet
/// is authority over. A real mint is never minted, on any cluster.
fn can_mint(cluster: Cluster, config: &PairConfig) -> bool {
    let mock = |key: &MintKey| matches!(key, MintKey::Keypair(_));
    !cluster.is_mainnet() && mock(&config.base.key) && mock(&config.quote.key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stake(total: u64, leader: u64, base: u64, quote: u64) -> VaultStake {
        VaultStake {
            total_shares: total,
            leader_shares: leader,
            base_atoms: base,
            quote_atoms: quote,
        }
    }

    #[test]
    fn deposit_quotes_the_base_leg_and_caps_it_above() {
        // 1:2 base:quote vault; 200 quote buys 100 shares and 100 base.
        let plan = plan_deposit(&stake(1_000, 1_000, 1_000, 2_000), 200).unwrap();
        assert_eq!(
            plan,
            Plan::Deposit {
                quote_in: 200,
                base_in: 100,
                max_base_in: 101,
                shares_out: 100,
            }
        );
    }

    #[test]
    fn deposit_refuses_an_unseeded_vault_and_a_zero_amount() {
        assert!(plan_deposit(&stake(0, 0, 0, 0), 100).is_err());
        assert!(plan_deposit(&stake(1_000, 1_000, 1_000, 2_000), 0).is_err());
    }

    #[test]
    fn withdraw_takes_a_share_of_the_leaders_stake_with_floors_below() {
        // Leader holds 600 of 1,000 shares; half of that is 300 shares,
        // a 30% slice of each leg.
        let plan = plan_withdraw(&stake(1_000, 600, 10_000, 20_000), 50).unwrap();
        assert_eq!(
            plan,
            Plan::Withdraw {
                shares_in: 300,
                base_out: 3_000,
                quote_out: 6_000,
                min_base_out: 2_970,
                min_quote_out: 5_940,
            }
        );
    }

    #[test]
    fn withdraw_refuses_out_of_range_percent_and_an_empty_stake() {
        let s = stake(1_000, 600, 10_000, 20_000);
        assert!(plan_withdraw(&s, 0).is_err());
        assert!(plan_withdraw(&s, 101).is_err());
        assert!(plan_withdraw(&stake(1_000, 0, 10_000, 20_000), 50).is_err());
        // 1% of a single share floors to zero shares.
        assert!(plan_withdraw(&stake(10, 1, 10, 10), 1).is_err());
    }

    #[test]
    fn tolerance_never_crosses_its_bound() {
        assert_eq!(less_tolerance(0), 0);
        assert_eq!(plus_tolerance(0), 0);
        // A one-atom bound still moves the cap up, by rounding up.
        assert_eq!(plus_tolerance(1), 2);
        assert_eq!(plus_tolerance(u64::MAX), u64::MAX);
    }

    #[test]
    fn human_renders_atoms_exactly() {
        assert_eq!(human(1_234_567, 6), "1.234567");
        assert_eq!(human(5, 6), "0.000005");
        assert_eq!(human(42, 0), "42");
        // Past u64's decimal range: raw atoms, never a panic mid-draw.
        assert_eq!(human(7, 20), "7 atoms");
    }

    #[test]
    fn deposit_refuses_a_vault_with_no_quote_and_a_sub_share_amount() {
        // Quote drained to zero by a taker sell: refuse, never divide by it.
        let err = plan_deposit(&stake(1_000, 1_000, 1_000, 0), 100).unwrap_err();
        assert!(format!("{err}").contains("no quote"), "{err}");
        // One quote atom against a deep vault buys zero shares.
        let err = plan_deposit(&stake(1, 1, 1_000, 1_000_000), 1).unwrap_err();
        assert!(format!("{err}").contains("too small"), "{err}");
    }

    #[test]
    fn only_localnet_mock_pairs_may_mint() {
        let mock = market::PAIRS[0];
        let real = market::MAINNET_PAIRS[0];
        assert!(can_mint(Cluster::Localnet, mock));
        // Mainnet never mints, whatever the pair says.
        assert!(!can_mint(Cluster::Mainnet, mock));
        assert!(!can_mint(Cluster::Mainnet, real));
        // A real mint is never minted, even on a localnet session.
        assert!(!can_mint(Cluster::Localnet, real));
    }

    #[test]
    fn the_confirmation_shows_the_bounds_that_are_sent() {
        let deposit = Plan::Deposit {
            quote_in: 2_500_000,
            base_in: 1_000_000,
            max_base_in: 1_010_000,
            shares_out: 42,
        };
        assert_eq!(
            plan_lines(&deposit, ("EURC", 6), ("USDC", 6), 0),
            [
                "in      2.500000 USDC",
                "        1.000000 EURC  (cap 1.010000)",
                "shares  ~+42 (estimate)",
            ]
        );
        let withdraw = Plan::Withdraw {
            shares_in: 300,
            base_out: 3_000_000,
            quote_out: 6_000_000,
            min_base_out: 2_970_000,
            min_quote_out: 5_940_000,
        };
        let lines = plan_lines(&withdraw, ("EURC", 6), ("USDC", 6), 600);
        assert_eq!(lines[0], "shares  -300 of 600");
        // Base and quote each pair the expected slice with its own floor.
        assert_eq!(lines[1], "out     3.000000 EURC  (min 2.970000)");
        assert_eq!(lines[2], "        6.000000 USDC  (min 5.940000)");
    }
}
