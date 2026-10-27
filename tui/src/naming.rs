//! Vault names in the cockpit: the name typed at create-vault, the leader's
//! rename (`set_vault_name`) and the admin override with its lock
//! (`override_vault_name`).
//!
//! [`validate`] is the program's own rule set (`encode_vault_name`) — at
//! most [`VAULT_NAME_LEN`] bytes of printable ASCII, empty meaning unset —
//! so a name the panel accepts is one the chain accepts, and the ceremony
//! never sends a create that fails on its name. Uniqueness is checked here
//! too, against a fresh read ([`taken_by`]), though the program's walk is
//! the authority.
//!
//! Rename and override share the leader prompt's shape: a form, then a
//! ticket sized from a fresh read, sent only after the typed `yes` — under
//! the red real-funds border on mainnet. The create name has no ticket of
//! its own: it is an argument of the create-vault step, shown in that
//! step's plan and confirmed with it.

use crate::accounts::{self, MarketView, VaultSeat};
use crate::chain;
use crate::cluster::Cluster;
use crate::job::Logger;
use anyhow::{bail, Context, Result};
use solana_client::rpc_client::RpcClient;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;

/// The on-chain name field's width, in bytes.
pub const VAULT_NAME_LEN: usize = 32;

/// How wide a name column is drawn; a longer name is clamped with `…`.
pub const NAME_COLUMN_WIDTH: usize = 16;

/// Refuse `name` unless the program would store it: at most
/// [`VAULT_NAME_LEN`] bytes, every one printable ASCII (`0x20..=0x7E`).
/// Empty is valid — the unset name.
pub fn validate(name: &str) -> Result<()> {
    if name.len() > VAULT_NAME_LEN {
        bail!(
            "a vault name is at most {VAULT_NAME_LEN} bytes (this one is {})",
            name.len()
        );
    }
    if !name.bytes().all(is_name_byte) {
        bail!("a vault name is printable ASCII only");
    }
    Ok(())
}

fn is_name_byte(b: u8) -> bool {
    (0x20..=0x7E).contains(&b)
}

/// The stored name as text, padding stripped; empty when unset. Lossy
/// rather than failing: the program writes ASCII only, so anything else is
/// a corrupt sector, shown rather than hidden.
pub fn decode(bytes: &[u8; VAULT_NAME_LEN]) -> String {
    let len = bytes.iter().position(|&b| b == 0).unwrap_or(VAULT_NAME_LEN);
    String::from_utf8_lossy(&bytes[..len]).into_owned()
}

/// What a person is shown for a vault: its name, or `#<seq>` — its
/// per-market number — when it has none. The frontend's vaults view renders
/// the same fallback.
pub fn label(name: &str, seq: u64) -> String {
    if name.is_empty() {
        format!("#{seq}")
    } else {
        name.to_string()
    }
}

/// `text` cut to `width` characters, the last one an ellipsis when cut.
pub fn clamp(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let kept: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}\u{2026}")
}

/// The other vault among `seats` already holding `name` under the
/// program's uniqueness rule — ASCII-case-insensitive, an unset name never
/// clashing — ignoring the vault at sector `own_idx`.
pub fn taken_by(seats: &[VaultSeat], name: &str, own_idx: u32) -> Option<VaultSeat> {
    if name.is_empty() {
        return None;
    }
    seats
        .iter()
        .find(|s| s.idx != own_idx && s.name().eq_ignore_ascii_case(name))
        .copied()
}

/// Which name write a prompt is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameOp {
    /// The name the create-vault step sends.
    Create,
    /// The leader renames its own vault.
    Rename,
    /// An admin replaces or blanks any vault's name, and sets the lock.
    Override,
}

impl NameOp {
    pub fn label(self) -> &'static str {
        match self {
            NameOp::Create => "Create vault",
            NameOp::Rename => "Rename vault",
            NameOp::Override => "EMERGENCY: override vault name",
        }
    }
}

