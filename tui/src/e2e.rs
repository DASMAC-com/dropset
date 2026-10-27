//! The ceremony end to end against a live `solana-test-validator`.
//!
//! The same functions the mainnet menu dispatches — publish, the stranded
//! buffer recovery, init, create-market, create-vault, the seed deposit — run
//! in order with the committed localnet keys, the ceremony log attached, and
//! every re-run checked to refuse. Mainnet differs only in the cluster switch
//! and the key source, both covered by unit tests elsewhere.
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
use solana_signer::Signer;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const L: Cluster = Cluster::Localnet;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
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
    let program: RoleKey =
        Arc::new(wallet::load_leader(&root.join("keys/AAAA.json").to_string_lossy()).unwrap());
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
    let published = publish::publish(&url, &artifact, L, &admin, Some(&program), &log).unwrap();
    assert!(published.contains(&artifact.sha256), "{published}");
    let again = publish::publish(&url, &artifact, L, &admin, Some(&program), &log).unwrap_err();
    assert!(format!("{again:#}").contains("refused"), "{again:#}");

    // A changed binary against the live program plans as an upgrade, signed
    // by the admin alone — no program key.
    let mut changed = artifact.bytes.clone();
    *changed.last_mut().unwrap() ^= 0xFF;
    let changed = Artifact {
        path: artifact.path,
        sha256: publish::sha256_hex(&changed),
        bytes: changed,
        head: crate::ceremony::git_head(&root).unwrap(),
        built_at: artifact.built_at,
    };
    let plan = publish::plan(&publish::client(&url), &changed, L, &admin.pubkey(), None).unwrap();
    assert_eq!(plan.mode, Mode::Upgrade);

    // A stranded buffer is found and reclaimed.
    let buffer = Keypair::new();
    let rent = client
        .get_minimum_balance_for_rent_exemption(UpgradeableLoaderState::size_of_buffer(16))
        .unwrap();
    let ixs = loader::create_buffer(&admin.pubkey(), &buffer.pubkey(), &admin.pubkey(), rent, 16)
        .unwrap();
    chain::send(&client, &admin, &[&admin, &buffer], &ixs).unwrap();
    assert_eq!(
        publish::stranded_buffers(&client, &admin.pubkey())
            .unwrap()
            .len(),
        1
    );
    publish::close_buffers(&client, &admin, &log).unwrap();
    assert!(publish::stranded_buffers(&client, &admin.pubkey())
        .unwrap()
        .is_empty());

    // The account steps for one roster pair, each refusing a second run.
    action::do_init(&client, &admin, L, &log).unwrap();
    assert!(matches!(
        action::do_init(&client, &admin, L, &log).unwrap(),
        Outcome::AlreadyThere(_)
    ));
    let config = market::roster(L)[0];
    let leader = market::leader(&root, config, None).unwrap();
    // A quote authority of its own, as on mainnet: the seed ladder must be
    // signed by it, not the leader, or the deposit step fails on chain.
    let quote_authority = Keypair::new();
    let steps: [&dyn Fn() -> anyhow::Result<Outcome>; 3] = [
        &|| action::do_create_market(&client, &admin, &root, L, config, &log),
        &|| {
            action::do_create_vault(
                &client,
                &admin,
                &root,
                L,
                config,
                &leader,
                &quote_authority.pubkey(),
                chain::BOOTSTRAP_VAULT_NAME,
                &log,
            )
        },
        &|| {
            action::do_deposit(
                &client,
                &admin,
                &root,
                L,
                config,
                &leader,
                &quote_authority,
                market::Funding::MintBoth,
                market::Posture::Active,
                &log,
            )
        },
    ];
    for step in steps {
        assert!(matches!(step().unwrap(), Outcome::Done(_)));
        assert!(matches!(step().unwrap(), Outcome::AlreadyThere(_)));
    }

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
