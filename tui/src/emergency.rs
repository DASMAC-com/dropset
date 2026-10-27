//! The emergency actions — the kill switch for a live vault, pulled from a
//! laptop with nothing but an RPC endpoint and the vault's keys, whatever
//! state the maker's own host is in. The kill is a ladder write, so it signs
//! with the quote authority and needs no custody key; close and drain sign
//! with the leader.
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
//! Each op runs over every roster market on which the session's signing key
//! for it holds a vault, so one confirmation stops every book rather than the
//! selected one. The prompt's plan and the job's sends come from the same rule
//! ([`step`]) applied to a fresh read; the job re-reads rather than
//! replaying the plan, and a market whose send fails is reported while the
//! rest still run — a kill switch that stops at its first error leaves the
//! remaining books quoting.
//!
//! The job acts only on the vaults the confirmed plan listed ([`Planned`]):
//! one opened between the prompt and Enter is left alone and turns the job
//! red, so it is planned and confirmed on its own; one gone since is
//! reported. A market the plan could not read is read again and acted on as
//! found, as its plan line said.

use crate::accounts::VaultStake;
use crate::chain;
use crate::cluster::Cluster;
use crate::job::Logger;
use crate::keys::LazyKey;
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
use std::collections::{BTreeMap, BTreeSet};
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

    /// The role whose key signs the op — the one the program checks.
    fn signer_role(self) -> &'static str {
        match self {
            Op::Kill => "quote authority",
            Op::Close | Op::Drain => "leader",
        }
    }

    /// Resolve the key the op signs with on `config`'s market: the quote
    /// authority for a kill, so a kill never loads the custody key, and the
    /// leader otherwise.
    fn signer(
        self,
        repo_root: &Path,
        config: &PairConfig,
        operator_leader: Option<&LazyKey>,
        operator_quote: Option<&Keypair>,
    ) -> Result<Keypair> {
        match self {
            Op::Kill => market::quote_authority(repo_root, config, operator_quote),
            Op::Close | Op::Drain => market::leader(repo_root, config, operator_leader),
        }
    }

    /// The vaults on `market` the op acts on for `signer`: those it is quote
    /// authority of for a kill, those it leads otherwise.
    ///
    /// A kill also lists the vaults the session's leader leads, when that key
    /// is already resolved — never resolving it — so a led vault with another
    /// quote authority shows as refused rather than going unseen while its
    /// ladder rests.
    fn seats(
        self,
        client: &RpcClient,
        cluster: Cluster,
        market: &Pubkey,
        signer: &Pubkey,
        operator_leader: Option<&LazyKey>,
    ) -> Result<Option<MarketSeats>> {
        match self {
            Op::Kill => {
                let Some(mut found) = quote_seats(client, cluster, market, signer)? else {
                    return Ok(None);
                };
                let leader = operator_leader.and_then(LazyKey::resolved_pubkey);
                if let Some(led) = leader.filter(|l| l != signer) {
                    let led = leader_seats(client, cluster, market, &led)?
                        .map(|m| m.seats)
                        .unwrap_or_default();
                    merge_seats(&mut found.seats, led);
                }
                Ok(Some(found))
            }
            Op::Close | Op::Drain => leader_seats(client, cluster, market, signer),
        }
    }

    /// The plan's first line — what the op does, in the operator's terms.
    fn headline(self) -> &'static str {
        match self {
            Op::Kill => {
                "EMERGENCY — zero every level on the quote key's vaults (reversible: a running \
                 maker re-arms it; close is the durable stop)"
            }
            Op::Close => {
                "EMERGENCY — close the leader's vaults: matching stops for good (PERMANENT)"
            }
            Op::Drain => "EMERGENCY — withdraw every leader share from the closed or frozen vaults",
        }
    }
}

/// A vault the session's key leads or quotes for, in any lifecycle state, as
/// one read found it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SectorSeat {
    /// Its sector index — valid only for this read, since sectors recycle.
    pub idx: u32,
    /// Its per-market vault number — what the operator is told.
    pub seq: u64,
    /// The key `set_liquidity_profile` must be signed by.
    pub quote_authority: Pubkey,
    /// Admin-frozen: still on the active list, but past the leader floor.
    pub frozen: bool,
    /// Closed: off the active list, so matching never visits it.
    pub tombstoned: bool,
    /// Whether any level on either side still carries size.
    pub quoting: bool,
    /// The share ledger and inventory a drain is sized from.
    pub stake: VaultStake,
}

/// One key's vaults on one market, with the treasuries a drain pays from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarketSeats {
    pub base_treasury: Pubkey,
    pub quote_treasury: Pubkey,
    pub seats: Vec<SectorSeat>,
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
    seats_where(client, cluster, market, |v| v.leader == leader.to_bytes())
}

