// cspell:word Upgradeab

//! Publishing the program: the first deploy **and** every later upgrade, on
//! the upgradeable loader (v3), signed in memory.
//!
//! The localnet "Deploy" action shells out to `solana program deploy`, which
//! takes its signers as keypair **files**. A mainnet key never exists as a
//! file (see [`crate::wallet::resolve_key`]), so this module drives the loader
//! itself: create a buffer, write the binary into it, read it back, then
//! either `DeployWithMaxDataLen` (first publish) or `Upgrade` (every one
//! after), and finally dump the deployed bytes back and compare. The buffer key
//! is generated per run and discarded; its authority is the admin, so a run
//! that dies mid-write leaves a buffer [`close_buffers`] can always reclaim.
//!
//! Which of the two a run does is decided from the chain, never chosen by the
//! operator: no program account means first publish (which needs the program
//! key), a program whose upgrade authority is the admin means upgrade, and a
//! program already holding these exact bytes means nothing to do. Re-running
//! is therefore safe at every point — it can neither burn the program id nor
//! redeploy what is already there.
//!
//! The admin is payer, buffer authority and upgrade authority in one, because
//! `init` requires the registry's genesis admin to be the upgrade authority.

use crate::ceremony::{self, GitHead};
use crate::chain::{self, describe_rpc_error};
use crate::cluster::Cluster;
use crate::job::Logger;
use anyhow::{anyhow, bail, Context, Result};
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

/// The artifact published: the default build `make program` leaves behind —
/// the ASM fast path, with `admin-teardown` on.
pub const ARTIFACT: &str = "target/deploy/dropset.so";

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

/// Loader-v3 account tags (the bincode `u32` enum discriminant).
const TAG_BUFFER: u8 = 1;
const TAG_PROGRAMDATA: u8 = 3;

/// The binary about to be published, and where it came from.
pub struct Artifact {
    pub bytes: Vec<u8>,
    /// Lower-case hex SHA-256 of `bytes`.
    pub sha256: String,
    pub head: GitHead,
}

