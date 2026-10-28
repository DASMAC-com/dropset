// cspell:word Upgradeab

//! Publishing the program: the first deploy **and** every later upgrade, on
//! the upgradeable loader (v3), signed through each role's [`Signer`].
//!
//! The localnet "Deploy" action shells out to `solana program deploy`, which
//! takes its signers as keypair **files**. A mainnet key never exists as a
//! file — the SSH agent signs for it (see [`crate::wallet::resolve_key`]) — so
//! this module drives the loader itself: create a buffer, write the binary into it, read it back, then
//! either `DeployWithMaxDataLen` (first publish) or `Upgrade` (every one
//! after), and finally dump the deployed bytes back and compare. The buffer key
//! is generated per run and discarded.
//!
//! The write phase signs with a **writer**: a second in-memory keypair, made
//! per run, that the admin funds with one transfer. The writer creates the
//! buffer and pays for and authorizes every chunk write, then hands the buffer
//! to the admin and sweeps its leftover lamports back in one transaction — so
//! the admin, whose key may prompt per signature, signs twice per publish (the
//! funding and the deploy or upgrade) however many chunks the binary takes.
//!
//! The writer's secret exists only in this process, so the handoff is what
//! makes a failed run recoverable. After a failed funding or write phase the
//! run first waits out every transaction it may still have in flight (see
//! `outlast_in_flight`), then retries the handoff (`HANDOFF_ATTEMPTS`),
//! re-reading the writer's balance and the buffer's authority each time, so a
//! buffer handed over is one [`close_buffers`] can reclaim. A handoff that
//! never lands, or a run killed before it, abandons the buffer's rent and the
//! writer's lamports for good: nobody holds the writer's key. Both addresses
//! are in the ceremony log (`writer` at `publish-start`, and a
//! `buffer-handoff-failed` line when the retries run out).
//!
//! Which of the two a run does is decided from the chain, never chosen by the
//! operator: no program account means first publish (which needs the program
//! key), a program whose upgrade authority is the admin means upgrade, and a
//! program already holding these exact bytes means nothing to do. Re-running
//! is therefore safe at every point — it can neither burn the program id nor
//! redeploy what is already there.
//!
//! The admin is the deploy's payer and the upgrade authority, because `init`
//! requires the registry's genesis admin to be the upgrade authority.

use crate::ceremony::{self, GitHead};
use crate::chain::{self, describe_rpc_error};
use crate::cluster::Cluster;
use crate::job::Logger;
use crate::keys::RoleKey;
use anyhow::{anyhow, bail, Context, Result};
use dropset_sdk::cluster::embeds_program_id;
use dropset_sdk::DROPSET_ID;
use solana_client::rpc_client::RpcClient;
use solana_client::rpc_config::{
    RpcAccountInfoConfig, RpcProgramAccountsConfig, RpcSendTransactionConfig, UiAccountEncoding,
    UiDataSliceConfig,
};
use solana_client::rpc_filter::{Memcmp, RpcFilterType};
use solana_client::send_and_confirm_transactions_in_parallel::{
    send_and_confirm_transactions_in_parallel_blocking_v2, SendAndConfirmConfigV2,
};
use solana_commitment_config::{CommitmentConfig, CommitmentLevel};
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_loader_v3_interface::{instruction as loader, state::UpgradeableLoaderState};
use solana_message::Message;
use solana_native_token::LAMPORTS_PER_SOL;
use solana_pubkey::{pubkey, Pubkey};
use solana_signer::Signer;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// The upgradeable BPF loader (v3).
pub const LOADER_V3_ID: Pubkey = pubkey!("BPFLoaderUpgradeab1e11111111111111111111111");

/// The `enable_loader_v4` feature gate (agave-feature-set). The loader probe
/// reads it on every publish: v3 is the ratified deploy target while v4 is
/// inactive, and an activation is the signal that v3 deploys are on notice.
pub const LOADER_V4_FEATURE: Pubkey = pubkey!("2aQJYqER2aKyb3cZw22v4SL2xMX7vwXBRWfvS4pTrtED");

/// The localnet artifact: the default build `make program` leaves behind —
/// the ASM fast path, with `admin-teardown` on, embedding the localnet id.
pub const ARTIFACT: &str = "target/deploy/dropset.so";

/// The mainnet artifact `make program-mainnet` leaves behind: the same build
/// with the `mainnet` feature, so it embeds the mainnet id — `admin-teardown`
/// still on, as every early-mainnet build. A distinct file, so no localnet
/// consumer can pick it up as [`ARTIFACT`].
pub const MAINNET_ARTIFACT: &str = "target/deploy/dropset_mainnet.so";

/// The artifact `cluster` publishes, and the target that builds it.
pub fn artifact_for(cluster: Cluster) -> (&'static str, &'static str) {
    match cluster {
        Cluster::Localnet => (ARTIFACT, "make program"),
        Cluster::Mainnet => (MAINNET_ARTIFACT, "make program-mainnet"),
    }
}

/// Program bytes per `Write` transaction. A one-signer write carries 220 bytes
/// of envelope, so this leaves headroom under the 1,232-byte packet; a test
/// pins it against the real serialized size.
const WRITE_CHUNK_BYTES: usize = 1_000;

/// The first publish reserves this multiple of the binary's size in its
/// `ProgramData`, so later upgrades that grow the logic fit without an extend.
/// Data structures are locked at publish; logic is expected to change.
const MAX_LEN_MULTIPLIER: usize = 2;

/// Request timeout for [`client`].
const PUBLISH_RPC_TIMEOUT: Duration = Duration::from_secs(60);

/// Base fee per signature.
const LAMPORTS_PER_SIGNATURE: u64 = 5_000;

/// Re-sign rounds the parallel writer gets for writes whose blockhash expired.
const WRITE_RESIGN_ROUNDS: usize = 5;

