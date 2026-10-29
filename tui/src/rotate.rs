//! The leader's custody rotation (`set_leader`): hand the session leader's
//! own vault on the selected market to a new key, which co-signs.
//!
//! The prompt shares the rename's shape (see [`crate::naming`]): a form, then
//! a ticket sized from a fresh read, sent only after the typed `yes` — under
//! the red real-funds border on mainnet. The form takes the incoming key the
//! way the session takes every key on its cluster: on mainnet its address,
//! signed through the SSH agent that serves it, so the cockpit never holds
//! the secret; on localnet a keypair file.
//!
//! [`refusal`] is the program's own rule set, checked against the fresh read
//! so the ticket never asks for a `yes` the chain would refuse: the vault is
//! not frozen, the incoming key is neither the current leader nor the zero
//! address, leads no open vault on the market, and holds no depositor
//! position on this one. Only the leader moves — the quote authority, the
//! shares and the vault's number stay — so the maker keeps quoting through
//! the rotation, and the vault keeps the number a person knows it by.
//!
//! After the write the session's own leader key no longer leads the vault,
//! so the leader commands refuse it until the cockpit is relaunched with the
//! new key as its leader; the result line says so.

use crate::accounts::{self, MarketView, VaultSeat};
use crate::chain;
use crate::cluster::Cluster;
use crate::job::Logger;
use crate::keys::{self, RoleKey};
use crate::wallet;
use anyhow::{bail, Context, Result};
use solana_client::rpc_client::RpcClient;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use std::path::Path;
use std::sync::Arc;

/// The label every rotation line carries.
pub const LABEL: &str = "Rotate leader";

/// The longest key the form takes — room for a keypair file path, not a
/// paste of a secret's bytes.
const MAX_KEY_CHARS: usize = 256;

/// The rotation prompt's editable stage: the incoming key, as typed.
#[derive(Clone, Debug, Default)]
pub struct Form {
    pub key: String,
}

impl Form {
    /// Type `c` into the key: printable ASCII only, up to `MAX_KEY_CHARS`.
    pub fn type_char(&mut self, c: char) {
        if c.is_ascii_graphic() && self.key.len() < MAX_KEY_CHARS {
            self.key.push(c);
        }
    }

    pub fn backspace(&mut self) {
        self.key.pop();
    }

    /// The form as display lines.
    pub fn lines(&self, cluster: Cluster) -> Vec<String> {
        vec![format!(
            "\u{25b8} {} {}\u{2588}",
            what_to_type(cluster),
            self.key
        )]
    }
}

/// What the form's one line asks for on `cluster`.
pub fn what_to_type(cluster: Cluster) -> &'static str {
    if cluster.is_mainnet() {
        "new leader address"
    } else {
        "new leader keypair file"
    }
}

/// The incoming leader's signer from `typed`: on mainnet the agent-served key
/// at that address — refused when no agent serves it, so the co-sign can
/// happen — and on localnet the keypair file, a relative path read from the
/// repo root as the committed `keys/` are.
pub fn new_leader_key(cluster: Cluster, repo_root: &Path, typed: &str) -> Result<RoleKey> {
    let typed = typed.trim();
    if typed.is_empty() {
        bail!("enter the {}", what_to_type(cluster));
    }
    if cluster.is_mainnet() {
        let (key, _) = wallet::resolve_key(typed, "the new leader", None)?;
        return Ok(key);
    }
    let path = if Path::new(typed).is_relative() && !typed.starts_with('~') {
        repo_root.join(typed).display().to_string()
    } else {
        typed.to_string()
    };
    Ok(Arc::new(wallet::load_leader(&path)?))
}

/// A sized rotation, read fresh, awaiting the typed `yes`.
#[derive(Clone, Debug)]
pub struct Ticket {
    pub market: Pubkey,
    pub base_mint: Pubkey,
    pub symbol: String,
    /// The vault's sector at the read — what the instruction addresses —
    /// and its number, what a person is shown. The execute step re-reads and
    /// refuses when the sector no longer holds that number.
    pub vault_idx: u32,
    pub vault_seq: u64,
    pub vault_label: String,
    pub old_leader: Pubkey,
    pub new_leader: Pubkey,
    /// The form's key as typed, re-resolved by the job: a signer is not
    /// carried across threads in a ticket.
    pub typed: String,
}

impl Ticket {
    /// The confirm stage's lines.
    pub fn summary(&self, cluster: Cluster) -> Vec<String> {
        let relaunch = if cluster.is_mainnet() {
            format!(
                "relaunch with {} (or --leader) set to the new address",
                keys::LEADER_VAR
            )
        } else {
            "wipe the ledger to restore the roster's committed leader".to_string()
        };
        vec![
            format!("market   {} ({})", self.symbol, self.market),
            format!("vault    #{} ({})", self.vault_seq, self.vault_label),
            format!("leader   {}", self.old_leader),
            format!("      \u{2192} {}", self.new_leader),
            "quoting  unchanged — the quote authority rotates on its own".to_string(),
            "signers  the current leader, and the new one co-signing — its signature \
             proves it is live"
                .to_string(),
            format!(
                "warning: this session's leader key stops leading vault #{} — {relaunch}",
                self.vault_seq
            ),
        ]
    }
}

