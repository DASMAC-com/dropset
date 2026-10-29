// cspell:word rodata

//! The ceremony end to end against a live `solana-test-validator`.
//!
//! The same functions the mainnet menu dispatches — publish, the stranded
//! buffer recovery, init, create-market, create-vault, the seed deposit — run
//! in order with the committed localnet keys, the ceremony log attached, and
//! every re-run checked to refuse. Mainnet differs only in the cluster switch
//! and the key source, both covered by unit tests elsewhere.
//!
//! Every role signs through a [`Counting`] signer, and each step asserts how
//! many signatures it took from each — on mainnet, one operator prompt apiece.
//!
//! Ignored by default: it needs `solana-test-validator` on `PATH`, the default
//! artifact built (`make program`), and port 8899 free. `make tui-ceremony-e2e`
//! builds and runs it.

use crate::action::{self, Outcome};
use crate::ceremony::CeremonyLog;
use crate::chain;
use crate::cluster::Cluster;
use crate::job::Logger;
use crate::keys::RoleKey;
use crate::market;
use crate::publish::{self, Artifact, Mode};
use crate::validator::Validator;
use crate::wallet;
use solana_keypair::Keypair;
use solana_loader_v3_interface::{instruction as loader, state::UpgradeableLoaderState};
use solana_native_token::LAMPORTS_PER_SOL;
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_signer::Signer;
use solana_signer::SignerError;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const L: Cluster = Cluster::Localnet;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

/// A signer that counts its signatures: what an agent-held key would prompt.
struct Counting {
    inner: RoleKey,
    signed: AtomicUsize,
}

impl Counting {
    fn new(inner: RoleKey) -> Arc<Self> {
        Arc::new(Self {
            inner,
            signed: AtomicUsize::new(0),
        })
    }

    /// The signatures since the last call.
    fn take(&self) -> usize {
        self.signed.swap(0, Ordering::SeqCst)
    }
}

impl Signer for Counting {
    fn try_pubkey(&self) -> Result<Pubkey, SignerError> {
        self.inner.try_pubkey()
    }

    fn try_sign_message(&self, message: &[u8]) -> Result<Signature, SignerError> {
        self.signed.fetch_add(1, Ordering::SeqCst);
        self.inner.try_sign_message(message)
    }

    fn is_interactive(&self) -> bool {
        false
    }
}

/// The signatures each of `keys` took since the last read, in order.
fn taken(keys: &[&Counting]) -> Vec<usize> {
    keys.iter().map(|k| k.take()).collect()
}

/// The file range of `elf`'s `.rodata` section, read from its section headers.
fn rodata(elf: &[u8]) -> std::ops::Range<usize> {
    let u16_at = |at: usize| usize::from(u16::from_le_bytes(elf[at..at + 2].try_into().unwrap()));
    let u64_at = |at: usize| u64::from_le_bytes(elf[at..at + 8].try_into().unwrap()) as usize;
    let u32_at = |at: usize| u32::from_le_bytes(elf[at..at + 4].try_into().unwrap()) as usize;
    // The ELF64 header's section-header offset, count and name-table index.
    let (headers_at, count, names_index) = (u64_at(0x28), u16_at(0x3c), u16_at(0x3e));
    let header = |i: usize| headers_at + 64 * i;
    let names = u64_at(header(names_index) + 24);
    (0..count)
        .map(header)
        .find(|&h| elf[names + u32_at(h)..].starts_with(b".rodata\0"))
        .map(|h| u64_at(h + 24)..u64_at(h + 24) + u64_at(h + 32))
        .expect("the artifact has a .rodata section")
}