/// How many times the writer's handoff is sent before the run gives up on it.
const HANDOFF_ATTEMPTS: usize = 5;
/// Pause between handoff attempts.
const HANDOFF_BACKOFF: Duration = Duration::from_secs(2);

/// Blocks a blockhash stays valid for (the runtime's `MAX_PROCESSING_AGE`), plus
/// a margin. A transaction signed against any blockhash this run has fetched
/// can no longer land once the chain is this many blocks past the height read
/// when the wait began.
const BLOCKHASH_VALIDITY_BLOCKS: u64 = 150 + 10;
/// Poll interval while [`outlast_in_flight`] waits.
const IN_FLIGHT_POLL: Duration = Duration::from_secs(2);
/// Wall-clock cap on [`outlast_in_flight`], for a node whose block height
/// cannot be read; the handoff then proceeds on whatever it reads.
const IN_FLIGHT_MAX_WAIT: Duration = Duration::from_secs(300);

/// Loader-v3 account tags (the bincode `u32` enum discriminant).
const TAG_BUFFER: u8 = 1;
const TAG_PROGRAMDATA: u8 = 3;

/// The binary about to be published, and where it came from.
pub struct Artifact {
    /// The repo-relative path it was read from ([`artifact_for`]).
    pub path: &'static str,
    pub bytes: Vec<u8>,
    /// Lower-case hex SHA-256 of `bytes`.
    pub sha256: String,
    /// The checkout's `HEAD` when the artifact was loaded — the commit the
    /// record names. The mainnet plan refuses unless the tree is clean and
    /// the artifact was written after that commit, so the name is earned.
    pub head: GitHead,
    /// The artifact's mtime, unix seconds.
    pub built_at: u64,
}

impl Artifact {
    /// Read `cluster`'s artifact ([`artifact_for`]) under `repo_root`, its
    /// mtime, and the checkout's `HEAD`.
    pub fn load(repo_root: &Path, cluster: Cluster) -> Result<Self> {
        let (relative, build) = artifact_for(cluster);
        let path = repo_root.join(relative);
        let bytes = std::fs::read(&path)
            .with_context(|| format!("read {} — run `{build}` first", path.display()))?;
        let built_at = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());
        let head = ceremony::git_head(repo_root).context("read the checkout's commit")?;
        Ok(Self {
            path: relative,
            sha256: sha256_hex(&bytes),
            bytes,
            head,
            built_at,
        })
    }
}

/// What a publish run will do, decided from the chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// No program yet: create it at the program key's address.
    First,
    /// The admin's program, holding different bytes: replace them.
    Upgrade,
    /// The program already holds exactly these bytes.
    Current,
}

/// The deployed program as the chain holds it.
struct Deployed {
    authority: Option<Pubkey>,
    /// Everything after the `ProgramData` header: the binary, zero-padded to
    /// the account's capacity.
    code: Vec<u8>,
}

/// A publish run, sized and checked — what the confirm prompt shows.
pub struct Plan {
    pub mode: Mode,
    /// The cluster's program id — what the run creates or upgrades.
    pub program: Pubkey,
    /// The artifact's repo-relative path.
    pub artifact: &'static str,
    pub sha256: String,
    pub commit: String,
    pub len: usize,
    /// `ProgramData` capacity after the run.
    pub max_len: usize,
    pub writes: usize,
    /// Upper bound on lamports the admin spends (the buffer's rent comes back).
    pub lamports: u64,
    pub balance: u64,
    pub authority: Pubkey,
    /// The loader probe's reading (see [`loader_v4_status`]).
    pub loader_v4: &'static str,
}

impl Plan {
    /// The plan, one line per fact, for the confirm prompt.
    pub fn lines(&self) -> Vec<String> {
        let verb = match self.mode {
            Mode::First => "FIRST PUBLISH — creates the program account",
            Mode::Upgrade => "UPGRADE — replaces the deployed logic in place",
            Mode::Current => "nothing to do — already deployed",
        };
        vec![
            verb.to_string(),
            format!("program:   {}", self.program),
            format!("authority: {} (payer + upgrade authority)", self.authority),
            format!("binary:    {}, {} bytes", self.artifact, self.len),
            format!("sha256:    {}", self.sha256),
            format!("commit:    {}", self.commit),
            format!(
                "capacity:  {} bytes ({} write transactions)",
                self.max_len, self.writes
            ),
            format!(
                "cost:      up to {} (balance {})",
                sol(self.lamports),
                sol(self.balance)
            ),
            format!("loader:    v3 (enable_loader_v4: {})", self.loader_v4),
        ]
    }
}

/// The loader probe: whether the `enable_loader_v4` feature is unscheduled,
/// pending, or active. A feature account holds bincode `Option<u64>` — the
/// activation slot once activated.
pub fn loader_v4_status(client: &RpcClient) -> Result<&'static str> {
    Ok(
        match chain::fetch_fresh(client, &LOADER_V4_FEATURE, "loader-v4 feature")? {
            None => "not scheduled",
            Some(account) => feature_status(&account.data),
        },
    )
}

/// [`loader_v4_status`]'s decode, separated so it is testable without a chain.
fn feature_status(data: &[u8]) -> &'static str {
    match data.first() {
        Some(1) => "ACTIVE — v3 deploys are on notice",
        _ => "pending activation",
    }
}