/// Why `seat` among `seats` cannot be handed to `new_leader`, if it cannot —
/// the program's own refusals, in the order it checks them.
pub fn refusal(seats: &[VaultSeat], seat: &VaultSeat, new_leader: &Pubkey) -> Option<String> {
    if seat.frozen {
        return Some(format!(
            "vault #{} is frozen — a frozen vault's custody cannot move",
            seat.seq
        ));
    }
    if *new_leader == Pubkey::default() {
        return Some("the zero address cannot lead a vault".to_string());
    }
    if *new_leader == seat.leader {
        return Some(format!(
            "{new_leader} already leads vault #{} — nothing to send",
            seat.seq
        ));
    }
    // One open vault per leader per market. A frozen vault counts whether or
    // not it is closed; an unfrozen closed one does not.
    let open = |s: &&VaultSeat| s.frozen || !s.stake.winding_down;
    if let Some(other) = seats.iter().filter(open).find(|s| s.leader == *new_leader) {
        return Some(format!(
            "{new_leader} already leads vault #{} on this market — one open vault per leader",
            other.seq
        ));
    }
    None
}

/// Size the rotation of the vault `leader` leads on `market` to the key
/// `form` names, reading the vaults and the new key's depositor position
/// fresh. Refuses whatever the program would.
pub fn prepare(
    client: &RpcClient,
    cluster: Cluster,
    repo_root: &Path,
    market: &MarketView,
    symbol: &str,
    leader: &Pubkey,
    form: &Form,
) -> Result<Ticket> {
    let new_leader = new_leader_key(cluster, repo_root, &form.key)?.pubkey();
    if cluster.is_mainnet() {
        wallet::refuse_committed(
            &new_leader,
            "the new leader",
            &wallet::committed_pubkeys(repo_root),
        )?;
    }
    let seats = accounts::vault_seats_fresh(client, cluster, &market.address)?
        .context("the market is gone")?;
    let seat = accounts::sole_seat_led_by(&seats, leader)
        .with_context(|| format!("{leader} does not lead exactly one vault here"))?;
    if let Some(reason) = refusal(&seats, &seat, &new_leader) {
        bail!(reason);
    }
    let depositor = chain::vault_depositor_pda(cluster, &market.address, seat.idx, &new_leader);
    let position = chain::fetch_fresh(client, &depositor, "the new leader's depositor position")?;
    if position.is_some_and(|a| a.owner == cluster.program_id()) {
        bail!(
            "{new_leader} holds a depositor position on vault #{} — a leader never also \
             deposits into its own vault; withdraw it first",
            seat.seq
        );
    }
    Ok(Ticket {
        market: market.address,
        base_mint: market.base_mint,
        symbol: symbol.to_string(),
        vault_idx: seat.idx,
        vault_seq: seat.seq,
        vault_label: seat.label(),
        old_leader: *leader,
        new_leader,
        typed: form.key.clone(),
    })
}