/// A [`Form`] line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Vault,
    Name,
    Lock,
}

/// The name prompt's editable stage. Only the override has more than the
/// name: the target vault and the lock.
#[derive(Clone, Debug)]
pub struct Form {
    pub op: NameOp,
    pub name: String,
    /// The target vault, as typed (override only): its number, or its
    /// current name — what the operator saw on the vault they mean, even one
    /// past the accounts pane's listed rows.
    pub vault: String,
    /// The lock to set (override only). On by default: the override answers
    /// an abusive name, and an unlocked one the leader can simply rename
    /// back — so unlocking is the deliberate choice, not the default.
    pub lock: bool,
    pub field: Field,
}

impl Form {
    pub fn new(op: NameOp) -> Self {
        Self {
            op,
            name: String::new(),
            vault: String::new(),
            lock: op == NameOp::Override,
            field: if op == NameOp::Override {
                Field::Vault
            } else {
                Field::Name
            },
        }
    }

    fn fields(&self) -> &'static [Field] {
        match self.op {
            NameOp::Override => &[Field::Vault, Field::Name, Field::Lock],
            NameOp::Create | NameOp::Rename => &[Field::Name],
        }
    }

    /// Move the cursor `delta` lines, clamped.
    pub fn move_cursor(&mut self, delta: isize) {
        let fields = self.fields();
        let at = fields.iter().position(|f| *f == self.field).unwrap_or(0) as isize;
        let next = (at + delta).clamp(0, fields.len() as isize - 1) as usize;
        self.field = fields[next];
    }

    /// Type `c` into the current line. The name and the vault take only
    /// bytes the program stores, up to its width, so an invalid name cannot
    /// be typed at all; space flips the lock.
    pub fn type_char(&mut self, c: char) {
        let storable = c.is_ascii() && is_name_byte(c as u8);
        match self.field {
            Field::Name if storable && self.name.len() < VAULT_NAME_LEN => self.name.push(c),
            Field::Vault if storable && self.vault.len() < VAULT_NAME_LEN => self.vault.push(c),
            Field::Lock if c == ' ' => self.lock = !self.lock,
            _ => {}
        }
    }

    pub fn backspace(&mut self) {
        match self.field {
            Field::Name => {
                self.name.pop();
            }
            Field::Vault => {
                self.vault.pop();
            }
            Field::Lock => {}
        }
    }

    /// Flip the lock when it is the current line (`←` / `→`).
    pub fn toggle(&mut self) {
        if self.field == Field::Lock {
            self.lock = !self.lock;
        }
    }

    /// The form as display lines, the current one marked.
    pub fn lines(&self) -> Vec<String> {
        let mark = |f: Field| if self.field == f { "\u{25b8}" } else { " " };
        let cursor = |f: Field| if self.field == f { "\u{2588}" } else { "" };
        let mut lines = Vec::new();
        if self.op == NameOp::Override {
            lines.push(format!(
                "{} vault    {}{}   (its #number, or its current name)",
                mark(Field::Vault),
                self.vault,
                cursor(Field::Vault)
            ));
        }
        lines.push(format!(
            "{} name     {}{}   ({}/{VAULT_NAME_LEN})",
            mark(Field::Name),
            self.name,
            cursor(Field::Name),
            self.name.len()
        ));
        if self.op == NameOp::Override {
            lines.push(format!(
                "{} lock     {}",
                mark(Field::Lock),
                if self.lock {
                    "[x] locked — the leader cannot rename"
                } else {
                    "[ ] unlocked"
                }
            ));
        }
        lines
    }
}

/// Why `name` cannot be the create-vault step's name on `cluster`, if it
/// cannot. Mainnet requires one: a published vault is named, and the
/// ceremony record carries the name it was created with.
pub fn create_refusal(name: &str, cluster: Cluster) -> Option<String> {
    if let Err(e) = validate(name) {
        return Some(e.to_string());
    }
    if cluster.is_mainnet() && name.is_empty() {
        return Some("the mainnet ceremony needs a vault name".into());
    }
    None
}