/// Size and check a publish of `artifact` by `authority` against the chain,
/// sending nothing.
///
/// Refuses — before anything is signed — a program that is immutable or owned
/// by another authority, a binary larger than the deployed capacity, a binary
/// built for another cluster's program id, a first publish whose program key
/// is not `cluster`'s program id, an admin that cannot afford it, and on
/// mainnet a binary built from a modified tree (its commit would not describe
/// it).
pub fn plan(
    client: &RpcClient,
    artifact: &Artifact,
    cluster: Cluster,
    authority: &Pubkey,
    program_key: Option<&Pubkey>,
) -> Result<Plan> {
    if cluster.is_mainnet() && artifact.head.dirty {
        bail!(
            "the working tree has uncommitted changes — commit them and rebuild, so the \
             recorded commit describes the binary"
        );
    }
    if cluster.is_mainnet() && artifact.built_at < artifact.head.time {
        bail!(
            "{} predates the checked-out commit {} — run `{}` so the recorded commit \
             describes the binary",
            artifact.path,
            artifact.head.commit,
            artifact_for(cluster).1
        );
    }
    let program = cluster.program_id();
    let deployed = deployed(client, cluster)?;
    let len = artifact.bytes.len();
    let mode = classify(
        deployed.as_ref(),
        &artifact.bytes,
        &program,
        authority,
        program_key,
    )?;
    let rent = |size: usize| -> Result<u64> {
        client
            .get_minimum_balance_for_rent_exemption(size)
            .map_err(|e| anyhow!("rent query: {}", describe_rpc_error(&e)))
    };
    let writes = len.div_ceil(WRITE_CHUNK_BYTES);
    let buffer_rent = rent(UpgradeableLoaderState::size_of_buffer(len))?;
    let floor = rent(0)?;
    let (max_len, lamports) = match (mode, &deployed) {
        (Mode::First, _) => {
            let max_len = len * MAX_LEN_MULTIPLIER;
            let accounts = rent(UpgradeableLoaderState::size_of_programdata(max_len))?
                + rent(UpgradeableLoaderState::size_of_program())?;
            (
                max_len,
                publish_cost(buffer_rent, floor, writes, Some(accounts)),
            )
        }
        (_, Some(d)) => (d.code.len(), publish_cost(buffer_rent, floor, writes, None)),
        (_, None) => unreachable!("classify returns First when nothing is deployed"),
    };
    let balance = client
        .get_balance(authority)
        .map_err(|e| anyhow!("balance query: {}", describe_rpc_error(&e)))?;
    if mode != Mode::Current && balance < lamports {
        bail!(
            "the admin {authority} holds {} but the publish needs up to {}",
            sol(balance),
            sol(lamports)
        );
    }
    Ok(Plan {
        mode,
        program,
        artifact: artifact.path,
        sha256: artifact.sha256.clone(),
        commit: artifact.head.commit.clone(),
        len,
        max_len,
        writes,
        lamports,
        balance,
        authority: *authority,
        loader_v4: loader_v4_status(client)?,
    })
}

/// The decision [`plan`] makes, separated so it is testable without a chain.
/// `program` is the cluster's program id.
fn classify(
    deployed: Option<&Deployed>,
    bytes: &[u8],
    program: &Pubkey,
    authority: &Pubkey,
    program_key: Option<&Pubkey>,
) -> Result<Mode> {
    // The id compiled into the binary by `declare_id!` must be the cluster's,
    // on a first publish and an upgrade alike: a localnet build upgraded into
    // the mainnet program would reject every account it owns.
    if !embeds_program_id(bytes, program) {
        let says = |id: &Pubkey| {
            if embeds(bytes, id) {
                "embeds"
            } else {
                "does not embed"
            }
        };
        bail!(
            "the binary was not built for this cluster's program id {program} (it {} it, and \
             {} the localnet id {DROPSET_ID}) — rebuild the cluster's artifact",
            says(program),
            says(&DROPSET_ID),
        );
    }
    let Some(deployed) = deployed else {
        // The program key must be the cluster's id — the one this client
        // derives every PDA from, and the one the binary embeds. On mainnet
        // that is the 1Password-held key, checked here before anything signs.
        return match program_key {
            Some(key) if key == program => Ok(Mode::First),
            Some(key) => bail!(
                "the program key is {key}, but this cluster's program id is {program} — a \
                 first publish must use the cluster's program key"
            ),
            None => bail!("the program is not deployed — a first publish needs --program-key"),
        };
    };
    match deployed.authority {
        None => bail!("the deployed program is immutable — it has no upgrade authority"),
        Some(a) if a != *authority => {
            bail!("the program's upgrade authority is {a}, not this admin ({authority})")
        }
        Some(_) => {}
    }
    if holds(&deployed.code, bytes) {
        return Ok(Mode::Current);
    }
    if bytes.len() > deployed.code.len() {
        bail!(
            "the binary is {} bytes but the deployed capacity is {} — extend the program first",
            bytes.len(),
            deployed.code.len()
        );
    }
    Ok(Mode::Upgrade)
}

/// Whether the binary `bytes` contain `id`'s 32 raw bytes — how `declare_id!`
/// leaves the program id in the artifact. Only words the refusal; the check
/// itself is the SDK's `embeds_program_id`, which also rejects a binary still
/// carrying the localnet id.
fn embeds(bytes: &[u8], id: &Pubkey) -> bool {
    let needle = id.as_ref();
    bytes.windows(needle.len()).any(|w| w == needle)
}

/// Whether the deployed `code` is exactly `bytes`, zero-padded.
fn holds(code: &[u8], bytes: &[u8]) -> bool {
    code.len() >= bytes.len()
        && code[..bytes.len()] == *bytes
        && code[bytes.len()..].iter().all(|&b| b == 0)
}