/// Every sector on `market` whose quote authority is `quote_authority`, read
/// fresh, as [`leader_seats`] reads them. How the hot path finds its vault
/// without the leader key: the quote authority is what it signs as.
pub fn quote_seats(
    client: &RpcClient,
    cluster: Cluster,
    market: &Pubkey,
    quote_authority: &Pubkey,
) -> Result<Option<MarketSeats>> {
    seats_where(client, cluster, market, |v| {
        v.quote_authority == quote_authority.to_bytes()
    })
}

/// The non-free sectors on `market` that `keep` selects, read fresh.
fn seats_where(
    client: &RpcClient,
    cluster: Cluster,
    market: &Pubkey,
    keep: impl Fn(&Vault) -> bool,
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
        seats: sector_seats(view.sectors(), keep),
    }))
}

/// Add each of `more` that `seats` lacks, keeping sector order.
fn merge_seats(seats: &mut Vec<SectorSeat>, more: Vec<SectorSeat>) {
    for seat in more {
        if !seats.iter().any(|s| s.idx == seat.idx) {
            seats.push(seat);
        }
    }
    seats.sort_by_key(|s| s.idx);
}

/// The non-free `sectors` that `keep` selects, in sector order.
fn sector_seats(sectors: &[Vault], keep: impl Fn(&Vault) -> bool) -> Vec<SectorSeat> {
    sectors
        .iter()
        .enumerate()
        .filter(|(_, v)| !v.is_free() && keep(v))
        .map(|(idx, v)| SectorSeat {
            idx: idx as u32,
            seq: v.seq.get(),
            quote_authority: Pubkey::new_from_array(v.quote_authority),
            frozen: v.frozen != 0,
            tombstoned: v.tombstoned != 0,
            quoting: v.profile.size_bps_sums() != (0, 0),
            stake: VaultStake::of(v),
        })
        .collect()
}

/// What `op` does to one seat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Kill,
    Close,
    /// Withdraw the leader's whole stake at these bounds.
    Drain {
        shares_in: u64,
        min_base_out: u64,
        min_quote_out: u64,
    },
    /// Already in the state the op leaves it in — nothing to send.
    Skip(&'static str),
    /// The op cannot run on this seat; the reason says what would let it.
    Refuse(&'static str),
}

/// The one rule the plan and the job both apply: what `op` does to `seat`,
/// signed by `signer` (the op's own key: the quote authority for a kill, the
/// leader otherwise).
pub fn step(op: Op, seat: &SectorSeat, signer: &Pubkey) -> Step {
    match op {
        Op::Kill if seat.tombstoned => Step::Skip("closed — matching never visits it"),
        // Before the signer check: a dead ladder needs no key at all.
        Op::Kill if !seat.quoting => Step::Skip("every level is already zero"),
        // The program checks the signer against the vault's quote authority.
        // A kill finds its vaults by that key, so this holds by construction;
        // it stays as the guard, since a send would only fail on chain.
        Op::Kill if seat.quote_authority != *signer => {
            Step::Refuse("its quote authority is not this session's quote key — close it instead")
        }
        Op::Kill => Step::Kill,
        Op::Close if seat.tombstoned => Step::Skip("already closed"),
        Op::Close => Step::Close,
        Op::Drain if seat.stake.leader_shares == 0 => Step::Skip("the leader holds no shares"),
        Op::Drain if !seat.tombstoned && !seat.frozen => {
            Step::Refuse("still active — close it first (the min-leader-share floor refuses)")
        }
        // No performance fee is realized first: the program's realize is a
        // no-op on a frozen or closed vault, so this read is what is paid.
        Op::Drain => match leader::plan_withdraw(&seat.stake, 100) {
            Ok(Plan::Withdraw {
                shares_in,
                min_base_out,
                min_quote_out,
                ..
            }) => Step::Drain {
                shares_in,
                min_base_out,
                min_quote_out,
            },
            Ok(Plan::Deposit { .. } | Plan::SetPerfFee { .. }) | Err(_) => {
                Step::Refuse("the stake does not size a withdrawal")
            }
        },
    }
}

/// The vaults a confirmed plan listed, per roster market it read, by
/// per-market vault number — never sector index, which recycles. The job
/// acts on exactly these. A market absent from the set is one the plan could
/// not resolve or read, which the job reads again and acts on as found.
///
/// Every listed vault counts, skipped and refused lines included: the job
/// re-applies [`step`] to each, so one whose state moved in the window gets
/// what the confirmed op now does to it — a kill for a ladder re-armed since,
/// a drain for a vault closed since.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Planned(BTreeMap<Pubkey, BTreeSet<u64>>);

/// One roster market resolved for an op: its address, its mints, and the
/// op's signing key on it.
struct Market {
    address: Pubkey,
    base: Pubkey,
    quote: Pubkey,
    signer: Keypair,
}

/// Every roster market resolved for `op`, each on its own: one market's
/// missing key or mint must not hold back every other market's stop.
fn resolve(
    op: Op,
    repo_root: &Path,
    cluster: Cluster,
    operator_leader: Option<&LazyKey>,
    operator_quote: Option<&Keypair>,
) -> Vec<(&'static PairConfig, Result<Market>)> {
    market::roster(cluster)
        .iter()
        .map(|&config| {
            let market = op
                .signer(repo_root, config, operator_leader, operator_quote)
                .and_then(|signer| {
                    let (base, quote) = market::pair_mints(repo_root, config)?;
                    Ok(Market {
                        address: chain::market_pda(cluster, &base, &quote),
                        base,
                        quote,
                        signer,
                    })
                });
            (config, market)
        })
        .collect()
}

/// The two chain calls an emergency op makes — one market's seats, read
/// fresh, and one vault's send — behind a seam so the plan and the job's
/// tally run over a stub in tests.
trait Rpc {
    fn seats(&self, op: Op, market: &Pubkey, signer: &Pubkey) -> Result<Option<MarketSeats>>;
    fn send(
        &self,
        wallet: &Keypair,
        signer: &Keypair,
        ixs: &[Instruction],
        label: &str,
        log: &Logger,
    ) -> Result<()>;
}

/// The live chain behind [`Rpc`].
struct Live<'a> {
    client: &'a RpcClient,
    cluster: Cluster,
    operator_leader: Option<&'a LazyKey>,
}

