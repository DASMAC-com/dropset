//! The SSH-agent signer spike's attended harness: list what each agent
//! socket serves, then sign a localnet transfer from an agent-held key.
//!
//! ```sh
//! cargo run -p dropset-tui --example agent_transfer -- <address> \
//!   [--socket <path>] [--rpc-url <url>] [--list]
//! ```
//!
//! With `--list` it only prints every identity each candidate socket serves
//! (whether a given 1Password item is served at all). Otherwise it finds the
//! agent serving `<address>`, funds the address from the localnet faucet,
//! and sends 1,000 lamports to a fresh throwaway address, signed through
//! the agent — one approval prompt — then verifies the signature and
//! confirms the transaction. Localnet only: the default RPC is
//! `http://127.0.0.1:8899`, and the faucet refuses anywhere else.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use dropset_tui::agent_signer::{default_candidate_sockets, list_identities, AgentSigner};
use dropset_tui::chain::{airdrop, describe_rpc_error, system_transfer_ix};
use solana_client::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use solana_transaction::Transaction;

const FUND_LAMPORTS: u64 = 1_000_000_000;
const TRANSFER_LAMPORTS: u64 = 1_000;

fn main() -> Result<()> {
    let mut address = None;
    let mut socket = None;
    let mut rpc_url = "http://127.0.0.1:8899".to_string();
    let mut list_only = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--socket" => socket = Some(PathBuf::from(args.next().context("--socket <path>")?)),
            "--rpc-url" => rpc_url = args.next().context("--rpc-url <url>")?,
            "--list" => list_only = true,
            a if a.starts_with("--") => bail!("unknown flag {a}"),
            a => address = Some(a.parse::<Pubkey>().context("address is not base58")?),
        }
    }

    let candidates = match &socket {
        Some(s) => vec![s.clone()],
        None => default_candidate_sockets(),
    };
    for path in &candidates {
        println!("socket {}", path.display());
        match list_identities(path) {
            Ok(ids) => {
                for id in ids {
                    let key = id
                        .ed25519_pubkey()
                        .map_or_else(|| "(not Ed25519)".to_string(), |p| p.to_string());
                    println!("  {} {key} {:?}", id.key_type(), id.comment);
                }
            }
            Err(e) => println!("  unavailable: {e:#}"),
        }
    }
    if list_only {
        return Ok(());
    }

    let address = address.context("usage: agent_transfer <address> [--socket p] [--list]")?;
    let signer = AgentSigner::connect_among(address, &candidates)?;
    println!("signer {address} via {}", signer.socket().display());

    let client = RpcClient::new_with_commitment(rpc_url, CommitmentConfig::confirmed());
    airdrop(&client, &address, FUND_LAMPORTS).context("fund from the localnet faucet")?;
    let recipient = Keypair::new().pubkey();
    let ix = system_transfer_ix(&address, &recipient, TRANSFER_LAMPORTS);
    let mut tx = Transaction::new_with_payer(&[ix], Some(&address));
    let blockhash = client
        .get_latest_blockhash()
        .map_err(|e| anyhow::anyhow!("blockhash: {}", describe_rpc_error(&e)))?;

    println!("requesting the agent's signature — approve the prompt");
    tx.try_sign(&[&signer], blockhash)
        .context("agent signature")?;
    let sig = tx.signatures[0];
    let verified = sig.verify(address.as_ref(), &tx.message_data());
    println!("signature {sig} verifies against {address}: {verified}");
    if !verified {
        bail!("signature does not verify");
    }

    let confirmed = client
        .send_and_confirm_transaction(&tx)
        .map_err(|e| anyhow::anyhow!("send: {}", describe_rpc_error(&e)))?;
    let received = client
        .get_balance(&recipient)
        .map_err(|e| anyhow::anyhow!("balance: {}", describe_rpc_error(&e)))?;
    println!("confirmed {confirmed}; {recipient} holds {received} lamports");
    Ok(())
}