/// Publish `artifact` to `rpc_url` as `admin` — a first publish (signed by
/// `program_key`) or an upgrade, whichever the chain calls for — and verify the
/// deployed bytes against it.
pub fn publish(
    rpc_url: &str,
    artifact: &Artifact,
    cluster: Cluster,
    admin: &dyn Signer,
    program_key: Option<&RoleKey>,
    log: &Logger,
) -> Result<String> {
    let client = Arc::new(client(rpc_url));
    let plan = plan(
        &client,
        artifact,
        cluster,
        &admin.pubkey(),
        program_key.map(|k| k.pubkey()).as_ref(),
    )?;
    if plan.mode == Mode::Current {
        bail!(
            "refused — the program already holds sha256 {}; nothing was sent",
            plan.sha256
        );
    }
    let bytes = &artifact.bytes;
    let buffer = Keypair::new();
    let writer = Keypair::new();
    log.log(format!(
        "buffer: {} (writer {})",
        buffer.pubkey(),
        writer.pubkey()
    ));
    log.record(
        "publish-start",
        &[
            ("mode", &format!("{:?}", plan.mode)),
            ("program", &plan.program.to_string()),
            ("buffer", &buffer.pubkey().to_string()),
            ("writer", &writer.pubkey().to_string()),
            ("sha256", &plan.sha256),
            ("bytes", &plan.len.to_string()),
            ("commit", &plan.commit),
            ("loader_v4", plan.loader_v4),
        ],
    );

    let rent = client
        .get_minimum_balance_for_rent_exemption(UpgradeableLoaderState::size_of_buffer(bytes.len()))
        .map_err(|e| anyhow!("rent query: {}", describe_rpc_error(&e)))?;
    let floor = client
        .get_minimum_balance_for_rent_exemption(0)
        .map_err(|e| anyhow!("rent query: {}", describe_rpc_error(&e)))?;
    let funding = writer_funding(rent, floor, plan.writes);
    let written = chain::send_logged(
        &client,
        admin,
        &[admin],
        &[chain::system_transfer_ix(
            &admin.pubkey(),
            &writer.pubkey(),
            funding,
        )],
        "fund_writer",
        log,
    )
    .context("fund the writer")
    .and_then(|_| write_buffer(&client, &buffer, &writer, rent, bytes, plan.writes, log));
    // The handoff runs whether or not the funding and writes landed. A failed
    // send can still land until its blockhash expires, so a failure waits that
    // out first and the handoff reads settled state.
    if written.is_err() {
        outlast_in_flight(&client, log);
    }
    let handoff = hand_off(&client, &buffer.pubkey(), &writer, &admin.pubkey(), log);
    // A failed handoff is reported first: it is the one failure the stranded
    // buffer recovery cannot undo.
    handoff.map_err(|e| match &written {
        Err(first) => e.context(format!(
            "hand the buffer to the admin, after the publish failed ({first:#})"
        )),
        Ok(()) => e.context("hand the buffer to the admin"),
    })?;
    written?;

    // Read the buffer back before it becomes the program: a write that landed
    // short or out of place is caught here, while nothing has been replaced.
    let written = chain::fetch_fresh(&client, &buffer.pubkey(), "buffer")?
        .context("the buffer vanished after writing")?;
    if written
        .data
        .get(UpgradeableLoaderState::size_of_buffer_metadata()..)
        != Some(&bytes[..])
    {
        bail!("the buffer does not hold the binary — not deploying; close it and re-run");
    }
    log.log("buffer verified against the binary");

    let label = match plan.mode {
        Mode::First => {
            let program = program_key.context("a first publish needs the program key")?;
            let program_rent = client
                .get_minimum_balance_for_rent_exemption(UpgradeableLoaderState::size_of_program())
                .map_err(|e| anyhow!("rent query: {}", describe_rpc_error(&e)))?;
            let ixs = loader::deploy_with_max_program_len(
                &admin.pubkey(),
                &program.pubkey(),
                &buffer.pubkey(),
                &admin.pubkey(),
                program_rent,
                plan.max_len,
            )?;
            chain::send_logged(&client, admin, &[admin, program], &ixs, "deploy", log)
                .context("deploy")?;
            "published"
        }
        Mode::Upgrade => {
            let ix = loader::upgrade(
                &plan.program,
                &buffer.pubkey(),
                &admin.pubkey(),
                &admin.pubkey(),
            );
            chain::send_logged(&client, admin, &[admin], &[ix], "upgrade", log)
                .context("upgrade")?;
            "upgraded"
        }
        Mode::Current => unreachable!("refused above"),
    };

    // Dump the program back and compare: the record states what the chain
    // holds, not what was sent.
    let live =
        deployed(&client, cluster)?.context("the program is not readable after deploying")?;
    if !holds(&live.code, bytes) {
        bail!("the deployed bytes do not match the binary — investigate before anything else");
    }
    if live.authority != Some(admin.pubkey()) {
        bail!(
            "the deployed upgrade authority is {:?}, not the admin",
            live.authority
        );
    }
    let deployed_sha = sha256_hex(&live.code[..bytes.len()]);
    log.record(
        "publish-verified",
        &[
            ("program", &plan.program.to_string()),
            ("sha256", &deployed_sha),
            ("capacity", &live.code.len().to_string()),
            ("authority", &admin.pubkey().to_string()),
        ],
    );
    log.accounts_changed();
    Ok(format!(
        "Program {label} — on-chain sha256 {deployed_sha} matches the binary"
    ))
}

/// Upper bound on the lamports a publish needs the admin to hold: the writer's
/// whole funding ([`writer_funding`], most of which comes back), the program
/// accounts' rent on a first publish (`first_accounts`), and a fee for each of
/// the admin's own signatures — one on the funding, then two on a first
/// publish's deploy (the admin's and the program key's, both paid by the
/// admin) or one on an upgrade.
fn publish_cost(buffer_rent: u64, floor: u64, writes: usize, first_accounts: Option<u64>) -> u64 {
    let deploy_signatures = if first_accounts.is_some() { 2 } else { 1 };
    writer_funding(buffer_rent, floor, writes)
        + first_accounts.unwrap_or(0)
        + (1 + deploy_signatures) * LAMPORTS_PER_SIGNATURE
}

