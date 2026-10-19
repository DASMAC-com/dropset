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
    fn seats(
        self,
        client: &RpcClient,
        cluster: Cluster,
        market: &Pubkey,
        signer: &Pubkey,
    ) -> Result<Option<MarketSeats>> {
        match self {
            Op::Kill => quote_seats(client, cluster, market, signer),
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

/// Every sector on `market` whose quote authority is `quote`, read fresh, as
/// [`leader_seats`] reads them. How the hot path finds its vault without the
/// leader key: the quote authority is what it signs as.
pub fn quote_seats(
    client: &RpcClient,
    cluster: Cluster,
    market: &Pubkey,
    quote: &Pubkey,
) -> Result<Option<MarketSeats>> {
    seats_where(client, cluster, market, |v| {
        v.quote_authority == quote.to_bytes()
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
/// signed by `signer` ([`Op::signer`]).
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
            Ok(Plan::Deposit { .. }) | Err(_) => {
                Step::Refuse("the stake does not size a withdrawal")
            }
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
    operator_leader: Option<&LazyKey>,
    operator_quote: Option<&Keypair>,
) -> Result<Vec<String>> {
    let mut lines = vec![op.headline().to_string()];
    let mut work = 0usize;
    let mut first_refusal = None;
    for config in market::roster(cluster) {
        let symbol = config.base.symbol;
        // One market's failure to resolve or read must not hold the prompt
        // back from every other market's stop: it is a line in the plan, and
        // the job tries again. With nothing else to send, the first such
        // reason is what the refusal names (a missing key included).
        let resolved = op
            .signer(repo_root, config, operator_leader, operator_quote)
            .and_then(|signer| Ok((signer.pubkey(), market::pair_mints(repo_root, config)?)));
        let (signer, (base, quote)) = match resolved {
            Ok(resolved) => resolved,
            Err(e) => {
                let reason = format!("{symbol}: {e:#}");
                lines.push(reason.clone());
                first_refusal.get_or_insert(reason);
                continue;
            }
        };
        let address = chain::market_pda(cluster, &base, &quote);
        let found = match op.seats(client, cluster, &address, &signer) {
            Ok(Some(found)) => found,
            Ok(None) => {
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
        if found.seats.is_empty() {
            lines.push(format!(
                "{symbol}: {} {signer} holds no vault — skipped",
                op.signer_role()
            ));
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
    Ok(lines)
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
/// op's key ([`Op::signer`]) and paid by `wallet`. Every market is attempted;
/// the job fails at the end if any send failed or any vault was refused, so
/// the operator sees the outcome in red rather than a partial stop reading as
/// success.
#[allow(clippy::too_many_arguments)]
pub fn execute(
    op: Op,
    client: &RpcClient,
    wallet: &Keypair,
    repo_root: &Path,
    cluster: Cluster,
    operator_leader: Option<&LazyKey>,
    operator_quote: Option<&Keypair>,
    log: &Logger,
) -> Result<String> {
    let mut sent = 0usize;
    let mut failed = 0usize;
    for config in market::roster(cluster) {
        let symbol = config.base.symbol;
        let resolved = op
            .signer(repo_root, config, operator_leader, operator_quote)
            .and_then(|signer| Ok((signer, market::pair_mints(repo_root, config)?)));
        let (signer, (base, quote)) = match resolved {
            Ok(resolved) => resolved,
            Err(e) => {
                log.log(format!("{symbol}: {e:#}"));
                failed += 1;
                continue;
            }
        };
        let address = chain::market_pda(cluster, &base, &quote);
        let found = match op.seats(client, cluster, &address, &signer.pubkey()) {
            Ok(Some(found)) => found,
            Ok(None) => continue,
            Err(e) => {
                log.log(format!("{symbol}: {e:#}"));
                failed += 1;
                continue;
            }
        };
        for seat in &found.seats {
            let ixs = match step(op, seat, &signer.pubkey()) {
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
                    &signer.pubkey(),
                    &address,
                    &base,
                    &quote,
                    &found,
                    seat,
                ),
            };
            let label = op.instruction();
            match chain::send_logged(client, wallet, &[wallet, &signer], &ixs, label, log)
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
/// program. `leader` is the step's signer: the quote authority for a kill,
/// the leader for the rest.
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
    seat: &SectorSeat,
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
        Step::Drain {
            shares_in,
            min_base_out,
            min_quote_out,
        } => {
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
        let err = Op::Kill.signer(root, config, Some(&leader), None).unwrap_err();
        assert!(format!("{err:#}").contains("--quote"), "{err:#}");
        assert_eq!(leader.resolved_pubkey(), None);
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
        // The kill's SDK builder stamps the committed program id; a missing
        // retarget would compile and send the mainnet kill nowhere.
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
