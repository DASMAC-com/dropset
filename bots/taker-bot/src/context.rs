//! Runtime context — the chain handle, the taker and mint-authority
//! identities, the discovered market, and the live stochastic flow.
//!
//! State is thin: the flow carries the only cross-tick memory (its regime and
//! buy-bias), and inventory / book are re-read from chain each tick rather
//! than tracked here.

use crate::model::Flow;
use solana_client::rpc_client::RpcClient;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;

/// The discovered market and its token metadata — everything the taker needs
/// to address the swap and size orders against the right decimals.
#[derive(Clone, Debug)]
pub struct MarketAddrs {
    pub market: Pubkey,
    pub base_mint: Pubkey,
    pub quote_mint: Pubkey,
    pub base_treasury: Pubkey,
    pub quote_treasury: Pubkey,
    pub base_decimals: u8,
    pub quote_decimals: u8,
}

/// The taker-bot's runtime context.
pub struct Context {
    pub client: RpcClient,
    /// The taker — signs and pays for each swap.
    pub taker: Keypair,
    /// The mock-mint authority — mints the taker its starting inventory and
    /// refills it when low. Never signs a swap.
    pub mint_authority: Keypair,
    pub market: MarketAddrs,
    /// The live stochastic order-flow generator.
    pub flow: Flow,
}

impl Context {
    pub fn new(
        client: RpcClient,
        taker: Keypair,
        mint_authority: Keypair,
        market: MarketAddrs,
        flow: Flow,
    ) -> Self {
        Self {
            client,
            taker,
            mint_authority,
            market,
            flow,
        }
    }
}