/// Lamports the admin funds the writer with: the buffer's rent, a fee for
/// every signature the writer makes — two on create-buffer, one per chunk, one
/// on the handoff — and `floor`, the rent-exempt minimum of an empty account.
/// A fee payer may not drop below that floor, so without it the last writes
/// would be dropped unlanded; the handoff's sweep, which closes the writer,
/// may. A re-signed write replaces an expired one that never landed, so it
/// costs no second fee. What is left comes back at the handoff.
fn writer_funding(buffer_rent: u64, floor: u64, writes: usize) -> u64 {
    buffer_rent + floor + (writes as u64 + 3) * LAMPORTS_PER_SIGNATURE
}

/// Create `buffer` and write `bytes` into it, all paid for and signed by
/// `writer`, which is the buffer's authority until [`hand_off`].
fn write_buffer(
    client: &Arc<RpcClient>,
    buffer: &Keypair,
    writer: &Keypair,
    rent: u64,
    bytes: &[u8],
    writes: usize,
    log: &Logger,
) -> Result<()> {
    let create = loader::create_buffer(
        &writer.pubkey(),
        &buffer.pubkey(),
        &writer.pubkey(),
        rent,
        bytes.len(),
    )?;
    chain::send_logged(
        client,
        writer,
        &[writer, buffer],
        &create,
        "create_buffer",
        log,
    )
    .context("create the buffer")?;

    log.log(format!("writing {writes} chunks…"));
    let messages = write_messages(&buffer.pubkey(), &writer.pubkey(), bytes);
    let config = SendAndConfirmConfigV2 {
        with_spinner: false,
        resign_txs_count: Some(WRITE_RESIGN_ROUNDS),
        rpc_send_transaction_config: RpcSendTransactionConfig {
            skip_preflight: true,
            preflight_commitment: Some(CommitmentLevel::Finalized),
            ..RpcSendTransactionConfig::default()
        },
    };
    let failed = send_and_confirm_transactions_in_parallel_blocking_v2(
        client.clone(),
        None,
        &messages,
        &vec![writer],
        config,
    )
    // Withheld rather than formatted: a transport error's text carries the
    // endpoint URL (see `describe_rpc_error`).
    .map_err(|_| anyhow!("writing the buffer failed — close it with \"Close stranded buffers\""))?;
    if let Some(err) = failed.into_iter().flatten().next() {
        bail!("a buffer write failed ({err}) — close it with \"Close stranded buffers\"");
    }
    Ok(())
}

/// Block until no transaction this run signed can still land: wait for the
/// chain to pass [`BLOCKHASH_VALIDITY_BLOCKS`] beyond its current height, so
/// every blockhash fetched before the wait has expired. Called only after a
/// failed funding or write phase, whose sends may have landed despite the
/// error. Never fails: an unreadable height is retried until
/// [`IN_FLIGHT_MAX_WAIT`], and the handoff then proceeds regardless, its own
/// re-reads being the last line.
fn outlast_in_flight(client: &RpcClient, log: &Logger) {
    let started = std::time::Instant::now();
    let mut until = None;
    log.log("waiting for in-flight transactions to expire before the handoff…");
    while started.elapsed() < IN_FLIGHT_MAX_WAIT {
        if let Ok(height) = client.get_block_height() {
            let until = *until.get_or_insert(height + BLOCKHASH_VALIDITY_BLOCKS);
            if height > until {
                return;
            }
        }
        std::thread::sleep(IN_FLIGHT_POLL);
    }
    log.log("in-flight wait timed out — handing off on the current state");
}

/// Hand `buffer` from `writer` to `admin` and sweep the writer's lamports back
/// to `admin`, in one transaction the writer pays for and signs.
///
/// Retried up to [`HANDOFF_ATTEMPTS`] times, each attempt re-reading the
/// writer's balance and the buffer's authority, so an attempt whose send
/// errored but landed is seen as done rather than re-sent. Done means the
/// buffer is absent or already the admin's and the writer holds nothing. When
/// every attempt fails, a `buffer-handoff-failed` record names both
/// addresses.
fn hand_off(
    client: &RpcClient,
    buffer: &Pubkey,
    writer: &Keypair,
    admin: &Pubkey,
    log: &Logger,
) -> Result<String> {
    let mut last = None;
    for attempt in 1..=HANDOFF_ATTEMPTS {
        if attempt > 1 {
            std::thread::sleep(HANDOFF_BACKOFF);
        }
        let state = client
            .get_balance(&writer.pubkey())
            .map_err(|e| anyhow!("writer balance: {}", describe_rpc_error(&e)))
            .and_then(|balance| {
                let account = chain::fetch_fresh(client, buffer, "buffer")?;
                let holds =
                    account.is_some_and(|a| buffer_authority(&a.data) == Some(writer.pubkey()));
                Ok((balance, holds))
            });
        let (balance, holds) = match state {
            Ok(state) => state,
            Err(e) => {
                last = Some(e);
                continue;
            }
        };
        let ixs = handoff_ixs(buffer, &writer.pubkey(), admin, balance, holds);
        if ixs.is_empty() {
            return Ok(String::new());
        }
        match chain::send_logged(client, writer, &[writer], &ixs, "hand_off", log) {
            Ok(sig) => {
                log.record(
                    "buffer-handoff",
                    &[
                        ("buffer", &buffer.to_string()),
                        ("authority", &admin.to_string()),
                        (
                            "swept",
                            &balance.saturating_sub(LAMPORTS_PER_SIGNATURE).to_string(),
                        ),
                    ],
                );
                return Ok(sig);
            }
            Err(e) => last = Some(e),
        }
    }
    log.record(
        "buffer-handoff-failed",
        &[
            ("buffer", &buffer.to_string()),
            ("writer", &writer.pubkey().to_string()),
        ],
    );
    Err(last
        .unwrap_or_else(|| anyhow!("no handoff attempt ran"))
        .context(format!(
            "the handoff failed {HANDOFF_ATTEMPTS} times — buffer {buffer} and its rent stay \
             with the writer {}, whose key this run held only in memory",
            writer.pubkey()
        )))
}