impl Artifact {
    /// Read [`ARTIFACT`] under `repo_root` and the commit it was built at.
    pub fn load(repo_root: &Path) -> Result<Self> {
        let path = repo_root.join(ARTIFACT);
        let bytes = std::fs::read(&path)
            .with_context(|| format!("read {} — run `make program` first", path.display()))?;
        let head = ceremony::git_head(repo_root).context("read the build commit")?;
        Ok(Self {
            sha256: sha256_hex(&bytes),
            bytes,
            head,
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
            format!("program:   {DROPSET_ID}"),
            format!("authority: {} (payer + upgrade authority)", self.authority),
            format!("binary:    {ARTIFACT}, {} bytes", self.len),
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
/// by another authority, a binary larger than the deployed capacity, a first
/// publish without the matching program key, an admin that cannot afford it,
/// and on mainnet a binary built from a modified tree (its commit would not
/// describe it).
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
    let deployed = deployed(client)?;
    let len = artifact.bytes.len();
    let mode = classify(deployed.as_ref(), &artifact.bytes, authority, program_key)?;
    let rent = |size: usize| -> Result<u64> {
        client
            .get_minimum_balance_for_rent_exemption(size)
            .map_err(|e| anyhow!("rent query: {}", describe_rpc_error(&e)))
    };
    let writes = len.div_ceil(WRITE_CHUNK_BYTES);
    // One signature per write; two each for create-buffer and deploy.
    let fees = (writes as u64 + 4) * LAMPORTS_PER_SIGNATURE;
    let buffer_rent = rent(UpgradeableLoaderState::size_of_buffer(len))?;
    let (max_len, lamports) = match (mode, &deployed) {
        (Mode::First, _) => {
            let max_len = len * MAX_LEN_MULTIPLIER;
            let accounts = rent(UpgradeableLoaderState::size_of_programdata(max_len))?
                + rent(UpgradeableLoaderState::size_of_program())?;
            (max_len, buffer_rent + accounts + fees)
        }
        (_, Some(d)) => (d.code.len(), buffer_rent + fees),
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
fn classify(
    deployed: Option<&Deployed>,
    bytes: &[u8],
    authority: &Pubkey,
    program_key: Option<&Pubkey>,
) -> Result<Mode> {
    let Some(deployed) = deployed else {
        // Two ids must agree with the key: the one this client derives every
        // PDA from, and the one compiled into the binary by `declare_id!`
        // (present as its raw 32 bytes). The second is what catches a build
        // made for a different cluster's id.
        return match program_key {
            Some(key) if *key == DROPSET_ID && embeds(bytes, key) => Ok(Mode::First),
            Some(key) => bail!(
                "the program key is {key}, but the binary {} it and this client's program \
                 id is {DROPSET_ID} (the binary {} that) — a first publish must use the key \
                 the binary was built for",
                if embeds(bytes, key) {
                    "embeds"
                } else {
                    "does not embed"
                },
                if embeds(bytes, &DROPSET_ID) {
                    "embeds"
                } else {
                    "does not embed"
                },
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
/// leaves the program id in the artifact.
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
    admin: &Keypair,
    program_key: Option<&Keypair>,
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
    log.log(format!("buffer: {}", buffer.pubkey()));
    log.record(
        "publish-start",
        &[
            ("mode", &format!("{:?}", plan.mode)),
            ("program", &DROPSET_ID.to_string()),
            ("buffer", &buffer.pubkey().to_string()),
            ("sha256", &plan.sha256),
            ("bytes", &plan.len.to_string()),
            ("commit", &plan.commit),
            ("loader_v4", plan.loader_v4),
        ],
    );

    let rent = client
        .get_minimum_balance_for_rent_exemption(UpgradeableLoaderState::size_of_buffer(bytes.len()))
        .map_err(|e| anyhow!("rent query: {}", describe_rpc_error(&e)))?;
    let create = loader::create_buffer(
        &admin.pubkey(),
        &buffer.pubkey(),
        &admin.pubkey(),
        rent,
        bytes.len(),
    )?;
    chain::send_logged(
        &client,
        admin,
        &[admin, &buffer],
        &create,
        "create_buffer",
        log,
    )
    .context("create the buffer")?;

    log.log(format!("writing {} chunks…", plan.writes));
    let messages = write_messages(&buffer.pubkey(), &admin.pubkey(), bytes);
    let config = SendAndConfirmConfigV2 {
        with_spinner: false,
        resign_txs_count: Some(WRITE_RESIGN_ROUNDS),
        rpc_send_transaction_config: RpcSendTransactionConfig {
            skip_preflight: true,
            preflight_commitment: Some(CommitmentLevel::Confirmed),
            ..RpcSendTransactionConfig::default()
        },
    };
    let failed = send_and_confirm_transactions_in_parallel_blocking_v2(
        client.clone(),
        None,
        &messages,
        &vec![admin],
        config,
    )
    // Withheld rather than formatted: a transport error's text carries the
    // endpoint URL (see `describe_rpc_error`).
    .map_err(|_| anyhow!("writing the buffer failed — close it with \"Close stranded buffers\""))?;
    if let Some(err) = failed.into_iter().flatten().next() {
        bail!("a buffer write failed ({err}) — close it with \"Close stranded buffers\"");
    }

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
                &DROPSET_ID,
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
    let live = deployed(&client)?.context("the program is not readable after deploying")?;
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
            ("program", &DROPSET_ID.to_string()),
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

/// An RPC client sized for publishing: reading `ProgramData` returns the whole
/// binary, far beyond the panel's two-second poll budget.
pub fn client(rpc_url: &str) -> RpcClient {
    RpcClient::new_with_timeout_and_commitment(
        rpc_url.to_string(),
        PUBLISH_RPC_TIMEOUT,
        CommitmentConfig::confirmed(),
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

/// Read the program's `ProgramData`, `None` when the program does not exist.
fn deployed(client: &RpcClient) -> Result<Option<Deployed>> {
    if chain::fetch_fresh(client, &DROPSET_ID, "program")?.is_none() {
        return Ok(None);
    }
    let account = chain::fetch_fresh(client, &chain::program_data(), "program data")?
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
pub fn close_buffers(client: &RpcClient, admin: &Keypair, log: &Logger) -> Result<String> {
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

    #[test]
    fn the_chain_decides_first_upgrade_or_nothing() {
        let admin = Pubkey::new_unique();
        let bin = [1u8, 2, 3];
        // Nothing deployed: only the build's own program key may create it,
        // and only from a binary that embeds that id.
        let mut elf = vec![7u8; 8];
        elf.extend_from_slice(DROPSET_ID.as_ref());
        assert_eq!(
            classify(None, &elf, &admin, Some(&DROPSET_ID)).unwrap(),
            Mode::First
        );
        let other_build = format!(
            "{:#}",
            classify(None, &bin, &admin, Some(&DROPSET_ID)).unwrap_err()
        );
        assert!(other_build.contains("does not embed"), "{other_build}");
        assert!(classify(None, &elf, &admin, Some(&Pubkey::new_unique())).is_err());
        assert!(classify(None, &elf, &admin, None).is_err());

        let at = |code: &[u8]| Deployed {
            authority: Some(admin),
            code: code.to_vec(),
        };
        // The same bytes, zero-padded, are already there.
        assert_eq!(
            classify(Some(&at(&[1, 2, 3, 0, 0])), &bin, &admin, None).unwrap(),
            Mode::Current
        );
        // Different bytes that fit are an upgrade, with no program key needed.
        assert_eq!(
            classify(Some(&at(&[9, 9, 9, 9])), &bin, &admin, None).unwrap(),
            Mode::Upgrade
        );
        // A binary larger than the capacity is refused, not truncated.
        assert!(classify(Some(&at(&[9, 9])), &bin, &admin, None).is_err());
        // Someone else's program, or an immutable one, is never touched.
        assert!(classify(Some(&at(&[9, 9, 9])), &bin, &Pubkey::new_unique(), None).is_err());
        let frozen = Deployed {
            authority: None,
            code: vec![9, 9, 9],
        };
        assert!(classify(Some(&frozen), &bin, &admin, None).is_err());
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