/// Send a confirmed `ticket`, signed by the vault's current `leader` and the
/// `new_leader`, `payer` paying — then read the vault back. Re-reads first
/// and refuses when the sector no longer holds the ticket's vault, or it has
/// changed hands since.
pub fn execute(
    client: &RpcClient,
    payer: &dyn Signer,
    leader: &dyn Signer,
    new_leader: &dyn Signer,
    cluster: Cluster,
    ticket: &Ticket,
    log: &Logger,
) -> Result<String> {
    for (key, want, role) in [
        (leader.pubkey(), ticket.old_leader, "current leader"),
        (new_leader.pubkey(), ticket.new_leader, "new leader"),
    ] {
        if key != want {
            bail!("the {role} key {key} is not the ticket's {want} — nothing was sent");
        }
    }
    let read = || -> Result<Option<VaultSeat>> {
        let seats = accounts::vault_seats_fresh(client, cluster, &ticket.market)?
            .context("the market is gone")?;
        Ok(seats
            .into_iter()
            .find(|s| s.idx == ticket.vault_idx && s.seq == ticket.vault_seq))
    };
    let Some(now) = read()? else {
        bail!(
            "vault #{} is gone since the preview — nothing was sent; re-open the prompt",
            ticket.vault_seq
        );
    };
    if now.leader != ticket.old_leader {
        bail!(
            "vault #{} is led by {} since the preview — nothing was sent",
            ticket.vault_seq,
            now.leader
        );
    }
    let ix = chain::build_set_leader_ix(
        cluster,
        &ticket.old_leader,
        &ticket.new_leader,
        &ticket.market,
        ticket.vault_idx,
    );
    let mut signers: Vec<&dyn Signer> = vec![payer];
    for s in [leader, new_leader] {
        if s.pubkey() != payer.pubkey() {
            signers.push(s);
        }
    }
    chain::send_logged(client, payer, &signers, &[ix], "set_leader", log)
        .context("send set_leader")?;
    log.accounts_changed();
    let seat = read()?.context("the vault is not readable after the write")?;
    if seat.leader != ticket.new_leader {
        bail!(
            "vault #{} is led by {} after the write",
            seat.seq,
            seat.leader
        );
    }
    log.record(
        "vault-leader-rotated",
        &[
            ("market", &ticket.market.to_string()),
            ("vault_seq", &ticket.vault_seq.to_string()),
            ("old_leader", &ticket.old_leader.to_string()),
            ("new_leader", &ticket.new_leader.to_string()),
        ],
    );
    Ok(format!(
        "{} vault #{} is now led by {} — relaunch with it as the leader to manage the vault",
        ticket.symbol, ticket.vault_seq, ticket.new_leader
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::VaultStake;
    use crate::naming::VAULT_NAME_LEN;

    fn seat(idx: u32, leader: Pubkey) -> VaultSeat {
        VaultSeat {
            idx,
            seq: u64::from(idx) + 10,
            leader,
            name: [0; VAULT_NAME_LEN],
            name_locked: false,
            frozen: false,
            allow_outside_depositors: false,
            outside_deposits_approved: false,
            seeded: true,
            stake: VaultStake::default(),
        }
    }

    #[test]
    fn a_rotation_to_a_fresh_key_is_allowed() {
        let (me, new) = (Pubkey::new_unique(), Pubkey::new_unique());
        let seats = [seat(0, Pubkey::new_unique()), seat(3, me)];
        assert_eq!(refusal(&seats, &seats[1], &new), None);
    }

    #[test]
    fn a_frozen_vault_the_zero_key_and_the_same_key_are_refused() {
        let me = Pubkey::new_unique();
        let mut frozen = seat(0, me);
        frozen.frozen = true;
        frozen.stake.winding_down = true;
        let err = refusal(&[frozen], &frozen, &Pubkey::new_unique()).unwrap();
        assert!(err.contains("frozen") && err.contains("#10"), "{err}");
        let open = seat(0, me);
        assert!(refusal(&[open], &open, &Pubkey::default())
            .unwrap()
            .contains("zero"));
        assert!(refusal(&[open], &open, &me)
            .unwrap()
            .contains("nothing to send"));
    }

    #[test]
    fn a_key_leading_an_open_or_frozen_vault_is_refused_and_a_closed_one_is_not() {
        let (me, other) = (Pubkey::new_unique(), Pubkey::new_unique());
        let mine = seat(0, me);
        let theirs = seat(1, other);
        let err = refusal(&[mine, theirs], &mine, &other).unwrap();
        assert!(
            err.contains("#11") && err.contains("one open vault"),
            "{err}"
        );
        // Closed and unfrozen: the leader walked away, and is free again.
        let mut closed = theirs;
        closed.stake.winding_down = true;
        assert_eq!(refusal(&[mine, closed], &mine, &other), None);
        // Frozen counts whether or not it is closed.
        let mut frozen = closed;
        frozen.frozen = true;
        assert!(refusal(&[mine, frozen], &mine, &other).is_some());
    }

    #[test]
    fn the_form_takes_printable_ascii_only() {
        let mut f = Form::default();
        for c in "keys/A\u{e9} B.json\t".chars() {
            f.type_char(c);
        }
        assert_eq!(f.key, "keys/AB.json");
        f.backspace();
        assert_eq!(f.key, "keys/AB.jso");
        for _ in 0..400 {
            f.type_char('x');
        }
        assert_eq!(f.key.len(), MAX_KEY_CHARS);
    }

    #[test]
    fn a_localnet_key_file_resolves_from_the_repo_root() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let key = new_leader_key(Cluster::Localnet, &root, " keys/FFFF.json ").unwrap();
        let direct = solana_keypair::read_keypair_file(root.join("keys/FFFF.json")).unwrap();
        assert_eq!(key.pubkey(), direct.pubkey());
        assert!(new_leader_key(Cluster::Localnet, &root, "  ").is_err());
        assert!(new_leader_key(Cluster::Localnet, &root, "keys/none.json").is_err());
    }

    #[test]
    fn the_ticket_shows_the_vault_number_and_never_its_sector() {
        let t = Ticket {
            market: Pubkey::new_unique(),
            base_mint: Pubkey::new_unique(),
            symbol: "TGBP".into(),
            vault_idx: 4,
            vault_seq: 7,
            vault_label: "Desk".into(),
            old_leader: Pubkey::new_unique(),
            new_leader: Pubkey::new_unique(),
            typed: String::new(),
        };
        for cluster in [Cluster::Localnet, Cluster::Mainnet] {
            let lines = t.summary(cluster);
            assert!(lines.iter().any(|l| l.contains("#7 (Desk)")));
            assert!(lines.iter().all(|l| !l.contains("sector")), "{lines:?}");
            assert!(lines.iter().any(|l| l.contains(&t.new_leader.to_string())));
        }
        assert!(t
            .summary(Cluster::Mainnet)
            .iter()
            .any(|l| l.contains(keys::LEADER_VAR)));
    }
}