/// The authority a loader-v3 `Buffer` account's data names: the bincode `u32`
/// tag, then an `Option<Pubkey>`. `None` for any other account or an
/// authority-less (immutable) buffer.
fn buffer_authority(data: &[u8]) -> Option<Pubkey> {
    if data.len() < 37 || data[..4] != [TAG_BUFFER, 0, 0, 0] || data[4] != 1 {
        return None;
    }
    Pubkey::try_from(&data[5..37]).ok()
}

/// [`hand_off`]'s instructions: the authority change while the writer still
/// holds the buffer, then a transfer of everything but the transaction's own
/// fee. Empty when there is nothing to hand over and too little to sweep.
fn handoff_ixs(
    buffer: &Pubkey,
    writer: &Pubkey,
    admin: &Pubkey,
    balance: u64,
    writer_holds_buffer: bool,
) -> Vec<Instruction> {
    let mut ixs = Vec::new();
    if writer_holds_buffer {
        ixs.push(loader::set_buffer_authority(buffer, writer, admin));
    }
    let sweep = balance.saturating_sub(LAMPORTS_PER_SIGNATURE);
    if sweep > 0 {
        ixs.push(chain::system_transfer_ix(writer, admin, sweep));
    }
    ixs
}

/// An RPC client sized for publishing: reading `ProgramData` returns the whole
/// binary, far beyond the panel's two-second poll budget.
pub fn client(rpc_url: &str) -> RpcClient {
    RpcClient::new_with_timeout_and_commitment(
        rpc_url.to_string(),
        PUBLISH_RPC_TIMEOUT,
        CommitmentConfig::finalized(),
    )
}

/// One `Write` message per chunk of `bytes`.
fn write_messages(buffer: &Pubkey, authority: &Pubkey, bytes: &[u8]) -> Vec<Message> {
    bytes
        .chunks(WRITE_CHUNK_BYTES)
        .enumerate()
        .map(|(i, chunk)| {
            let offset = u32::try_from(i * WRITE_CHUNK_BYTES).expect("a program fits in u32");
            let ix = loader::write(buffer, authority, offset, chunk.to_vec());
            Message::new(&[ix], Some(authority))
        })
        .collect()
}

/// Read `cluster`'s program's `ProgramData`, `None` when the program does not
/// exist.
fn deployed(client: &RpcClient, cluster: Cluster) -> Result<Option<Deployed>> {
    if chain::fetch_fresh(client, &cluster.program_id(), "program")?.is_none() {
        return Ok(None);
    }
    let account = chain::fetch_fresh(client, &chain::program_data(cluster), "program data")?
        .context("the program exists but its ProgramData does not")?;
    parse_programdata(&account.data).map(Some)
}

/// Decode a `ProgramData` account: tag, slot, `Option<authority>`, then code.
fn parse_programdata(data: &[u8]) -> Result<Deployed> {
    let header = UpgradeableLoaderState::size_of_programdata_metadata();
    if data.len() < header || data[0] != TAG_PROGRAMDATA {
        bail!("not a loader-v3 ProgramData account");
    }
    // tag u32 (4) + slot u64 (8), then the option byte and its pubkey.
    let authority = match data[12] {
        0 => None,
        _ => Some(Pubkey::try_from(&data[13..45]).expect("32-byte slice")),
    };
    Ok(Deployed {
        authority,
        code: data[header..].to_vec(),
    })
}

/// Every loader-v3 buffer whose authority is `authority`, with its lamports —
/// the leftovers of a publish that stopped between create and deploy. The
/// data slice is empty: only the address and balance are needed.
pub fn stranded_buffers(client: &RpcClient, authority: &Pubkey) -> Result<Vec<(Pubkey, u64)>> {
    let config = RpcProgramAccountsConfig {
        filters: Some(buffer_filters(authority)),
        account_config: RpcAccountInfoConfig {
            encoding: Some(UiAccountEncoding::Base64),
            data_slice: Some(UiDataSliceConfig {
                offset: 0,
                length: 0,
            }),
            ..RpcAccountInfoConfig::default()
        },
        ..RpcProgramAccountsConfig::default()
    };
    let accounts = client
        .get_program_ui_accounts_with_config(&LOADER_V3_ID, config)
        .map_err(|e| anyhow!("list buffers: {}", describe_rpc_error(&e)))?;
    Ok(accounts.into_iter().map(|(k, a)| (k, a.lamports)).collect())
}

/// Match a `Buffer` (tag 1) whose `Some(authority)` is `authority`.
fn buffer_filters(authority: &Pubkey) -> Vec<RpcFilterType> {
    vec![
        RpcFilterType::Memcmp(Memcmp::new_base58_encoded(0, &[TAG_BUFFER, 0, 0, 0])),
        RpcFilterType::Memcmp(Memcmp::new_base58_encoded(4, &[1])),
        RpcFilterType::Memcmp(Memcmp::new_base58_encoded(5, authority.as_ref())),
    ]
}

