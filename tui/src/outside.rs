//! Opening a vault to outside depositors: the two-key gate the program
//! reads at `deposit` (`set_outside_deposits.rs`).
//!
//! Both halves start unset at `create_vault`, so a vault the cockpit
//! creates takes no outside capital until each side opts in: the leader
//! through `set_allow_outside_depositors`, then a registry admin through
//! `set_outside_deposits_approved`. [`open()`] sends the leader's half and
//! reads it back, then the admin's when this session's wallet is in the
//! registry's admin set — and when it is not, stops there and says the
//! admin half is outstanding, rather than send a write the program
//! refuses. Each half already set is skipped, so a re-run finishes a
//! partial open and a full one sends nothing.

use crate::accounts::{self, VaultSeat};
use crate::chain;
use crate::cluster::Cluster;
use crate::job::Logger;
use anyhow::{bail, Context, Result};
use solana_client::rpc_client::RpcClient;
use solana_pubkey::Pubkey;
use solana_signer::Signer;

/// What [`open()`] still has to send for `seat`, given whether the session
/// holds an admin key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    pub leader: bool,
    pub admin: bool,
    /// The admin half is unset and no admin key is held to set it.
    pub admin_outstanding: bool,
}

/// The sends that open `seat`: each half only when it is unset, the
/// admin's only when `holds_admin`.
pub fn plan(seat: &VaultSeat, holds_admin: bool) -> Plan {
    Plan {
        leader: !seat.allow_outside_depositors,
        admin: !seat.outside_deposits_approved && holds_admin,
        admin_outstanding: !seat.outside_deposits_approved && !holds_admin,
    }
}

/// Open `leader`'s vault on `market` to outside depositors: the leader's
/// half signed by `leader`, then the admin's signed by `wallet` when it is
/// a registry admin, each confirmed by a fresh read before the next.
/// `wallet` pays both. `symbol` names the market in the log and summary.
pub fn open(
    client: &RpcClient,
    wallet: &dyn Signer,
    leader: &dyn Signer,
    cluster: Cluster,
    market: &Pubkey,
    symbol: &str,
    log: &Logger,
) -> Result<String> {
    // The same rule `do_create_vault` holds: the vault's leader is never
    // this session's own wallet, so a key that is both refuses here too.
    if leader.pubkey() == wallet.pubkey() {
        bail!(
            "the leader must not be this session's wallet ({}) — nothing sent",
            wallet.pubkey()
        );
    }
    let seat = read(client, cluster, market, &leader.pubkey())?;
    let holds_admin = accounts::registry_admins_fresh(client, cluster)?.contains(&wallet.pubkey());
    let todo = plan(&seat, holds_admin);
    if todo.leader {
        let ix = chain::build_set_allow_outside_depositors_ix(
            cluster,
            &leader.pubkey(),
            market,
            seat.idx,
            true,
        );
        chain::send_logged(
            client,
            wallet,
            &[wallet, leader],
            &[ix],
            "set_allow_outside_depositors",
            log,
        )
        .context("send set_allow_outside_depositors")?;
        // Announced per send, so a failed admin half below still leaves the
        // panel showing the leader half that landed.
        log.accounts_changed();
        confirm(
            client,
            cluster,
            market,
            &seat,
            |s| s.allow_outside_depositors,
            "leader",
        )?;
    }
    if todo.admin {
        let ix = chain::build_set_outside_deposits_approved_ix(
            cluster,
            &wallet.pubkey(),
            market,
            seat.idx,
            true,
        );
        chain::send_logged(
            client,
            wallet,
            &[wallet],
            &[ix],
            "set_outside_deposits_approved",
            log,
        )
        .context("send set_outside_deposits_approved")?;
        log.accounts_changed();
        confirm(
            client,
            cluster,
            market,
            &seat,
            |s| s.outside_deposits_approved,
            "admin",
        )?;
    }
    // What the reads above verified: the leader half always holds here, and
    // the admin half unless it is the outstanding one.
    log.record(
        "vault-outside-deposits-verified",
        &[
            ("market", &market.to_string()),
            ("vault_seq", &seat.seq.to_string()),
            ("allow_outside_depositors", "true"),
            (
                "outside_deposits_approved",
                if todo.admin_outstanding {
                    "false"
                } else {
                    "true"
                },
            ),
        ],
    );
    let vault = format!("{symbol} vault {}", seat.label());
    Ok(summary(todo, &vault, &wallet.pubkey()))
}

