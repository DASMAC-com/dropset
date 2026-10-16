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
use dropset_math_core::share::{
    compute_pro_rata_slice, min_leader_share_ok, single_leg_basket, BasketError,
};
use solana_client::rpc_client::RpcClient;
use solana_instruction::Instruction;
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
/// Parts per million — the unit of the min-leader-share floor.
const PPM: u128 = 1_000_000;

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
            // Two things the read cannot settle: fee shares minted by the
            // realize that runs first stay behind even at 100%, and the
            // min-leader-share floor — checked when the ticket was sized,
            // against a realize mirrored from this read — is judged again at
            // send, after whatever swaps and deposits land in between.
            "note    fee shares minted at send stay behind even at 100%".to_string(),
            "        the program re-checks the leader-share floor".to_string(),
        ],
    }
}

/// `atoms` at `decimals`, as a decimal string — exact, no float. A decimals
/// count past `u64`'s range (no real mint) falls back to raw atoms rather
/// than panicking in the middle of drawing the confirmation.
pub(crate) fn human(atoms: u64, decimals: u8) -> String {
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
    // A taker sell can drain a seeded vault's quote leg to exactly zero; the
    // kernel refuses that as `EmptyLeg` rather than dividing by it.
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
        BasketError::ZeroShares => anyhow::anyhow!("the amount is too small to buy one share"),
        BasketError::EmptyLeg => anyhow::anyhow!("the vault holds no quote to price this against"),
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
///
/// `shares_in` is a share of the stake **as read**, so a 100% draw never
/// asks for more than the leader held; the slice and the floor are judged
/// against [`VaultStake::realized`], the state the program sees after the
/// realize it runs first — which mints fee shares to the leader, so judging
/// the floor pre-realize would refuse draws the program accepts.
pub fn plan_withdraw(stake: &VaultStake, percent: u64) -> Result<Plan> {
    if !(1..=100).contains(&percent) {
        bail!("withdraw between 1 and 100 percent of the stake");
    }
    if stake.leader_shares == 0 || stake.total_shares == 0 {
        bail!("the leader holds no shares in this vault");
    }
    let shares_in = shares_for(stake, percent);
    if shares_in == 0 {
        bail!("{percent}% of the stake rounds to zero shares");
    }
    let realized = stake.realized();
    if !leaves_floor(&realized, shares_in) {
        match max_withdraw_percent(stake, &realized) {
            Some(max) => bail!(
                "{percent}% leaves the leader under the {} floor — at most {max}%",
                ppm_percent(stake.min_leader_share)
            ),
            None => bail!(
                "the leader is at the {} floor — no withdraw fits",
                ppm_percent(stake.min_leader_share)
            ),
        }
    }
    let (base_out, quote_out) = compute_pro_rata_slice(
        shares_in,
        realized.total_shares,
        realized.base_atoms,
        realized.quote_atoms,
    );
    Ok(Plan::Withdraw {
        shares_in,
        base_out,
        quote_out,
        min_base_out: less_tolerance(base_out),
        min_quote_out: less_tolerance(quote_out),
    })
}

/// `percent` of the leader's shares, floored.
fn shares_for(stake: &VaultStake, percent: u64) -> u64 {
    (u128::from(stake.leader_shares) * u128::from(percent) / 100) as u64
}

/// Whether burning `shares_in` of the leader's shares keeps the program's
/// skin-in-the-game floor — the `withdraw_leader` check, mirrored so the
/// prompt refuses before `yes` rather than the program after it. Waived on a
/// winding-down vault, and for a draw that empties the vault outright.
/// `stake` is the post-realize state ([`VaultStake::realized`]), as the
/// program judges it.
fn leaves_floor(stake: &VaultStake, shares_in: u64) -> bool {
    let new_total = stake.total_shares.saturating_sub(shares_in);
    stake.winding_down
        || new_total == 0
        || min_leader_share_ok(
            stake.leader_shares.saturating_sub(shares_in),
            new_total,
            stake.min_leader_share,
        )
}

/// The largest whole percentage of the stake as read that the floor allows
/// against `realized`, or `None` when none does.
fn max_withdraw_percent(stake: &VaultStake, realized: &VaultStake) -> Option<u64> {
    (1..=100).rev().find(|&p| {
        let shares = shares_for(stake, p);
        shares > 0 && leaves_floor(realized, shares)
    })
}

/// A ppm share as a two-place percentage — exact, no float.
fn ppm_percent(ppm: u32) -> String {
    format!("{}.{:02}%", ppm / 10_000, (ppm % 10_000) / 100)
}

/// One mint's display facts: its ticker and decimals.
pub type Leg<'a> = (&'a str, u8);