/// Close every buffer [`stranded_buffers`] finds for `admin`, returning the
/// rent to it.
pub fn close_buffers(client: &RpcClient, admin: &dyn Signer, log: &Logger) -> Result<String> {
    let buffers = stranded_buffers(client, &admin.pubkey())?;
    if buffers.is_empty() {
        bail!(
            "refused — no stranded buffers for {}; nothing was sent",
            admin.pubkey()
        );
    }
    let mut reclaimed = 0u64;
    for (buffer, lamports) in &buffers {
        let ix = loader::close(buffer, &admin.pubkey(), &admin.pubkey());
        chain::send_logged(client, admin, &[admin], &[ix], "close_buffer", log)
            .with_context(|| format!("close buffer {buffer}"))?;
        log.record("close-buffer", &[("buffer", &buffer.to_string())]);
        reclaimed += lamports;
    }
    log.accounts_changed();
    Ok(format!(
        "Closed {} buffer(s), reclaimed {}",
        buffers.len(),
        sol(reclaimed)
    ))
}

/// Lower-case hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `lamports` as SOL, for the plan and summaries.
pub fn sol(lamports: u64) -> String {
    format!("{:.4} SOL", lamports as f64 / LAMPORTS_PER_SOL as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dropset_sdk::cluster::DROPSET_MAINNET_ID;

    #[test]
    fn the_writer_is_funded_for_every_signature_and_its_floor() {
        // Two on create-buffer, one per chunk, one on the handoff.
        let fees = (10 + 3) * LAMPORTS_PER_SIGNATURE;
        assert_eq!(writer_funding(1_000, 890_880, 10), 1_000 + 890_880 + fees);
    }

    #[test]
    fn the_planned_cost_covers_the_writer_and_every_admin_signature() {
        let writer = writer_funding(1_000, 890_880, 10);
        // Upgrade: the funding and the upgrade, one admin signature each.
        assert_eq!(
            publish_cost(1_000, 890_880, 10, None),
            writer + 2 * LAMPORTS_PER_SIGNATURE
        );
        // First publish: the funding, then the deploy's two signatures, plus
        // the program accounts' rent.
        assert_eq!(
            publish_cost(1_000, 890_880, 10, Some(7_000)),
            writer + 7_000 + 3 * LAMPORTS_PER_SIGNATURE
        );
        // Every signature of the run: 10 writes, 2 on create, 1 on the
        // handoff, 1 on the funding, 2 on the deploy.
        assert_eq!(
            publish_cost(0, 0, 10, Some(0)),
            (10 + 2 + 1 + 1 + 2) * LAMPORTS_PER_SIGNATURE
        );
    }

    #[test]
    fn a_buffer_names_its_authority_only_when_it_has_one() {
        let authority = Pubkey::new_unique();
        let mut data = vec![TAG_BUFFER, 0, 0, 0, 1];
        data.extend_from_slice(authority.as_ref());
        data.extend_from_slice(&[0; 16]);
        assert_eq!(buffer_authority(&data), Some(authority));
        // An immutable buffer, another account type, a short read.
        let mut immutable = data.clone();
        immutable[4] = 0;
        assert_eq!(buffer_authority(&immutable), None);
        let mut programdata = data.clone();
        programdata[0] = TAG_PROGRAMDATA;
        assert_eq!(buffer_authority(&programdata), None);
        assert_eq!(buffer_authority(&data[..20]), None);
    }

    #[test]
    fn the_handoff_returns_the_buffer_and_sweeps_all_but_its_fee() {
        let (buffer, writer, admin) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let ixs = handoff_ixs(&buffer, &writer, &admin, 100_000, true);
        assert_eq!(
            ixs,
            vec![
                loader::set_buffer_authority(&buffer, &writer, &admin),
                chain::system_transfer_ix(&writer, &admin, 100_000 - LAMPORTS_PER_SIGNATURE),
            ]
        );
        // A buffer the writer no longer holds (never created, or already
        // handed over) leaves only the sweep; an empty writer holding no
        // buffer leaves nothing to send.
        assert_eq!(
            handoff_ixs(&buffer, &writer, &admin, 100_000, false).len(),
            1
        );
        assert!(handoff_ixs(&buffer, &writer, &admin, LAMPORTS_PER_SIGNATURE, false).is_empty());
    }

    fn programdata(authority: Option<Pubkey>, code: &[u8]) -> Vec<u8> {
        let mut data = vec![TAG_PROGRAMDATA, 0, 0, 0];
        data.extend_from_slice(&7u64.to_le_bytes());
        match authority {
            Some(a) => {
                data.push(1);
                data.extend_from_slice(a.as_ref());
            }
            None => data.extend_from_slice(&[0; 33]),
        }
        data.extend_from_slice(code);
        data
    }

    #[test]
    fn programdata_decodes_authority_and_code() {
        let admin = Pubkey::new_unique();
        let d = parse_programdata(&programdata(Some(admin), &[1, 2, 0, 0])).unwrap();
        assert_eq!(d.authority, Some(admin));
        assert_eq!(d.code, vec![1, 2, 0, 0]);
        assert_eq!(
            parse_programdata(&programdata(None, &[9]))
                .unwrap()
                .authority,
            None
        );
        assert!(parse_programdata(&[TAG_BUFFER; 60]).is_err());
    }

    /// A stand-in binary that embeds `ids` the way `declare_id!` does.
    fn elf(ids: &[&Pubkey]) -> Vec<u8> {
        let mut bin = vec![7u8; 8];
        for id in ids {
            bin.extend_from_slice(id.as_ref());
            bin.push(1);
        }
        bin
    }

    #[test]
    fn the_chain_decides_first_upgrade_or_nothing() {
        let admin = Pubkey::new_unique();
        let p = DROPSET_ID;
        let bin = elf(&[&p]);
        // Nothing deployed: only the cluster's own program key may create it,
        // and only from a binary that embeds that id.
        assert_eq!(
            classify(None, &bin, &p, &admin, Some(&p)).unwrap(),
            Mode::First
        );
        let other_build = format!(
            "{:#}",
            classify(None, &[1, 2, 3], &p, &admin, Some(&p)).unwrap_err()
        );
        assert!(other_build.contains("does not embed"), "{other_build}");
        assert!(classify(None, &bin, &p, &admin, Some(&Pubkey::new_unique())).is_err());
        assert!(classify(None, &bin, &p, &admin, None).is_err());

        let at = |code: &[u8]| Deployed {
            authority: Some(admin),
            code: code.to_vec(),
        };
        let padded = [&bin[..], &[0, 0]].concat();
        let larger = vec![9u8; bin.len() + 1];
        // The same bytes, zero-padded, are already there.
        assert_eq!(
            classify(Some(&at(&padded)), &bin, &p, &admin, None).unwrap(),
            Mode::Current
        );
        // Different bytes that fit are an upgrade, with no program key needed.
        assert_eq!(
            classify(Some(&at(&larger)), &bin, &p, &admin, None).unwrap(),
            Mode::Upgrade
        );
        // A binary larger than the capacity is refused, not truncated.
        assert!(classify(Some(&at(&[9, 9])), &bin, &p, &admin, None).is_err());
        // Someone else's program, or an immutable one, is never touched.
        let other = Pubkey::new_unique();
        assert!(classify(Some(&at(&larger)), &bin, &p, &other, None).is_err());
        let frozen = Deployed {
            authority: None,
            code: larger.clone(),
        };
        assert!(classify(Some(&frozen), &bin, &p, &admin, None).is_err());
    }

    /// Mainnet publishes the mainnet build with the 1Password-held key, and
    /// nothing else: a first publish needs a key equal to the mainnet id, and
    /// a localnet build is refused on a first publish and an upgrade alike.
    #[test]
    fn mainnet_publishes_only_the_mainnet_build_with_the_mainnet_key() {
        let admin = Pubkey::new_unique();
        let m = Cluster::Mainnet.program_id();
        assert_eq!(m, DROPSET_MAINNET_ID);
        let mainnet_build = elf(&[&m]);
        assert_eq!(
            classify(None, &mainnet_build, &m, &admin, Some(&m)).unwrap(),
            Mode::First
        );
        // The localnet key, whose secret is public, cannot publish mainnet.
        let wrong_key = format!(
            "{:#}",
            classify(None, &mainnet_build, &m, &admin, Some(&DROPSET_ID)).unwrap_err()
        );
        assert!(wrong_key.contains("program key"), "{wrong_key}");
        // A localnet build, or one carrying both ids, is never published to
        // the mainnet program.
        let deployed = Deployed {
            authority: Some(admin),
            code: vec![0; 256],
        };
        for bin in [elf(&[&DROPSET_ID]), elf(&[&m, &DROPSET_ID])] {
            assert!(classify(None, &bin, &m, &admin, Some(&m)).is_err());
            let refusal = format!(
                "{:#}",
                classify(Some(&deployed), &bin, &m, &admin, None).unwrap_err()
            );
            assert!(refusal.contains("not built for"), "{refusal}");
        }
        // The mainnet build is never published to localnet either.
        assert!(classify(None, &mainnet_build, &DROPSET_ID, &admin, Some(&DROPSET_ID)).is_err());
    }

    #[test]
    fn each_cluster_publishes_its_own_artifact() {
        assert_eq!(artifact_for(Cluster::Localnet).0, ARTIFACT);
        assert_eq!(artifact_for(Cluster::Mainnet).0, MAINNET_ARTIFACT);
        assert_ne!(ARTIFACT, MAINNET_ARTIFACT);
        assert_eq!(artifact_for(Cluster::Mainnet).1, "make program-mainnet");
        // A missing artifact names the cluster's own build target.
        let empty = tempfile::tempdir().unwrap();
        let Err(missing) = Artifact::load(empty.path(), Cluster::Mainnet) else {
            panic!("an empty tree has no artifact");
        };
        let missing = format!("{missing:#}");
        assert!(missing.contains(MAINNET_ARTIFACT), "{missing}");
        assert!(missing.contains("make program-mainnet"), "{missing}");
    }

    #[test]
    fn padding_must_be_zero_for_bytes_to_count_as_deployed() {
        assert!(holds(&[1, 2, 0], &[1, 2]));
        assert!(!holds(&[1, 2, 5], &[1, 2]));
        assert!(!holds(&[1], &[1, 2]));
    }

    #[test]
    fn a_full_write_fits_one_packet() {
        let admin = Keypair::new();
        let bytes = vec![0xAB; WRITE_CHUNK_BYTES * 2 + 1];
        let messages = write_messages(&Pubkey::new_unique(), &admin.pubkey(), &bytes);
        assert_eq!(messages.len(), 3);
        let tx = solana_transaction::Transaction::new(
            &[&admin],
            messages[0].clone(),
            solana_message::Hash::default(),
        );
        let size = bincode::serialized_size(&tx).unwrap();
        assert!(size <= 1_232, "a full write is {size} bytes");
    }

    #[test]
    fn buffer_filters_match_the_loader_buffer_layout() {
        // A buffer header is tag u32 = 1, then Some(authority).
        let authority = Pubkey::new_unique();
        let mut header = vec![TAG_BUFFER, 0, 0, 0, 1];
        header.extend_from_slice(authority.as_ref());
        assert_eq!(
            header.len(),
            UpgradeableLoaderState::size_of_buffer_metadata()
        );
        for filter in buffer_filters(&authority) {
            let RpcFilterType::Memcmp(m) = filter else {
                panic!("memcmp only");
            };
            assert!(m.bytes_match(&header), "{m:?}");
        }
    }

    #[test]
    fn the_loader_probe_reads_the_activation_option() {
        assert_eq!(feature_status(&[0]), "pending activation");
        let mut active = vec![1];
        active.extend_from_slice(&42u64.to_le_bytes());
        assert!(feature_status(&active).starts_with("ACTIVE"));
    }

    #[test]
    fn sha256_is_lower_hex() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