fn wait_for(client: &solana_client::rpc_client::RpcClient) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while client.get_slot().is_err() {
        assert!(Instant::now() < deadline, "validator did not come up");
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[test]
#[ignore = "needs solana-test-validator and `make program`; run `make tui-ceremony-e2e`"]
fn the_ceremony_runs_end_to_end_on_localnet() {
    let root = repo_root();
    let validator = Validator::spawn().unwrap();
    let url = validator.rpc_url().to_string();
    let client = chain::rpc(&url);
    wait_for(&client);

    let (admin, _) = wallet::load(None, &root).unwrap();
    let admin = Counting::new(Arc::new(admin));
    let program = Counting::new(Arc::new(
        wallet::load_leader(&root.join("keys/AAAA.json").to_string_lossy()).unwrap(),
    ));
    let program_key: RoleKey = program.clone();
    chain::airdrop(&client, &admin.pubkey(), 100 * LAMPORTS_PER_SOL).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let record_path = dir.path().join("ceremony.log");
    let record = Arc::new(CeremonyLog::open(&record_path, &root, L).unwrap());
    let log = Logger::stdout().with_record(Some(record));

    // Publish: a first deploy, verified on-chain, then a refused re-run.
    let artifact = Artifact::load(&root, L).unwrap();
    assert_eq!(artifact.path, publish::ARTIFACT);
    // The localnet deployment is the committed key's: what every PDA below
    // is derived from.
    assert_eq!(program.pubkey(), L.program_id());
    let stranded = publish::Stranded::default();
    let run_publish = |artifact: &Artifact, program_key: Option<&RoleKey>| {
        publish::publish(&url, artifact, L, &*admin, program_key, &stranded, &log)
    };
    let published = run_publish(&artifact, Some(&program_key)).unwrap();
    assert!(published.contains(&artifact.sha256), "{published}");
    // The admin funds the writer and signs the deploy; the program key signs
    // the deploy alone. Every chunk write is the writer's.
    assert_eq!(taken(&[&admin, &program]), [2, 1], "first publish");
    let again = run_publish(&artifact, Some(&program_key)).unwrap_err();
    assert!(format!("{again:#}").contains("refused"), "{again:#}");
    assert_eq!(taken(&[&admin, &program]), [0, 0], "refused publish");

    // A changed binary against the live program publishes as an upgrade,
    // signed by the admin alone — no program key. One `.rodata` byte outside
    // the embedded id differs, so the ELF stays loadable; the upgrade back to
    // the built binary leaves the steps below running the real program.
    let mut changed = artifact.bytes.clone();
    let id = L.program_id();
    let id_at = changed.windows(32).position(|w| w == id.as_ref()).unwrap();
    let byte = rodata(&changed)
        .rev()
        .find(|i| !(id_at..id_at + 32).contains(i))
        .unwrap();
    changed[byte] ^= 0xFF;
    let changed = Artifact {
        path: artifact.path,
        sha256: publish::sha256_hex(&changed),
        bytes: changed,
        head: crate::ceremony::git_head(&root).unwrap(),
        built_at: artifact.built_at,
    };
    let plan = publish::plan(&publish::client(&url), &changed, L, &admin.pubkey(), None).unwrap();
    assert_eq!(plan.mode, Mode::Upgrade);
    for (binary, step) in [(&changed, "upgrade"), (&artifact, "upgrade back")] {
        let upgraded = run_publish(binary, None).unwrap();
        assert!(upgraded.contains(&binary.sha256), "{upgraded}");
        // The admin funds the writer and signs the upgrade.
        assert_eq!(taken(&[&admin, &program]), [2, 0], "{step}");
    }
    assert!(stranded.addresses().is_empty());

    // A stranded buffer is found and reclaimed, and so is one still held by
    // a writer whose handoff failed: the recovery hands it back first.
    let buffer = Keypair::new();
    let rent = client
        .get_minimum_balance_for_rent_exemption(UpgradeableLoaderState::size_of_buffer(16))
        .unwrap();
    let ixs = loader::create_buffer(&admin.pubkey(), &buffer.pubkey(), &admin.pubkey(), rent, 16)
        .unwrap();
    chain::send(&client, &*admin, &[&*admin, &buffer], &ixs).unwrap();
    admin.take();
    let (writer, held) = (Keypair::new(), Keypair::new());
    chain::airdrop(&client, &writer.pubkey(), LAMPORTS_PER_SOL).unwrap();
    let ixs = loader::create_buffer(&writer.pubkey(), &held.pubkey(), &writer.pubkey(), rent, 16)
        .unwrap();
    chain::send(&client, &writer, &[&writer, &held], &ixs).unwrap();
    let writer_address = writer.pubkey();
    stranded.keep(held.pubkey(), writer);
    assert_eq!(
        publish::stranded_buffers(&client, &admin.pubkey())
            .unwrap()
            .len(),
        1
    );
    publish::close_buffers(&client, &*admin, &stranded, &log).unwrap();
    // The handoff is the writer's to sign; the admin signs each close.
    assert_eq!(taken(&[&admin]), [2], "close two stranded buffers");
    assert!(stranded.addresses().is_empty());
    assert_eq!(client.get_balance(&writer_address).unwrap(), 0);
    assert!(publish::stranded_buffers(&client, &admin.pubkey())
        .unwrap()
        .is_empty());

    // The account steps for one roster pair, each refusing a second run.
    action::do_init(&client, &*admin, L, &log).unwrap();
    // Localnet mints a mock fee mint first; mainnet's is the roster's.
    assert_eq!(taken(&[&admin]), [2], "init");
    assert!(matches!(
        action::do_init(&client, &*admin, L, &log).unwrap(),
        Outcome::AlreadyThere(_)
    ));
    assert_eq!(taken(&[&admin]), [0], "refused init");
    let config = market::roster(L)[0];
    let leader = Counting::new(market::leader(&root, config, None).unwrap());
    // A quote authority of its own, as on mainnet: the seed ladder must be
    // signed by it, not the leader, or the deposit step fails on chain.
    let quote_authority = Counting::new(Arc::new(Keypair::new()));
    chain::airdrop(&client, &quote_authority.pubkey(), LAMPORTS_PER_SOL).unwrap();
    let keys = [&*admin, &*leader, &*quote_authority];
    // Signatures per step from the admin, the leader and the quote authority.
    // The admin's localnet extras are the mock minting mainnet never does:
    // the pair's two mints before create-market, and the leader's ATAs and
    // mint-tos before the seed deposit, which is one transaction for all three.
    let expected: [(&str, [usize; 3]); 3] = [
        ("create_market", [1 + 2, 0, 0]),
        ("create_vault", [1, 0, 0]),
        ("deposit", [1 + 5, 1, 1]),
    ];
    let steps: [&dyn Fn() -> anyhow::Result<Outcome>; 3] = [
        &|| action::do_create_market(&client, &*admin, &root, L, config, &log),
        &|| {
            action::do_create_vault(
                &client,
                &*admin,
                &root,
                L,
                config,
                &*leader,
                &quote_authority.pubkey(),
                chain::BOOTSTRAP_VAULT_NAME,
                &log,
            )
        },
        &|| {
            action::do_deposit(
                &client,
                &*admin,
                &root,
                L,
                config,
                &*leader,
                &*quote_authority,
                market::Funding::MintBoth,
                market::Posture::Active,
                &log,
            )
        },
    ];
    let mut signed = Vec::new();
    for (step, (label, _)) in steps.into_iter().zip(expected) {
        assert!(matches!(step().unwrap(), Outcome::Done(_)), "{label}");
        signed.push((label, taken(&keys)));
        assert!(
            matches!(step().unwrap(), Outcome::AlreadyThere(_)),
            "{label}"
        );
        assert_eq!(taken(&keys), [0, 0, 0], "refused {label}");
    }
    let expected: Vec<_> = expected.iter().map(|(l, n)| (*l, n.to_vec())).collect();
    assert_eq!(signed, expected);

    // Everything landed at the localnet deployment's addresses, owned by its
    // program: the registry, and the market with its seeded vault.
    let registry = crate::accounts::registry_fresh(&client, L)
        .unwrap()
        .unwrap();
    assert_eq!(registry.address, chain::registry_pda(L));
    let (base, quote) = market::pair_mints(&root, config).unwrap();
    let market_address = chain::market_pda(L, &base, &quote);
    let owner = client.get_account(&market_address).unwrap().owner;
    assert_eq!(owner, L.program_id());
    let seats = crate::accounts::vault_seats_fresh(&client, L, &market_address)
        .unwrap()
        .unwrap();
    assert!(seats
        .iter()
        .any(|s| s.leader == leader.pubkey() && s.seeded));

    // The daily re-centre: paid for and signed by the quote authority alone.
    let seat = crate::passive::passive_seat(&client, L, &market_address, &quote_authority.pubkey())
        .unwrap();
    let price = crate::accounts::read_reference_price(&client, &market_address, seat.idx).unwrap();
    let ticket = crate::passive::Ticket {
        op: crate::passive::PassiveOp::Recenter,
        market: market_address,
        base_mint: base,
        base_symbol: config.base.symbol,
        quote_symbol: config.quote.symbol,
        seat,
        quote_authority: quote_authority.pubkey(),
        levels: Vec::new(),
        center: 1.0,
        center_source: "e2e",
        price,
        warnings: Vec::new(),
    };
    crate::passive::execute(&client, &*quote_authority, L, &ticket, &log).unwrap();
    assert_eq!(taken(&keys), [0, 0, 1], "re-centre");

    // The record carries the publish, the recovery and every signature.
    let body = std::fs::read_to_string(&record_path).unwrap();
    for step in [
        "step=publish-start",
        "step=publish-verified",
        "step=close-buffer",
        "step=init",
        "step=init-verified",
        "step=create_market",
        "step=create_vault",
        "step=deposit_leader",
    ] {
        assert!(body.contains(step), "{step} missing from:\n{body}");
    }
    assert!(body.contains(&format!("sha256={}", artifact.sha256)));
    assert!(body.contains(&format!("commit={}", artifact.head.commit)));
}