impl Rpc for Live<'_> {
    fn seats(&self, op: Op, market: &Pubkey, signer: &Pubkey) -> Result<Option<MarketSeats>> {
        op.seats(
            self.client,
            self.cluster,
            market,
            signer,
            self.operator_leader,
        )
    }

    fn send(
        &self,
        wallet: &Keypair,
        signer: &Keypair,
        ixs: &[Instruction],
        label: &str,
        log: &Logger,
    ) -> Result<()> {
        chain::send_logged(self.client, wallet, &[wallet, signer], ixs, label, log).map(drop)
    }
}

/// The plan `op` shows under its typed-`yes` prompt: a line per roster
/// market, read fresh, nothing signed, and the vaults those lines list —
/// what [`execute`] is then confined to. Refuses when nothing would be sent,
/// naming the first refusal if there was one.
pub fn plan(
    op: Op,
    client: &RpcClient,
    repo_root: &Path,
    cluster: Cluster,
    admin: &Pubkey,
    operator_leader: Option<&LazyKey>,
    operator_quote: Option<&Keypair>,
) -> Result<(Vec<String>, Planned)> {
    let rpc = Live {
        client,
        cluster,
        operator_leader,
    };
    let markets = resolve(op, repo_root, cluster, operator_leader, operator_quote);
    plan_over(op, &rpc, &markets, admin)
}

