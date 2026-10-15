//! The emergency actions — the kill switch for a live vault, pulled from a
//! laptop with nothing but an RPC endpoint and the leader key, whatever
//! state the maker's own host is in.
//!
//! In an eCLOB a vault's resting quotes outlive the bot that wrote them, so
//! stopping the bot stops nothing. These are the on-chain writes that do, in
//! the order a full stop takes them:
//!
//! 1. [`Op::Kill`] — `set_liquidity_profile` with an all-zero profile: every
//!    level on both sides has no size and no life, and the flush the write
//!    arms empties the book at the next take. Reversible: the next profile
//!    write (a restarted maker's included) re-arms the ladder.
//! 2. [`Op::Close`] — `close_vault` tombstones the vault. Matching never
//!    visits it again and depositor withdrawals stay open. Permanent.
//! 3. [`Op::Drain`] — `withdraw_leader` of every leader share. Only once the
//!    vault is closed or frozen: the program's min-leader-share floor refuses
//!    a full draw from an active vault.
//!
//! Each op runs over every roster market on which the session's leader leads
//! a vault, so one confirmation stops every book rather than the selected
//! one. The prompt's plan and the job's sends come from the same rule
//! ([`step`]) applied to a fresh read; the job re-reads rather than
//! replaying the plan, and a market whose send fails is reported while the
//! rest still run — a kill switch that stops at its first error leaves the
//! remaining books quoting.

use crate::accounts::VaultStake;
use crate::chain;
use crate::cluster::Cluster;
use crate::job::Logger;
use crate::leader::{self, Plan};
use crate::market::{self, PairConfig};
use anyhow::{anyhow, bail, Context, Result};
use dropset_sdk::layout::{MarketView as SlabView, Vault};
use dropset_sdk::quoting::{set_liquidity_profile_ix, PROFILE_BYTES};
use solana_client::rpc_client::RpcClient;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use std::path::Path;

/// One of the three emergency writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Kill,
    Close,
    Drain,
}

impl Op {
    /// The instruction the op sends, as the log and ceremony record name it.
    fn instruction(self) -> &'static str {
        match self {
            Op::Kill => "set_liquidity_profile",
            Op::Close => "close_vault",
            Op::Drain => "withdraw_leader",
        }
    }

    /// The plan's first line — what the op does, in the operator's terms.
    fn headline(self) -> &'static str {
        match self {
            Op::Kill => "EMERGENCY — zero every level on the leader's vaults (reversible)",
            Op::Close => {
                "EMERGENCY — close the leader's vaults: matching stops for good (PERMANENT)"
            }
            Op::Drain => "EMERGENCY — withdraw every leader share from the closed vaults",
        }
    }
}

/// A vault the leader leads, in any lifecycle state, as one read found it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seat {
    /// Its sector index — valid only for this read, since sectors recycle.
    pub idx: u32,
    /// Its per-market vault number — what the operator is told.
    pub seq: u64,
    /// The key `set_liquidity_profile` must be signed by.
    pub quote_authority: Pubkey,
    pub frozen: bool,
    pub tombstoned: bool,
    /// Whether any level on either side still carries size.
    pub quoting: bool,
    pub stake: VaultStake,
}

/// The leader's vaults on one market, with the treasuries a drain pays from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarketSeats {
    pub base_treasury: Pubkey,
    pub quote_treasury: Pubkey,
    pub seats: Vec<Seat>,
}

/// Every sector `leader` leads on `market`, read fresh: `Ok(None)` when the
/// market account is absent, an error when it cannot be read or decoded.
///
/// A sector scan rather than the active list
/// [`crate::accounts::vault_seats_fresh`] walks, because a closed vault
/// leaves that list and the drain is the step that needs it afterwards. A
/// free sector has a zeroed leader, so a drained-and-reclaimed vault drops
/// out by itself.
pub fn leader_seats(
    client: &RpcClient,
    cluster: Cluster,
    market: &Pubkey,
    leader: &Pubkey,
) -> Result<Option<MarketSeats>> {
    let Some(account) = chain::fetch_fresh(client, market, "market")? else {
        return Ok(None);
    };
    if account.owner != cluster.program_id() {
        bail!(
            "{market} is owned by {}, not the Dropset program",
            account.owner
        );
    }
    let view = SlabView::load(&account.data)
        .map_err(|_| anyhow!("{market} does not decode as a market slab"))?;
    Ok(Some(MarketSeats {
        base_treasury: Pubkey::new_from_array(view.header.base_treasury),
        quote_treasury: Pubkey::new_from_array(view.header.quote_treasury),
        seats: seats_led_by(view.sectors(), leader),
    }))
}