/// The job's closing line for `vault` after `todo` ran: the admin half
/// outstanding names the wallet that could not sign it.
fn summary(todo: Plan, vault: &str, wallet: &Pubkey) -> String {
    match todo {
        Plan {
            admin_outstanding: true,
            ..
        } => format!(
            "{vault}: leader half set; the admin half is outstanding — this session's \
             wallet {wallet} is not a registry admin"
        ),
        Plan {
            leader: false,
            admin: false,
            ..
        } => format!("{vault} is already open to outside depositors — nothing sent"),
        _ => format!("{vault} is open to outside depositors"),
    }
}

/// `leader`'s one vault on `market`, read fresh. A frozen or closed vault
/// is refused: it takes no deposit whatever the gate says.
fn read(
    client: &RpcClient,
    cluster: Cluster,
    market: &Pubkey,
    leader: &Pubkey,
) -> Result<VaultSeat> {
    let seats = accounts::vault_seats_fresh(client, cluster, market)?
        .context("market not found — create the market first")?;
    let seat = accounts::sole_seat_led_by(&seats, leader)
        .with_context(|| format!("{leader} does not lead exactly one vault on {market}"))?;
    if seat.stake.winding_down {
        bail!(
            "vault {} is frozen or closed — it takes no deposits",
            seat.label()
        );
    }
    Ok(seat)
}

/// Re-read `seat` after a send and refuse unless `set` now holds — the
/// sector recycled, or the write did not land.
fn confirm(
    client: &RpcClient,
    cluster: Cluster,
    market: &Pubkey,
    seat: &VaultSeat,
    set: impl Fn(&VaultSeat) -> bool,
    half: &str,
) -> Result<()> {
    let now = read(client, cluster, market, &seat.leader)?;
    if now.idx != seat.idx || now.seq != seat.seq {
        bail!(
            "vault {} moved during the open — re-run to finish",
            seat.label()
        );
    }
    if !set(&now) {
        bail!(
            "vault {}'s {half} half reads unset after the write",
            seat.label()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::VaultStake;
    use crate::naming::VAULT_NAME_LEN;

    fn seat(allowed: bool, approved: bool) -> VaultSeat {
        VaultSeat {
            idx: 0,
            seq: 1,
            leader: Pubkey::new_unique(),
            name: [0; VAULT_NAME_LEN],
            name_locked: false,
            allow_outside_depositors: allowed,
            outside_deposits_approved: approved,
            seeded: true,
            stake: VaultStake::default(),
        }
    }

    #[test]
    fn a_closed_vault_with_an_admin_key_sends_both_halves() {
        assert_eq!(
            plan(&seat(false, false), true),
            Plan {
                leader: true,
                admin: true,
                admin_outstanding: false
            }
        );
    }

    #[test]
    fn without_an_admin_key_only_the_leader_half_is_sent() {
        assert_eq!(
            plan(&seat(false, false), false),
            Plan {
                leader: true,
                admin: false,
                admin_outstanding: true
            }
        );
        // An admin half someone else set is not outstanding.
        assert!(!plan(&seat(false, true), false).admin_outstanding);
    }

    #[test]
    fn the_summary_names_an_outstanding_admin_half_and_its_wallet() {
        let wallet = Pubkey::new_unique();
        let s =
            |a: bool, b: bool, key: bool| summary(plan(&seat(a, b), key), "TGBP vault #1", &wallet);
        let outstanding = s(false, false, false);
        assert!(
            outstanding
                .starts_with("TGBP vault #1: leader half set; the admin half is outstanding"),
            "{outstanding}"
        );
        assert!(outstanding.contains(&wallet.to_string()), "{outstanding}");
        assert_eq!(
            s(true, true, true),
            "TGBP vault #1 is already open to outside depositors — nothing sent"
        );
        assert_eq!(
            s(false, false, true),
            "TGBP vault #1 is open to outside depositors"
        );
        // Outstanding wins even when nothing else was left to send.
        assert!(s(true, false, false).contains("outstanding"));
    }

    #[test]
    fn a_set_half_is_never_resent() {
        let open = plan(&seat(true, true), true);
        assert!(!open.leader && !open.admin && !open.admin_outstanding);
        // A partial open finishes with the missing half only.
        assert_eq!(
            plan(&seat(true, false), true),
            Plan {
                leader: false,
                admin: true,
                admin_outstanding: false
            }
        );
    }
}