/// [`plan`] over resolved `markets`, reading through `rpc`.
fn plan_over(
    op: Op,
    rpc: &impl Rpc,
    markets: &[(&'static PairConfig, Result<Market>)],
    admin: &Pubkey,
) -> Result<(Vec<String>, Planned)> {
    let mut lines = vec![op.headline().to_string()];
    let mut planned = Planned::default();
    let mut work = 0usize;
    let mut first_refusal = None;
    for (config, market) in markets {
        let symbol = config.base.symbol;
        // One market's failure to resolve or read must not hold the prompt
        // back from every other market's stop: it is a line in the plan, and
        // the job tries again. With nothing else to send, the first such
        // reason is what the refusal names (a missing key included).
        let market = match market {
            Ok(market) => market,
            Err(e) => {
                let reason = format!("{symbol}: {e:#}");
                lines.push(reason.clone());
                first_refusal.get_or_insert(reason);
                continue;
            }
        };
        let signer = market.signer.pubkey();
        let found = match rpc.seats(op, &market.address, &signer) {
            Ok(Some(found)) => found,
            Ok(None) => {
                // Read, and empty: a vault on a market created since is not
                // what the operator confirmed.
                planned.0.insert(market.address, BTreeSet::new());
                lines.push(format!("{symbol}: no market — skipped"));
                continue;
            }
            Err(e) => {
                let reason = format!("{symbol}: read failed — {e:#}");
                lines.push(reason.clone());
                first_refusal.get_or_insert(reason);
                continue;
            }
        };
        planned.0.insert(
            market.address,
            found.seats.iter().map(|seat| seat.seq).collect(),
        );
        if found.seats.is_empty() {
            // A kill scans by quote authority, so say so: a vault led under
            // another quote key is out of its reach, and close stops it.
            lines.push(match op {
                Op::Kill => format!(
                    "{symbol}: quote authority {signer} signs for no vault — skipped (close \
                     stops a vault led under another quote key)"
                ),
                Op::Close | Op::Drain => {
                    format!("{symbol}: leader {signer} leads no vault — skipped")
                }
            });
            continue;
        }
        for seat in &found.seats {
            let s = step(op, seat, &signer);
            if let Step::Refuse(reason) = s {
                first_refusal.get_or_insert(format!("{symbol} vault #{}: {reason}", seat.seq));
            }
            if matches!(s, Step::Kill | Step::Close | Step::Drain { .. }) {
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
        "signed by the {} per market · fees paid by the admin {admin}",
        op.signer_role()
    ));
    Ok((lines, planned))
}

/// One seat's line in the plan.
fn plan_line(symbol: &str, config: &PairConfig, seat: &SectorSeat, step: Step) -> String {
    let vault = format!("{symbol}: vault #{}", seat.seq);
    match step {
        Step::Kill => format!("{vault} — KILL: zero every bid and ask level"),
        Step::Close => format!("{vault} — CLOSE: tombstone it; matching stops for good"),
        Step::Drain {
            shares_in,
            min_base_out,
            min_quote_out,
        } => format!(
            "{vault} — DRAIN: {shares_in} shares for at least {} {symbol} + {} {}",
            leader::human(min_base_out, config.base.decimals),
            leader::human(min_quote_out, config.quote.decimals),
            config.quote.symbol
        ),
        Step::Skip(reason) => format!("{vault} — {reason}, skipped"),
        Step::Refuse(reason) => format!("{vault} — REFUSED: {reason}"),
    }
}

/// Run `op` against a fresh read, one transaction per vault, signed by the
/// op's key (the quote authority for a kill, the leader otherwise) and paid by
/// `wallet`. Confined to the vaults `planned` lists when there was a plan;
/// localnet, which has no prompt, acts on everything it reads. Every market is
/// attempted; the job fails at the end if any send failed, any vault was
/// refused, or any vault was left alone for want of a plan, so the operator
/// sees the outcome in red rather than a partial stop reading as success.
#[allow(clippy::too_many_arguments)]
pub fn execute(
    op: Op,
    client: &RpcClient,
    wallet: &Keypair,
    repo_root: &Path,
    cluster: Cluster,
    operator_leader: Option<&LazyKey>,
    operator_quote: Option<&Keypair>,
    planned: Option<&Planned>,
    log: &Logger,
) -> Result<String> {
    let rpc = Live {
        client,
        cluster,
        operator_leader,
    };
    let markets = resolve(op, repo_root, cluster, operator_leader, operator_quote);
    execute_over(op, &rpc, cluster, wallet, &markets, planned, log)
}

/// [`execute`] over resolved `markets`, reading and sending through `rpc`.
fn execute_over(
    op: Op,
    rpc: &impl Rpc,
    cluster: Cluster,
    wallet: &Keypair,
    markets: &[(&'static PairConfig, Result<Market>)],
    planned: Option<&Planned>,
    log: &Logger,
) -> Result<String> {
    let mut sent = 0usize;
    let mut failed = 0usize;
    let mut unplanned = 0usize;
    for (config, market) in markets {
        let symbol = config.base.symbol;
        let market = match market {
            Ok(market) => market,
            Err(e) => {
                log.log(format!("{symbol}: {e:#}"));
                failed += 1;
                continue;
            }
        };
        let signer = market.signer.pubkey();
        let found = match rpc.seats(op, &market.address, &signer) {
            Ok(Some(found)) => found,
            Ok(None) => continue,
            Err(e) => {
                log.log(format!("{symbol}: {e:#}"));
                failed += 1;
                continue;
            }
        };
        // `None` when there was no plan, or the plan could not read this
        // market: the job then acts on its own read.
        let listed = planned.and_then(|p| p.0.get(&market.address));
        for seq in listed.into_iter().flatten() {
            if !found.seats.iter().any(|seat| seat.seq == *seq) {
                log.log(format!(
                    "{symbol} vault #{seq}: no longer found since the plan — nothing to send"
                ));
            }
        }
        for seat in &found.seats {
            let s = step(op, seat, &signer);
            // Before anything is counted: a vault with nothing to send needs
            // no plan, so one opened since with a dead ladder leaves the job
            // green rather than asking for a re-plan that would refuse.
            if let Step::Skip(_) = s {
                continue;
            }
            // A vault's number is never reused on its market, so one the plan
            // does not list opened after the operator read it — left alone,
            // refusal or send alike.
            if listed.is_some_and(|listed| !listed.contains(&seat.seq)) {
                log.log(format!(
                    "{symbol} vault #{}: opened since the plan — left alone; run it again to \
                     plan it",
                    seat.seq
                ));
                unplanned += 1;
                continue;
            }
            let ixs = match s {
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
                    &signer,
                    &market.address,
                    &market.base,
                    &market.quote,
                    &found,
                    seat,
                ),
            };
            let label = op.instruction();
            match rpc
                .send(wallet, &market.signer, &ixs, label, log)
                .with_context(|| format!("{symbol} vault #{}: {label}", seat.seq))
            {
                Ok(()) => {
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
    let mut shortfalls = Vec::new();
    if failed > 0 {
        shortfalls.push(format!("{failed} failed or were refused"));
    }
    if unplanned > 0 {
        shortfalls.push(format!(
            "{unplanned} opened since the plan and were left alone"
        ));
    }
    if !shortfalls.is_empty() {
        bail!(
            "{} landed on {sent} vault(s); {} — see the log",
            op.instruction(),
            shortfalls.join("; ")
        );
    }
    if sent == 0 {
        bail!("nothing was sent — every vault is already done");
    }
    Ok(format!("{} landed on {sent} vault(s)", op.instruction()))
}

/// The transaction for a sending `step` on `seat`, addressed to `cluster`'s
/// program, signed by `signer` (the op's own key).
#[allow(clippy::too_many_arguments)]
fn instructions(
    step: Step,
    cluster: Cluster,
    wallet: &Keypair,
    signer: &Pubkey,
    market: &Pubkey,
    base_mint: &Pubkey,
    quote_mint: &Pubkey,
    found: &MarketSeats,
    seat: &SectorSeat,
) -> Vec<Instruction> {
    match step {
        // Aimed at the cluster's own program, like every other quoting write:
        // the localnet id does not belong to us on mainnet.
        Step::Kill => vec![set_liquidity_profile_ix(
            cluster.deployment(),
            *signer,
            *market,
            seat.idx,
            [0u8; PROFILE_BYTES],
        )],
        Step::Close => vec![chain::build_close_vault_ix(
            cluster, signer, market, seat.idx,
        )],
        Step::Drain {
            shares_in,
            min_base_out,
            min_quote_out,
        } => {
            let mut ixs =
                leader::leader_ata_creates(&wallet.pubkey(), signer, base_mint, quote_mint);
            ixs.push(chain::build_withdraw_leader_ix(
                cluster,
                signer,
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
        Step::Skip(_) | Step::Refuse(_) => {
            unreachable!("instructions() is only called for a sending step")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seat(leader: Pubkey) -> SectorSeat {
        SectorSeat {
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
                ..VaultStake::default()
            },
        }
    }

    #[test]
    fn kill_zeroes_a_quoting_vault_and_skips_a_dead_one() {
        let me = Pubkey::new_unique();
        assert_eq!(step(Op::Kill, &seat(me), &me), Step::Kill);
        let idle = SectorSeat {
            quoting: false,
            ..seat(me)
        };
        assert!(matches!(step(Op::Kill, &idle, &me), Step::Skip(_)));
        let closed = SectorSeat {
            tombstoned: true,
            ..seat(me)
        };
        assert!(matches!(step(Op::Kill, &closed, &me), Step::Skip(_)));
    }

    #[test]
    fn kill_refuses_a_vault_whose_quote_authority_is_another_key() {
        // The program would reject the signature; refusing here points at
        // close instead of a bare on-chain error.
        let me = Pubkey::new_unique();
        let delegated = SectorSeat {
            quote_authority: Pubkey::new_unique(),
            ..seat(me)
        };
        assert!(matches!(step(Op::Kill, &delegated, &me), Step::Refuse(_)));
        // Unless its ladder is already dead: nothing to sign, so no refusal
        // to turn a finished kill red.
        let idle = SectorSeat {
            quoting: false,
            ..delegated
        };
        assert!(matches!(step(Op::Kill, &idle, &me), Step::Skip(_)));
    }

    #[test]
    fn a_kill_signs_with_the_quote_key_and_never_resolves_the_leader() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let config = market::roster(Cluster::Mainnet)[0];
        // Unresolvable on purpose: resolving it at all would fail the test.
        let leader = LazyKey::new(
            "op://nowhere/lead/key".to_string(),
            "--leader",
            Cluster::Mainnet,
            vec![],
            vec![],
        );
        let quote = Keypair::new();
        let signer = Op::Kill
            .signer(root, config, Some(&leader), Some(&quote))
            .unwrap();
        assert_eq!(signer.pubkey(), quote.pubkey());
        assert_eq!(leader.resolved_pubkey(), None);
        // With no quote key the kill refuses; it never falls back to the leader.
        let err = Op::Kill
            .signer(root, config, Some(&leader), None)
            .unwrap_err();
        assert!(format!("{err:#}").contains("--quote"), "{err:#}");
        assert_eq!(leader.resolved_pubkey(), None);
    }

    #[test]
    fn a_kill_refuses_a_led_vault_its_quote_key_cannot_sign() {
        // The leader's own scan adds the vault the quote scan missed; it is
        // refused, so the job ends red instead of reading as a full stop.
        let quote = Pubkey::new_unique();
        let mut found = vec![SectorSeat {
            idx: 5,
            ..seat(quote)
        }];
        let led_elsewhere = SectorSeat {
            idx: 2,
            quote_authority: Pubkey::new_unique(),
            ..seat(quote)
        };
        // The vault both scans found is listed once.
        let both = found[0];
        merge_seats(&mut found, vec![led_elsewhere, both]);
        assert_eq!(found.iter().map(|s| s.idx).collect::<Vec<_>>(), [2, 5]);
        assert!(matches!(step(Op::Kill, &found[0], &quote), Step::Refuse(_)));
        assert_eq!(step(Op::Kill, &found[1], &quote), Step::Kill);
    }

    #[test]
    fn close_tombstones_once() {
        let me = Pubkey::new_unique();
        assert_eq!(step(Op::Close, &seat(me), &me), Step::Close);
        // A frozen vault is still on the active list, so it still closes.
        let frozen = SectorSeat {
            frozen: true,
            ..seat(me)
        };
        assert_eq!(step(Op::Close, &frozen, &me), Step::Close);
        let closed = SectorSeat {
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
            SectorSeat {
                tombstoned: true,
                ..seat(me)
            },
            SectorSeat {
                frozen: true,
                ..seat(me)
            },
        ] {
            // Every leader share, for the whole stake less the 100 bps floor.
            assert_eq!(
                step(Op::Drain, &stopped, &me),
                Step::Drain {
                    shares_in: 1_000,
                    min_base_out: 9_900,
                    min_quote_out: 19_800,
                }
            );
        }
        let empty = SectorSeat {
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
        // Distinct values per field, so a transposed mapping fails: the
        // drain is sized from exactly these.
        sectors[3].total_shares = 40u64.into();
        sectors[3].leader_shares = 30u64.into();
        sectors[3].base_atoms = 20u64.into();
        sectors[3].quote_atoms = 10u64.into();
        let found = sector_seats(&sectors, |v| v.leader == me.to_bytes());
        assert_eq!(found.iter().map(|s| s.idx).collect::<Vec<_>>(), [2, 3]);
        // The kill's scan keys on the quote authority instead: only sector 2
        // names `me` there, and a free sector never matches.
        let quoted = sector_seats(&sectors, |v| v.quote_authority == me.to_bytes());
        assert_eq!(quoted.iter().map(|s| s.idx).collect::<Vec<_>>(), [2]);
        assert!(found[0].quoting && !found[0].tombstoned);
        assert!(!found[1].quoting && found[1].tombstoned);
        assert_eq!(found[0].quote_authority, me);
        assert_eq!(
            found[1].stake,
            VaultStake {
                total_shares: 40,
                leader_shares: 30,
                base_atoms: 20,
                quote_atoms: 10,
                min_leader_share: 0,
                winding_down: true,
                hwm: 0,
                perf_fee_rate: 0,
            }
        );
    }

    #[test]
    fn every_emergency_write_is_addressed_to_the_clusters_program() {
        // Every builder takes a deployment, so no write can omit its cluster;
        // this pins that each one is handed the right one.
        let me = Pubkey::new_unique();
        let wallet = Keypair::new();
        let found = MarketSeats {
            base_treasury: Pubkey::new_unique(),
            quote_treasury: Pubkey::new_unique(),
            seats: vec![],
        };
        let closed = SectorSeat {
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

    use crate::job::JobEvent;
    use std::cell::RefCell;
    use std::collections::{HashMap, VecDeque};
    use std::sync::mpsc;

    /// A chain the test scripts: each market's reads in order, and every
    /// send's write recorded.
    #[derive(Default)]
    struct Stub {
        reads: RefCell<HashMap<Pubkey, VecDeque<Result<Option<MarketSeats>>>>>,
        sent: RefCell<Vec<Instruction>>,
        /// Sends touching this market fail.
        rejects: Option<Pubkey>,
    }

    impl Stub {
        fn script(&self, market: &Market, read: Result<Option<MarketSeats>>) {
            self.reads
                .borrow_mut()
                .entry(market.address)
                .or_default()
                .push_back(read);
        }
    }

    impl Rpc for Stub {
        fn seats(&self, _: Op, market: &Pubkey, _: &Pubkey) -> Result<Option<MarketSeats>> {
            self.reads
                .borrow_mut()
                .get_mut(market)
                .and_then(VecDeque::pop_front)
                .unwrap_or_else(|| panic!("an unscripted read of {market}"))
        }

        fn send(
            &self,
            _: &Keypair,
            _: &Keypair,
            ixs: &[Instruction],
            _: &str,
            _: &Logger,
        ) -> Result<()> {
            let write = ixs.last().unwrap().clone();
            if let Some(rejected) = self.rejects {
                if write.accounts.iter().any(|a| a.pubkey == rejected) {
                    bail!("the stub rejects this market");
                }
            }
            self.sent.borrow_mut().push(write);
            Ok(())
        }
    }

    fn resolved() -> Market {
        Market {
            address: Pubkey::new_unique(),
            base: Pubkey::new_unique(),
            quote: Pubkey::new_unique(),
            signer: Keypair::new(),
        }
    }

    fn holding(seats: Vec<SectorSeat>) -> Result<Option<MarketSeats>> {
        Ok(Some(MarketSeats {
            base_treasury: Pubkey::new_unique(),
            quote_treasury: Pubkey::new_unique(),
            seats,
        }))
    }

    fn vault(idx: u32, seq: u64) -> SectorSeat {
        SectorSeat {
            idx,
            seq,
            ..seat(Pubkey::new_unique())
        }
    }

    /// The close the job should send for sector `idx` of `market`.
    fn close(market: &Market, idx: u32) -> Instruction {
        chain::build_close_vault_ix(
            Cluster::Localnet,
            &market.signer.pubkey(),
            &market.address,
            idx,
        )
    }

    /// The kill the job should send for sector `idx` of `market`.
    fn kill(market: &Market, idx: u32) -> Instruction {
        set_liquidity_profile_ix(
            Cluster::Localnet.deployment(),
            market.signer.pubkey(),
            market.address,
            idx,
            [0u8; PROFILE_BYTES],
        )
    }

    /// Run `op`'s job over `markets`, returning its outcome and its log lines.
    fn run(
        op: Op,
        stub: &Stub,
        markets: &[(&'static PairConfig, Result<Market>)],
        planned: Option<&Planned>,
    ) -> (Result<String>, Vec<String>) {
        let (tx, rx) = mpsc::channel();
        let outcome = execute_over(
            op,
            stub,
            Cluster::Localnet,
            &Keypair::new(),
            markets,
            planned,
            &Logger::new(tx),
        );
        let lines = rx
            .try_iter()
            .filter_map(|event| match event {
                JobEvent::Log(line) => Some(line),
                _ => None,
            })
            .collect();
        (outcome, lines)
    }

    #[test]
    fn the_job_closes_exactly_the_vaults_the_plan_listed() {
        let config = market::roster(Cluster::Localnet)[0];
        let (a, b, c, d) = (resolved(), resolved(), resolved(), resolved());
        let stub = Stub::default();
        // A: vault #2 goes and vault #3 opens in its recycled sector between
        // the prompt and Enter — keyed by sector, the job would close it.
        stub.script(&a, holding(vec![vault(0, 1), vault(1, 2)]));
        stub.script(&a, holding(vec![vault(0, 1), vault(1, 3)]));
        // B: the plan could not read it, so the job reads it and acts.
        stub.script(&b, Err(anyhow!("unreachable")));
        stub.script(&b, holding(vec![vault(0, 5)]));
        // C: closed by someone else in the window — nothing left to send.
        stub.script(&c, holding(vec![vault(4, 9)]));
        stub.script(
            &c,
            holding(vec![SectorSeat {
                tombstoned: true,
                ..vault(4, 9)
            }]),
        );
        // D: no market when planned; created, with a vault, before Enter.
        stub.script(&d, Ok(None));
        stub.script(&d, holding(vec![vault(0, 1)]));
        let markets = vec![
            (config, Ok(a)),
            (config, Ok(b)),
            (config, Ok(c)),
            (config, Ok(d)),
        ];
        let market = |i: usize| markets[i].1.as_ref().unwrap();

        let (lines, planned) =
            plan_over(Op::Close, &stub, &markets, &Pubkey::new_unique()).unwrap();
        assert!(lines.iter().any(|l| l.contains("read failed")), "{lines:?}");
        assert_eq!(
            planned,
            Planned(BTreeMap::from([
                (market(0).address, BTreeSet::from([1, 2])),
                (market(2).address, BTreeSet::from([9])),
                (market(3).address, BTreeSet::new()),
            ]))
        );

        let (outcome, log) = run(Op::Close, &stub, &markets, Some(&planned));
        assert_eq!(
            *stub.sent.borrow(),
            [close(market(0), 0), close(market(1), 0)]
        );
        let logged = |needle: &str| log.iter().filter(|l| l.contains(needle)).count();
        assert_eq!(
            logged("vault #2: no longer found since the plan"),
            1,
            "{log:?}"
        );
        assert_eq!(logged("vault #3: opened since the plan"), 1, "{log:?}");
        assert_eq!(logged("vault #1: opened since the plan"), 1, "{log:?}");
        // The vaults left alone turn the job red: the stop is not complete.
        let err = format!("{:#}", outcome.unwrap_err());
        assert!(err.contains("landed on 2 vault(s)"), "{err}");
        assert!(err.contains("2 opened since the plan"), "{err}");
    }

    #[test]
    fn a_kill_leaves_an_unplanned_ladder_alone_but_never_counts_a_dead_one() {
        let config = market::roster(Cluster::Localnet)[0];
        let a = resolved();
        let me = a.signer.pubkey();
        let quoting = |idx, seq| SectorSeat {
            idx,
            seq,
            ..seat(me)
        };
        let foreign = |idx, seq| SectorSeat {
            quote_authority: Pubkey::new_unique(),
            ..quoting(idx, seq)
        };
        let stub = Stub::default();
        stub.script(&a, holding(vec![quoting(0, 1)]));
        // Opened since: #2 with a dead ladder, nothing to send.
        stub.script(
            &a,
            holding(vec![
                quoting(0, 1),
                SectorSeat {
                    quoting: false,
                    ..quoting(1, 2)
                },
            ]),
        );
        let markets = vec![(config, Ok(a))];
        let a = markets[0].1.as_ref().unwrap();
        let (_, planned) = plan_over(Op::Kill, &stub, &markets, &me).unwrap();
        let (outcome, log) = run(Op::Kill, &stub, &markets, Some(&planned));
        // No plan was needed for #2, so the kill reads as complete.
        assert_eq!(
            outcome.unwrap(),
            "set_liquidity_profile landed on 1 vault(s)"
        );
        assert!(!log.iter().any(|l| l.contains("opened since")), "{log:?}");
        assert_eq!(*stub.sent.borrow(), [kill(a, 0)]);

        // A refusal the operator read and a quoting ladder opened since: both
        // turn the job red, and only the planned ladder is zeroed.
        let stub = Stub::default();
        stub.script(a, holding(vec![quoting(0, 1), foreign(2, 3)]));
        stub.script(
            a,
            holding(vec![quoting(0, 1), foreign(2, 3), quoting(3, 4)]),
        );
        let (_, planned) = plan_over(Op::Kill, &stub, &markets, &me).unwrap();
        let (outcome, log) = run(Op::Kill, &stub, &markets, Some(&planned));
        assert_eq!(*stub.sent.borrow(), [kill(a, 0)]);
        assert!(
            log.iter().any(|l| l.contains("vault #4: opened since")),
            "{log:?}"
        );
        let err = format!("{:#}", outcome.unwrap_err());
        assert!(
            err.contains("landed on 1 vault(s); 1 failed or were refused; 1 opened since"),
            "{err}"
        );
    }

    #[test]
    fn without_a_plan_the_job_acts_on_its_read_and_one_failure_spares_the_rest() {
        let config = market::roster(Cluster::Localnet)[0];
        let (a, b) = (resolved(), resolved());
        let stub = Stub {
            rejects: Some(a.address),
            ..Stub::default()
        };
        stub.script(&a, holding(vec![vault(0, 1)]));
        stub.script(&b, holding(vec![vault(0, 1), vault(3, 4)]));
        let markets = vec![
            (config, Ok(a)),
            (config, Err(anyhow!("no leader key"))),
            (config, Ok(b)),
        ];
        let b = markets[2].1.as_ref().unwrap();

        let (outcome, log) = run(Op::Close, &stub, &markets, None);
        assert_eq!(*stub.sent.borrow(), [close(b, 0), close(b, 3)]);
        assert!(log.iter().any(|l| l.contains("no leader key")), "{log:?}");
        let err = format!("{:#}", outcome.unwrap_err());
        assert!(
            err.contains("landed on 2 vault(s); 2 failed or were refused"),
            "{err}"
        );

        // Every vault already done is a red "nothing was sent", not a success.
        let stub = Stub::default();
        let a = resolved();
        stub.script(
            &a,
            holding(vec![SectorSeat {
                tombstoned: true,
                ..vault(0, 1)
            }]),
        );
        let (outcome, _) = run(Op::Close, &stub, &[(config, Ok(a))], None);
        assert!(format!("{:#}", outcome.unwrap_err()).contains("nothing was sent"));
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