/// The non-free `sectors` whose leader is `leader`, in sector order.
fn seats_led_by(sectors: &[Vault], leader: &Pubkey) -> Vec<Seat> {
    sectors
        .iter()
        .enumerate()
        .filter(|(_, v)| !v.is_free() && v.leader == leader.to_bytes())
        .map(|(idx, v)| Seat {
            idx: idx as u32,
            seq: v.seq.get(),
            quote_authority: Pubkey::new_from_array(v.quote_authority),
            frozen: v.frozen != 0,
            tombstoned: v.tombstoned != 0,
            quoting: v.profile.size_bps_sums() != (0, 0),
            stake: VaultStake {
                total_shares: v.total_shares.get(),
                leader_shares: v.leader_shares.get(),
                base_atoms: v.base_atoms.get(),
                quote_atoms: v.quote_atoms.get(),
            },
        })
        .collect()
}

/// What `op` does to one seat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Kill,
    Close,
    /// Withdraw the leader's whole stake at these bounds.
    Drain(Plan),
    /// Already in the state the op leaves it in — nothing to send.
    Skip(&'static str),
    /// The op cannot run on this seat; the reason says what would let it.
    Refuse(&'static str),
}

/// The one rule the plan and the job both apply: what `op` does to `seat`,
/// signed by `leader`.
pub fn step(op: Op, seat: &Seat, leader: &Pubkey) -> Step {
    match op {
        Op::Kill if seat.tombstoned => Step::Skip("closed — matching never visits it"),
        // The program checks the signer against the vault's quote authority,
        // not its leader. The TUI opens vaults with the two equal; one opened
        // otherwise needs its own key, and a send would only fail on chain.
        Op::Kill if seat.quote_authority != *leader => {
            Step::Refuse("its quote authority is not the leader — kill it with that key")
        }
        Op::Kill if !seat.quoting => Step::Skip("every level is already zero"),
        Op::Kill => Step::Kill,
        Op::Close if seat.tombstoned => Step::Skip("already closed"),
        Op::Close => Step::Close,
        Op::Drain if seat.stake.leader_shares == 0 => Step::Skip("the leader holds no shares"),
        Op::Drain if !seat.tombstoned && !seat.frozen => {
            Step::Refuse("still active — close it first (the min-leader-share floor refuses)")
        }
        Op::Drain => match leader::plan_withdraw(&seat.stake, 100) {
            Ok(plan) => Step::Drain(plan),
            Err(_) => Step::Refuse("the stake does not size a withdrawal"),
        },
    }
}

/// The plan `op` shows under its typed-`yes` prompt: a line per roster
/// market, read fresh, nothing signed. Refuses when nothing would be sent,
/// naming the first refusal if there was one.
pub fn plan(
    op: Op,
    client: &RpcClient,
    repo_root: &Path,
    cluster: Cluster,
    admin: &Pubkey,
    operator_leader: Option<&Keypair>,
) -> Result<Vec<String>> {
    let mut lines = vec![op.headline().to_string()];
    let mut work = 0usize;
    let mut first_refusal = None;
    for config in market::roster(cluster) {
        let symbol = config.base.symbol;
        let leader = market::leader(repo_root, config, operator_leader)?.pubkey();
        let (base, quote) = market::pair_mints(repo_root, config)?;
        let address = chain::market_pda(cluster, &base, &quote);
        let Some(found) = leader_seats(client, cluster, &address, &leader)? else {
            lines.push(format!("{symbol}: no market — skipped"));
            continue;
        };
        if found.seats.is_empty() {
            lines.push(format!(
                "{symbol}: leader {leader} leads no vault — skipped"
            ));
            continue;
        }
        for seat in &found.seats {
            let s = step(op, seat, &leader);
            if let Step::Refuse(reason) = s {
                first_refusal.get_or_insert(format!("{symbol} vault #{}: {reason}", seat.seq));
            }
            if matches!(s, Step::Kill | Step::Close | Step::Drain(_)) {
                work += 1;
            }
            lines.push(plan_line(symbol, config, seat, s));
        }
    }
    if work == 0 {
        match first_refusal {
            Some(reason) => bail!("nothing to send — {reason}"),
            None => bail!("nothing to send — every vault is already done"),
        }
    }
    lines.push(format!(
        "signed by the leader per market · fees paid by the admin {admin}"
    ));
    Ok(lines)
}

/// One seat's line in the plan.
fn plan_line(symbol: &str, config: &PairConfig, seat: &Seat, step: Step) -> String {
    let vault = format!("{symbol}: vault #{}", seat.seq);
    match step {
        Step::Kill => format!("{vault} — KILL: zero every bid and ask level"),
        Step::Close => format!("{vault} — CLOSE: tombstone it; matching stops for good"),
        Step::Drain(Plan::Withdraw {
            shares_in,
            min_base_out,
            min_quote_out,
            ..
        }) => format!(
            "{vault} — DRAIN: {shares_in} shares for at least {} {symbol} + {} {}",
            leader::human(min_base_out, config.base.decimals),
            leader::human(min_quote_out, config.quote.decimals),
            config.quote.symbol
        ),
        // `plan_withdraw` only ever sizes a withdrawal.
        Step::Drain(Plan::Deposit { .. }) => format!("{vault} — REFUSED: not a withdrawal"),
        Step::Skip(reason) => format!("{vault} — {reason}, skipped"),
        Step::Refuse(reason) => format!("{vault} — REFUSED: {reason}"),
    }
}

/// Run `op` against a fresh read, one transaction per vault, signed by the
/// leader and paid by `wallet`. Every market is attempted; the job fails at
/// the end if any send failed or any vault was refused, so the operator
/// sees the outcome in red rather than a partial stop reading as success.
pub fn execute(
    op: Op,
    client: &RpcClient,
    wallet: &Keypair,
    repo_root: &Path,
    cluster: Cluster,
    operator_leader: Option<&Keypair>,
    log: &Logger,
) -> Result<String> {
    let mut sent = 0usize;
    let mut failed = 0usize;
    for config in market::roster(cluster) {
        let symbol = config.base.symbol;
        let leader = market::leader(repo_root, config, operator_leader)?;
        let (base, quote) = market::pair_mints(repo_root, config)?;
        let address = chain::market_pda(cluster, &base, &quote);
        let found = match leader_seats(client, cluster, &address, &leader.pubkey()) {
            Ok(Some(found)) => found,
            Ok(None) => continue,
            Err(e) => {
                log.log(format!("{symbol}: {e:#}"));
                failed += 1;
                continue;
            }
        };
        for seat in &found.seats {
            let ixs = match step(op, seat, &leader.pubkey()) {
                Step::Skip(_) => continue,
                Step::Refuse(reason) => {
                    log.log(format!("{symbol} vault #{}: refused — {reason}", seat.seq));
                    failed += 1;
                    continue;
                }
                s => instructions(
                    s,
                    cluster,
                    wallet,
                    &leader.pubkey(),
                    &address,
                    &base,
                    &quote,
                    &found,
                    seat,
                ),
            };
            let label = op.instruction();
            match chain::send_logged(client, wallet, &[wallet, &leader], &ixs, label, log)
                .with_context(|| format!("{symbol} vault #{}: {label}", seat.seq))
            {
                Ok(_) => {
                    sent += 1;
                    log.accounts_changed();
                }
                Err(e) => {
                    log.log(format!("{e:#}"));
                    failed += 1;
                }
            }
        }
    }
    if failed > 0 {
        bail!(
            "{} landed on {sent} vault(s); {failed} failed or were refused — see the log",
            op.instruction()
        );
    }
    if sent == 0 {
        bail!("nothing was sent — every vault is already done");
    }
    Ok(format!("{} landed on {sent} vault(s)", op.instruction()))
}

/// The transaction for a sending `step` on `seat`, addressed to `cluster`'s
/// program.
#[allow(clippy::too_many_arguments)]
fn instructions(
    step: Step,
    cluster: Cluster,
    wallet: &Keypair,
    leader: &Pubkey,
    market: &Pubkey,
    base_mint: &Pubkey,
    quote_mint: &Pubkey,
    found: &MarketSeats,
    seat: &Seat,
) -> Vec<Instruction> {
    match step {
        // The SDK builder addresses the committed program id, so it passes
        // through `retarget` like every other quoting write — without it, the
        // kill would go to a program that does not exist on mainnet.
        Step::Kill => vec![chain::retarget(
            cluster,
            set_liquidity_profile_ix(*leader, *market, seat.idx, [0u8; PROFILE_BYTES]),
        )],
        Step::Close => vec![chain::build_close_vault_ix(
            cluster, leader, market, seat.idx,
        )],
        Step::Drain(Plan::Withdraw {
            shares_in,
            min_base_out,
            min_quote_out,
            ..
        }) => {
            let mut ixs =
                leader::leader_ata_creates(&wallet.pubkey(), leader, base_mint, quote_mint);
            ixs.push(chain::build_withdraw_leader_ix(
                cluster,
                leader,
                market,
                base_mint,
                quote_mint,
                &found.base_treasury,
                &found.quote_treasury,
                seat.idx,
                shares_in,
                min_base_out,
                min_quote_out,
            ));
            ixs
        }
        Step::Drain(Plan::Deposit { .. }) | Step::Skip(_) | Step::Refuse(_) => {
            unreachable!("instructions() is only called for a sending step")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seat(leader: Pubkey) -> Seat {
        Seat {
            idx: 3,
            seq: 7,
            quote_authority: leader,
            frozen: false,
            tombstoned: false,
            quoting: true,
            stake: VaultStake {
                total_shares: 1_000,
                leader_shares: 1_000,
                base_atoms: 10_000,
                quote_atoms: 20_000,
            },
        }
    }

    #[test]
    fn kill_zeroes_a_quoting_vault_and_skips_a_dead_one() {
        let me = Pubkey::new_unique();
        assert_eq!(step(Op::Kill, &seat(me), &me), Step::Kill);
        let idle = Seat {
            quoting: false,
            ..seat(me)
        };
        assert!(matches!(step(Op::Kill, &idle, &me), Step::Skip(_)));
        let closed = Seat {
            tombstoned: true,
            ..seat(me)
        };
        assert!(matches!(step(Op::Kill, &closed, &me), Step::Skip(_)));
    }

    #[test]
    fn kill_refuses_a_vault_whose_quote_authority_is_another_key() {
        // The program would reject the leader's signature; refusing here says
        // which key would work instead of a bare on-chain error.
        let me = Pubkey::new_unique();
        let delegated = Seat {
            quote_authority: Pubkey::new_unique(),
            ..seat(me)
        };
        assert!(matches!(step(Op::Kill, &delegated, &me), Step::Refuse(_)));
    }

    #[test]
    fn close_tombstones_once() {
        let me = Pubkey::new_unique();
        assert_eq!(step(Op::Close, &seat(me), &me), Step::Close);
        // A frozen vault is still on the active list, so it still closes.
        let frozen = Seat {
            frozen: true,
            ..seat(me)
        };
        assert_eq!(step(Op::Close, &frozen, &me), Step::Close);
        let closed = Seat {
            tombstoned: true,
            ..seat(me)
        };
        assert!(matches!(step(Op::Close, &closed, &me), Step::Skip(_)));
    }

    #[test]
    fn drain_waits_for_a_closed_or_frozen_vault_and_takes_the_whole_stake() {
        let me = Pubkey::new_unique();
        // Active: the min-leader-share floor would refuse a full draw.
        assert!(matches!(step(Op::Drain, &seat(me), &me), Step::Refuse(_)));
        for stopped in [
            Seat {
                tombstoned: true,
                ..seat(me)
            },
            Seat {
                frozen: true,
                ..seat(me)
            },
        ] {
            let Step::Drain(Plan::Withdraw {
                shares_in,
                base_out,
                quote_out,
                ..
            }) = step(Op::Drain, &stopped, &me)
            else {
                panic!("a stopped vault drains");
            };
            assert_eq!((shares_in, base_out, quote_out), (1_000, 10_000, 20_000));
        }
        let empty = Seat {
            tombstoned: true,
            stake: VaultStake {
                leader_shares: 0,
                ..seat(me).stake
            },
            ..seat(me)
        };
        assert!(matches!(step(Op::Drain, &empty, &me), Step::Skip(_)));
    }

    #[test]
    fn the_sector_scan_finds_closed_vaults_and_ignores_free_and_foreign_ones() {
        use bytemuck::Zeroable;
        let me = Pubkey::new_unique();
        let stranger = Pubkey::new_unique();
        let mut sectors = [Vault::zeroed(); 4];
        // 0: free (zeroed leader). 1: a stranger's. 2: ours, live and quoting.
        // 3: ours, closed with a zero profile.
        sectors[1].leader = stranger.to_bytes();
        sectors[2].leader = me.to_bytes();
        sectors[2].quote_authority = me.to_bytes();
        sectors[2].profile.bids[0].size_bps = 500u16.into();
        sectors[3].leader = me.to_bytes();
        sectors[3].tombstoned = 1;
        let found = seats_led_by(&sectors, &me);
        assert_eq!(found.iter().map(|s| s.idx).collect::<Vec<_>>(), [2, 3]);
        assert!(found[0].quoting && !found[0].tombstoned);
        assert!(!found[1].quoting && found[1].tombstoned);
        assert_eq!(found[0].quote_authority, me);
    }

    #[test]
    fn every_emergency_write_is_addressed_to_the_clusters_program() {
        // The kill's SDK builder stamps the committed program id; a missing
        // retarget would compile and send the mainnet kill nowhere.
        let me = Pubkey::new_unique();
        let wallet = Keypair::new();
        let found = MarketSeats {
            base_treasury: Pubkey::new_unique(),
            quote_treasury: Pubkey::new_unique(),
            seats: vec![],
        };
        let closed = Seat {
            tombstoned: true,
            ..seat(me)
        };
        for cluster in [Cluster::Localnet, Cluster::Mainnet] {
            for s in [
                step(Op::Kill, &seat(me), &me),
                step(Op::Close, &seat(me), &me),
                step(Op::Drain, &closed, &me),
            ] {
                let ixs = instructions(
                    s,
                    cluster,
                    &wallet,
                    &me,
                    &Pubkey::new_unique(),
                    &Pubkey::new_unique(),
                    &Pubkey::new_unique(),
                    &found,
                    &seat(me),
                );
                // The drain's ATA creates ride in front; the write is last.
                let write = ixs.last().unwrap();
                assert_eq!(
                    write.program_id,
                    cluster.program_id(),
                    "{s:?} on {cluster:?}"
                );
            }
        }
    }

    #[test]
    fn the_kill_profile_has_no_size_and_no_life() {
        // What "zero the ladder" relies on: an all-zero profile passes the
        // per-side size check and every level is dead in both clock domains.
        let profile: &dropset_sdk::layout::LiquidityProfile =
            bytemuck::from_bytes(&[0u8; PROFILE_BYTES]);
        assert_eq!(profile.size_bps_sums(), (0, 0));
        assert!(profile.validate_size_sums().is_ok());
        for level in profile.bids.iter().chain(&profile.asks) {
            assert_eq!(level.expiry_offset_secs.get(), 0);
            assert_eq!(level.expiry_offset_slots.get(), 0);
        }
    }
}