/// The quote-terms value of `(base, quote)` atoms at `price` (human quote
/// per base), or `None` without a price. A float, and labelled `≈` wherever
/// shown: it is a reading aid, never a bound.
fn value_in_quote(
    base: u64,
    quote: u64,
    (_, bd): Leg,
    (_, qd): Leg,
    price: Option<f64>,
) -> Option<f64> {
    let p = price?;
    let scale = |atoms: u64, d: u8| atoms as f64 / 10f64.powi(i32::from(d));
    Some(scale(base, bd) * p + scale(quote, qd))
}

/// `base + quote`, in human units, with the `≈` quote total when priced.
fn basket_text(base: u64, quote: u64, b: Leg, q: Leg, price: Option<f64>) -> String {
    let mut s = format!(
        "{} {} + {} {}",
        human(base, b.1),
        b.0,
        human(quote, q.1),
        q.0
    );
    if let Some(v) = value_in_quote(base, quote, b, q, price) {
        s.push_str(&format!("  \u{2248} {v:.2} {}", q.0));
    }
    s
}

/// What the leader holds in `seat`'s vault, for the prompt header and the
/// leader pane, as labelled `(label, value)` rows: its number, shares and
/// share of the vault beside the floor, and the leader's pro-rata slice of
/// the inventory, valued at `price`. The share is in exact ppm, rendered
/// like the floor, so the two never round to different sides of each other.
pub fn holding_lines(
    seq: u64,
    stake: &VaultStake,
    b: Leg,
    q: Leg,
    price: Option<f64>,
) -> [(&'static str, String); 2] {
    let share_ppm = if stake.total_shares == 0 {
        0
    } else {
        (u128::from(stake.leader_shares) * PPM / u128::from(stake.total_shares)) as u32
    };
    let (base, quote) = compute_pro_rata_slice(
        stake.leader_shares,
        stake.total_shares,
        stake.base_atoms,
        stake.quote_atoms,
    );
    [
        (
            "vault",
            format!(
                "#{seq} · shares {} of {} ({}) · floor {}",
                stake.leader_shares,
                stake.total_shares,
                ppm_percent(share_ppm),
                ppm_percent(stake.min_leader_share)
            ),
        ),
        ("stake", basket_text(base, quote, b, q, price)),
    ]
}

/// The live preview under the amount being typed, sized with the same
/// planners against the poll's `stake` — `Err` carries the refusal, to show
/// in place of the amounts. `amount` is `None` while the buffer is empty.
pub fn preview(
    op: LeaderOp,
    stake: &VaultStake,
    amount: Option<u64>,
    b: Leg,
    q: Leg,
    price: Option<f64>,
) -> Option<Result<Vec<String>, String>> {
    let amount = amount?;
    let planned = match op {
        LeaderOp::Deposit => 10u64
            .checked_pow(u32::from(q.1))
            .and_then(|scale| amount.checked_mul(scale))
            .ok_or_else(|| anyhow::anyhow!("amount overflows the quote mint's atoms"))
            .and_then(|atoms| plan_deposit(stake, atoms)),
        LeaderOp::Withdraw => plan_withdraw(stake, amount),
    };
    Some(match planned {
        Ok(Plan::Deposit {
            quote_in,
            base_in,
            shares_out,
            ..
        }) => Ok(vec![format!(
            "preview  +{}, ~+{shares_out} shares",
            basket_text(base_in, quote_in, b, q, price)
        )]),
        Ok(Plan::Withdraw {
            shares_in,
            base_out,
            quote_out,
            ..
        }) => Ok(vec![
            format!("preview  -{shares_in} shares"),
            format!("         {}", basket_text(base_out, quote_out, b, q, price)),
        ]),
        Err(e) => Err(format!("{e:#}")),
    })
}

/// The one vault `leader` leads on `market`, read fresh — refusing none or
/// several, since guessing which stake to move is not this command's call.
fn led_seat(
    client: &RpcClient,
    cluster: Cluster,
    market: &Pubkey,
    leader: &Pubkey,
) -> Result<VaultSeat> {
    let seats =
        accounts::vault_seats_fresh(client, cluster, market)?.context("market not found")?;
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
    let seat = led_seat(client, cluster, &market.address, leader)?;
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
    let now = led_seat(client, cluster, &ticket.market.address, &ticket.leader)?;
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
                cluster,
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
            cluster,
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
    let mut ixs = leader_ata_creates(
        &wallet.pubkey(),
        &ticket.leader,
        &m.base_mint,
        &m.quote_mint,
    );
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

/// The idempotent creates for `leader`'s two ATAs, paid by `payer`. The
/// program reads both as existing accounts on a leader deposit or withdraw
/// and creates neither, so they ride in front of it in the same transaction.
pub(crate) fn leader_ata_creates(
    payer: &Pubkey,
    leader: &Pubkey,
    base_mint: &Pubkey,
    quote_mint: &Pubkey,
) -> Vec<Instruction> {
    [base_mint, quote_mint]
        .into_iter()
        .map(|mint| chain::ata_with_create_ix(payer, leader, mint, &chain::SPL_TOKEN_PROGRAM_ID).1)
        .collect()
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
            ..VaultStake::default()
        }
    }

    #[test]
    fn withdraw_refuses_a_draw_under_the_floor_and_names_the_most_allowed() {
        // Leader holds 600 of 1,000 with a 50% floor: burning s leaves
        // (600 - s) / (1,000 - s) >= 1/2, so s <= 200 — a third of the stake.
        let s = VaultStake {
            min_leader_share: 500_000,
            ..stake(1_000, 600, 10_000, 20_000)
        };
        assert!(plan_withdraw(&s, 33).is_ok());
        let err = plan_withdraw(&s, 34).unwrap_err();
        assert_eq!(
            format!("{err}"),
            "34% leaves the leader under the 50.00% floor — at most 33%"
        );
        // Winding down waives it, as the program does.
        let s = VaultStake {
            winding_down: true,
            ..s
        };
        assert!(plan_withdraw(&s, 100).is_ok());
        // A sole holder's full exit empties the vault and is always allowed.
        let sole = VaultStake {
            min_leader_share: 500_000,
            ..stake(1_000, 1_000, 10_000, 20_000)
        };
        assert!(plan_withdraw(&sole, 100).is_ok());
    }

    #[test]
    fn the_preview_sizes_as_typed_and_shows_a_refusal_in_place() {
        let s = stake(1_000, 600, 10_000_000, 20_000_000);
        let (b, q) = (("EURC", 6), ("USDC", 6));
        assert_eq!(preview(LeaderOp::Withdraw, &s, None, b, q, None), None);
        assert_eq!(
            preview(LeaderOp::Withdraw, &s, Some(50), b, q, Some(1.5)),
            Some(Ok(vec![
                "preview  -300 shares".to_string(),
                "         3.000000 EURC + 6.000000 USDC  \u{2248} 10.50 USDC".to_string(),
            ]))
        );
        let refused = preview(LeaderOp::Withdraw, &s, Some(101), b, q, None);
        assert!(matches!(refused, Some(Err(e)) if e.contains("between 1 and 100")));
    }

    #[test]
    fn holdings_show_the_leaders_slice_and_its_value() {
        let s = VaultStake {
            min_leader_share: 200_000,
            ..stake(1_000, 600, 10_000_000, 20_000_000)
        };
        assert_eq!(
            holding_lines(3, &s, ("EURC", 6), ("USDC", 6), Some(2.0)),
            [
                (
                    "vault",
                    "#3 · shares 600 of 1000 (60.00%) · floor 20.00%".to_string()
                ),
                (
                    "stake",
                    "6.000000 EURC + 12.000000 USDC  \u{2248} 24.00 USDC".to_string()
                ),
            ]
        );
        // Unpriced: the amounts alone, no `≈` total.
        let [_, (_, stake_row)] = holding_lines(3, &s, ("EURC", 6), ("USDC", 6), None);
        assert_eq!(stake_row, "6.000000 EURC + 12.000000 USDC");
    }

    #[test]
    fn the_share_truncates_like_the_floor_beside_it() {
        // 9,999 of 100,000 shares is 99,990 ppm against a 99,999 ppm floor:
        // under it, and a float `{:.2}` would round it up to read 10.00%
        // beside a floor of 9.99%.
        let s = VaultStake {
            min_leader_share: 99_999,
            ..stake(100_000, 9_999, 1, 1)
        };
        let [(_, vault_row), _] = holding_lines(1, &s, ("EURC", 6), ("USDC", 6), None);
        assert_eq!(
            vault_row,
            "#1 · shares 9999 of 100000 (9.99%) · floor 9.99%"
        );
    }

    #[test]
    fn the_deposit_preview_scales_whole_quote_units_and_refuses_an_overflow() {
        // 1:2 base:quote vault at 6/6 decimals; 2 whole USDC is 2,000,000
        // atoms, buying 1,000 shares and 1.000000 EURC.
        let s = stake(1_000_000, 1_000_000, 1_000_000_000, 2_000_000_000);
        let (b, q) = (("EURC", 6), ("USDC", 6));
        assert_eq!(
            preview(LeaderOp::Deposit, &s, Some(2), b, q, None),
            Some(Ok(vec![
                "preview  +1.000000 EURC + 2.000000 USDC, ~+1000 shares".to_string()
            ]))
        );
        let overflow = preview(LeaderOp::Deposit, &s, Some(u64::MAX), b, q, None);
        assert!(matches!(overflow, Some(Err(e)) if e.contains("overflows")));
    }

    #[test]
    fn withdraw_at_the_floor_says_no_withdraw_fits() {
        // Exactly at a 50% floor with an outside holder: any burn dips under.
        let s = VaultStake {
            min_leader_share: 500_000,
            ..stake(1_000, 500, 10_000, 20_000)
        };
        let err = plan_withdraw(&s, 1).unwrap_err();
        assert_eq!(
            format!("{err}"),
            "the leader is at the 50.00% floor — no withdraw fits"
        );
    }

    #[test]
    fn the_floor_is_judged_after_the_realize_mints_the_leader_fee_shares() {
        // A pending performance fee: VPS is above the HWM, so realize mints
        // shares to the leader before the program checks the floor.
        let s = VaultStake {
            min_leader_share: 500_000,
            hwm: 1,
            perf_fee_rate: 500_000,
            ..stake(1_000, 500, 1_000_000, 1_000_000)
        };
        let realized = s.realized();
        assert!(realized.leader_shares > s.leader_shares, "{realized:?}");
        assert_eq!(
            realized.total_shares - s.total_shares,
            realized.leader_shares - s.leader_shares
        );
        // Judged as read, the leader sits exactly at the floor and no draw
        // fits; judged after the realize, as on chain, some do.
        assert!(!leaves_floor(&s, shares_for(&s, 1)));
        assert!(plan_withdraw(&s, 1).is_ok());
        // Winding down skips the realize, as the program does.
        let frozen = VaultStake {
            winding_down: true,
            ..s
        };
        assert_eq!(frozen.realized(), frozen);
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
        // Quote drained to zero by a taker sell: the kernel's `EmptyLeg`
        // refusal, mapped — not a panic, and not some other basket error.
        let err = plan_deposit(&stake(1_000, 1_000, 1_000, 0), 100).unwrap_err();
        assert_eq!(
            format!("{err}"),
            "the vault holds no quote to price this against"
        );
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