/// A sized rename or override, read fresh, awaiting the typed `yes`.
#[derive(Clone, Debug)]
pub struct Ticket {
    pub op: NameOp,
    pub market: Pubkey,
    pub base_mint: Pubkey,
    pub symbol: String,
    /// The vault's sector at the read, and its number — the execute step
    /// re-reads and refuses when the sector no longer holds that number.
    pub vault_idx: u32,
    pub vault_seq: u64,
    pub old_name: String,
    pub name: String,
    /// The lock to set (override only; `false` on a rename).
    pub lock: bool,
    pub was_locked: bool,
    /// Who signs: the leader on a rename, the admin on an override.
    pub signer: Pubkey,
}

impl Ticket {
    /// The confirm stage's lines.
    pub fn summary(&self) -> Vec<String> {
        let shown = |n: &str| {
            if n.is_empty() {
                format!("(unset — shows as #{})", self.vault_seq)
            } else {
                format!("\"{n}\"")
            }
        };
        let mut lines = vec![
            format!("market   {} ({})", self.symbol, self.market),
            format!("vault    #{} (sector {})", self.vault_seq, self.vault_idx),
            format!(
                "name     {} \u{2192} {}",
                shown(&self.old_name),
                shown(&self.name)
            ),
        ];
        if self.op == NameOp::Override {
            let word = |l: bool| if l { "locked" } else { "unlocked" };
            lines.push(format!(
                "lock     {} \u{2192} {}",
                word(self.was_locked),
                word(self.lock)
            ));
        }
        let role = if self.op == NameOp::Override {
            "admin"
        } else {
            "leader"
        };
        lines.push(format!("signer   {} ({role})", self.signer));
        lines
    }
}

/// Size `form` against `market`, reading its vaults fresh. A rename targets
/// the one vault `signer` leads there; an override, the vault the form
/// numbers. Refuses whatever the program would — an invalid or taken name,
/// a locked name or a frozen or closed vault on a rename — and a write that
/// changes nothing.
pub fn prepare(
    client: &RpcClient,
    cluster: Cluster,
    market: &MarketView,
    symbol: &str,
    signer: &Pubkey,
    form: &Form,
) -> Result<Ticket> {
    validate(&form.name)?;
    let seats = accounts::vault_seats_fresh(client, cluster, &market.address)?
        .context("the market is gone")?;
    let seat = target(&seats, signer, form)?;
    ticket_for(&seats, seat, market, symbol, signer, form)
}

/// The seat `form` addresses among `seats`.
fn target(seats: &[VaultSeat], signer: &Pubkey, form: &Form) -> Result<VaultSeat> {
    match form.op {
        NameOp::Rename => {
            let seat = accounts::sole_seat_led_by(seats, signer)
                .with_context(|| format!("{signer} does not lead exactly one vault here"))?;
            // Frozen first: freeze also sets the lock, and "frozen" is the
            // truer reason of the two.
            if seat.stake.winding_down {
                bail!(
                    "vault #{} is frozen or closed — its name is final",
                    seat.seq
                );
            }
            if seat.name_locked {
                bail!(
                    "vault #{}'s name is locked by an admin — only an override changes it",
                    seat.seq
                );
            }
            Ok(seat)
        }
        NameOp::Override => find_vault(seats, &form.vault),
        NameOp::Create => bail!("the create name is sent by the create-vault step"),
    }
}

/// The vault `typed` names among `seats`: by number when it is one
/// (a leading `#` allowed), else by its current name under the uniqueness
/// rule — at most one vault can hold it.
fn find_vault(seats: &[VaultSeat], typed: &str) -> Result<VaultSeat> {
    let typed = typed.trim();
    if typed.is_empty() {
        bail!("enter the vault's #number or its current name");
    }
    let digits = typed.strip_prefix('#').unwrap_or(typed);
    if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
        if let Ok(seq) = digits.parse::<u64>() {
            if let Some(seat) = seats.iter().find(|s| s.seq == seq) {
                return Ok(*seat);
            }
        }
    }
    seats
        .iter()
        .find(|s| s.name().eq_ignore_ascii_case(typed))
        .copied()
        .with_context(|| format!("no live vault numbered or named \"{typed}\" on this market"))
}

fn ticket_for(
    seats: &[VaultSeat],
    seat: VaultSeat,
    market: &MarketView,
    symbol: &str,
    signer: &Pubkey,
    form: &Form,
) -> Result<Ticket> {
    if let Some(other) = taken_by(seats, &form.name, seat.idx) {
        bail!(
            "\"{}\" is taken on this market by vault #{}",
            form.name,
            other.seq
        );
    }
    let old_name = seat.name();
    let lock = form.op == NameOp::Override && form.lock;
    let unchanged =
        old_name == form.name && (form.op == NameOp::Rename || lock == seat.name_locked);
    if unchanged {
        bail!(
            "vault #{} already reads that way — nothing to send",
            seat.seq
        );
    }
    Ok(Ticket {
        op: form.op,
        market: market.address,
        base_mint: market.base_mint,
        symbol: symbol.to_string(),
        vault_idx: seat.idx,
        vault_seq: seat.seq,
        old_name,
        name: form.name.clone(),
        lock,
        was_locked: seat.name_locked,
        signer: *signer,
    })
}

/// Send a confirmed `ticket` — `signer` the leader on a rename, the admin
/// (`payer` itself) on an override — then read the vault back. Re-reads
/// first and refuses when the sector no longer holds the ticket's vault.
pub fn execute(
    client: &RpcClient,
    payer: &Keypair,
    signer: &Keypair,
    cluster: Cluster,
    ticket: &Ticket,
    log: &Logger,
) -> Result<String> {
    if signer.pubkey() != ticket.signer {
        bail!(
            "the session key {} is not the ticket's signer {} — nothing was sent",
            signer.pubkey(),
            ticket.signer
        );
    }
    let read = || -> Result<Option<VaultSeat>> {
        let seats = accounts::vault_seats_fresh(client, cluster, &ticket.market)?
            .context("the market is gone")?;
        Ok(seats
            .into_iter()
            .find(|s| s.idx == ticket.vault_idx && s.seq == ticket.vault_seq))
    };
    if read()?.is_none() {
        bail!(
            "vault #{} is no longer at sector {} — nothing was sent; re-open the prompt",
            ticket.vault_seq,
            ticket.vault_idx
        );
    }
    let (ix, label) = match ticket.op {
        NameOp::Rename => (
            chain::build_set_vault_name_ix(
                cluster,
                &ticket.signer,
                &ticket.market,
                ticket.vault_idx,
                &ticket.name,
            ),
            "set_vault_name",
        ),
        NameOp::Override => (
            chain::build_override_vault_name_ix(
                cluster,
                &ticket.signer,
                &ticket.market,
                ticket.vault_idx,
                &ticket.name,
                ticket.lock,
            ),
            "override_vault_name",
        ),
        NameOp::Create => bail!("the create name is sent by the create-vault step"),
    };
    let signers: Vec<&Keypair> = if signer.pubkey() == payer.pubkey() {
        vec![payer]
    } else {
        vec![payer, signer]
    };
    chain::send_logged(client, payer, &signers, &[ix], label, log)
        .with_context(|| format!("send {label}"))?;
    log.accounts_changed();
    let seat = read()?.context("the vault is not readable after the write")?;
    if seat.name() != ticket.name
        || (ticket.op == NameOp::Override && seat.name_locked != ticket.lock)
    {
        bail!(
            "vault #{} reads \"{}\" (locked: {}) after the write",
            seat.seq,
            seat.name(),
            seat.name_locked
        );
    }
    log.record(
        "vault-name-verified",
        &[
            ("market", &ticket.market.to_string()),
            ("vault_seq", &ticket.vault_seq.to_string()),
            ("name", &ticket.name),
            ("name_locked", &seat.name_locked.to_string()),
        ],
    );
    Ok(format!(
        "{} vault #{} now shows as {}",
        ticket.symbol,
        ticket.vault_seq,
        seat.label()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::VaultStake;

    fn packed(name: &str) -> [u8; VAULT_NAME_LEN] {
        let mut out = [0; VAULT_NAME_LEN];
        out[..name.len()].copy_from_slice(name.as_bytes());
        out
    }

    fn seat(idx: u32, leader: Pubkey, name: &str) -> VaultSeat {
        VaultSeat {
            idx,
            seq: u64::from(idx) + 10,
            leader,
            name: packed(name),
            name_locked: false,
            seeded: true,
            stake: VaultStake::default(),
        }
    }

    fn market() -> MarketView {
        crate::accounts::tests::market(1, 1)
    }

    #[test]
    fn validate_matches_the_program_rules() {
        // The program's own cases (`state/market/name.rs`).
        assert!(validate("").is_ok());
        assert!(validate(" ~").is_ok());
        assert!(validate(&"x".repeat(VAULT_NAME_LEN)).is_ok());
        assert!(validate(&"x".repeat(VAULT_NAME_LEN + 1)).is_err());
        for bad in ["a\u{1f}", "a\u{7f}", "a\0b", "café", "tab\t"] {
            assert!(validate(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn decode_strips_padding_and_reads_a_full_field() {
        assert_eq!(decode(&packed("AUDD Desk")), "AUDD Desk");
        assert_eq!(decode(&[0; VAULT_NAME_LEN]), "");
        assert_eq!(decode(&[b'x'; VAULT_NAME_LEN]), "x".repeat(VAULT_NAME_LEN));
    }

    #[test]
    fn a_blank_name_shows_as_the_vault_number() {
        assert_eq!(label("", 7), "#7");
        assert_eq!(label("Desk", 7), "Desk");
    }

    #[test]
    fn clamp_cuts_with_an_ellipsis() {
        assert_eq!(clamp("short", 16), "short");
        assert_eq!(clamp(&"x".repeat(16), 16), "x".repeat(16));
        assert_eq!(
            clamp(&"x".repeat(20), 16),
            format!("{}\u{2026}", "x".repeat(15))
        );
    }

    #[test]
    fn uniqueness_folds_case_ignores_self_and_unset() {
        let k = Pubkey::new_unique();
        let seats = [seat(0, k, "Alpha"), seat(1, k, ""), seat(2, k, "Beta")];
        assert_eq!(taken_by(&seats, "aLPHA", 2).map(|s| s.idx), Some(0));
        assert_eq!(taken_by(&seats, "Alpha", 0), None);
        assert_eq!(taken_by(&seats, "Alpha2", 2), None);
        assert_eq!(taken_by(&seats, "", 2), None);
    }

    #[test]
    fn the_form_types_only_storable_names() {
        let mut f = Form::new(NameOp::Rename);
        // A non-ASCII letter and a tab are dropped, not typed.
        for c in "D\u{e9}sk\t!".chars() {
            f.type_char(c);
        }
        assert_eq!(f.name, "Dsk!");
        for _ in 0..40 {
            f.type_char('x');
        }
        assert_eq!(f.name.len(), VAULT_NAME_LEN);
        // A rename has the name line only.
        f.move_cursor(1);
        assert_eq!(f.field, Field::Name);
    }

    #[test]
    fn the_override_form_walks_number_name_and_lock() {
        let mut f = Form::new(NameOp::Override);
        // The abuse lever locks unless told otherwise.
        assert_eq!((f.field, f.lock), (Field::Vault, true));
        f.type_char('1');
        f.type_char('a');
        f.move_cursor(1);
        f.type_char('Z');
        f.move_cursor(1);
        f.type_char(' ');
        assert_eq!((f.vault.as_str(), f.name.as_str(), f.lock), ("1a", "Z", false));
        f.toggle();
        assert!(f.lock);
        assert_eq!(f.lines().len(), 3);
        // A rename never carries a lock.
        assert!(!Form::new(NameOp::Rename).lock);
    }

    #[test]
    fn mainnet_creates_need_a_name() {
        assert!(create_refusal("", Cluster::Localnet).is_none());
        assert!(create_refusal("", Cluster::Mainnet).is_some());
        assert!(create_refusal("Desk", Cluster::Mainnet).is_none());
        assert!(create_refusal("caf\u{e9}", Cluster::Localnet).is_some());
    }

    #[test]
    fn a_rename_targets_the_leaders_vault_and_refuses_a_lock() {
        let (me, other) = (Pubkey::new_unique(), Pubkey::new_unique());
        let mut form = Form::new(NameOp::Rename);
        form.name = "Desk".into();
        let seats = [seat(0, other, ""), seat(3, me, "Old")];
        assert_eq!(target(&seats, &me, &form).unwrap().idx, 3);
        assert!(target(&seats[..1], &me, &form).is_err());
        let mut locked = seats;
        locked[1].name_locked = true;
        let err = target(&locked, &me, &form).unwrap_err().to_string();
        assert!(err.contains("locked"), "{err}");
        // Freeze sets the lock too; the refusal names the freeze.
        let mut frozen = locked;
        frozen[1].stake.winding_down = true;
        let err = target(&frozen, &me, &form).unwrap_err().to_string();
        assert!(err.contains("frozen"), "{err}");
    }

    #[test]
    fn an_override_targets_by_number_or_name_even_when_locked() {
        let admin = Pubkey::new_unique();
        let mut form = Form::new(NameOp::Override);
        let mut seats = [seat(0, Pubkey::new_unique(), "Old"), seat(1, admin, "")];
        seats[0].name_locked = true;
        assert!(target(&seats, &admin, &form).is_err());
        for typed in ["10", "#10", "old", " Old "] {
            form.vault = typed.into();
            assert_eq!(target(&seats, &admin, &form).unwrap().idx, 0, "{typed:?}");
        }
        form.vault = "#11".into();
        assert_eq!(target(&seats, &admin, &form).unwrap().idx, 1);
        for typed in ["12", "Older", "99999999999999999999999"] {
            form.vault = typed.into();
            assert!(target(&seats, &admin, &form).is_err(), "{typed:?}");
        }
    }

    #[test]
    fn a_ticket_refuses_a_taken_name_and_a_no_op() {
        let k = Pubkey::new_unique();
        let seats = [seat(0, k, "Alpha"), seat(1, k, "Old")];
        let mut form = Form::new(NameOp::Override);
        form.lock = false;
        form.vault = "11".into();
        form.name = "ALPHA".into();
        let t = |f: &Form| ticket_for(&seats, seats[1], &market(), "TGBP", &k, f);
        assert!(t(&form).unwrap_err().to_string().contains("#10"));
        form.name = "Old".into();
        assert!(t(&form).is_err(), "same name, same lock");
        // A rename to the current name is a no-op whatever the form's lock.
        let mut rename = Form::new(NameOp::Rename);
        rename.name = "Old".into();
        rename.lock = true;
        assert!(t(&rename).is_err(), "rename to the same name");
        // The same name with the lock flipped is a real write.
        form.lock = true;
        let ticket = t(&form).unwrap();
        assert!(ticket.lock && !ticket.was_locked);
        assert!(ticket
            .summary()
            .iter()
            .any(|l| l.contains("unlocked \u{2192} locked")));
        // Blanking shows the fallback the panel will draw.
        form.name.clear();
        let blank = t(&form).unwrap();
        assert!(blank.summary().iter().any(|l| l.contains("shows as #11")));
    }
}
