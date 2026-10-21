// cspell:word Tokenkeg
//! The Orca Whirlpool spot adapter (docs/data-feeds.md §9) — a basis-leg
//! **tape** read straight from chain state.
//!
//! The pool is where the on-chain price of a token forms and what Jupiter
//! routes through, so its current price is both a third tape for the basis leg
//! and the number that says whether a resting ladder sits inside the route.
//! It is **never designated believable on its own**: a pool price is whatever
//! the last swap left it at, and a thin pool can be walked by anyone willing to
//! pay the slippage. It can corroborate the CEX legs; it does not stand in for
//! them.
//!
//! Each poll reads one Whirlpool account over JSON-RPC, decodes it with Orca's
//! SDK (`Whirlpool::from_bytes`), and converts its Q64.64 `sqrt_price` to a
//! decimals-adjusted quote-per-base price with the SDK's `sqrt_price_to_price`.
//! The pool address is pinned by the caller, never discovered: a pool lookup
//! by mint pair would let whoever creates the next pool for that pair choose
//! which one this reads.
//!
//! **Orientation is checked, not assumed.** A Whirlpool's token A and B are
//! ordered by mint address, not by which leg is the base, so a pool can price
//! the canonical pair either way round. The caller names both legs' mints; a
//! pool that is base/quote as A/B is read as-is, one that is quote/base is
//! inverted, and one that is not exactly that pair is refused — that is a
//! mis-pinned address, not a price. A pool with no liquidity in range yields no
//! price: its `sqrt_price` is the last one traded, which nothing can trade at.
//!
//! **Reads go through [`RpcFailover`]**, an ordered endpoint list: primary
//! first, the next one whenever this one cannot serve the read (see
//! [`ReadError::Unavailable`]), and back to
//! the primary on the very next poll, since every poll starts from the top. The
//! cost of always retrying the primary is one call budget per poll while it is
//! down — at a 15-second cadence and a 10-second budget that is a slow reading,
//! not a lost one, and it buys a return to the primary with no health state to
//! get wrong. Endpoints are logged by **label**, never by URL: a keyed provider
//! carries its API key in the URL itself.

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use orca_whirlpools_client::{Whirlpool, WHIRLPOOL_DISCRIMINATOR, WHIRLPOOL_ID};
use orca_whirlpools_core::sqrt_price_to_price;
use solana_client::client_error::{ClientError, ClientErrorKind};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;
use solana_pubkey::Pubkey;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::{Batch, Source};

/// The public mainnet-beta endpoint — the fallback every endpoint list ends
/// with, and the whole list when no keyed primary is configured.
///
/// Rate-limited per IP (100 requests per 10 s, 40 per method), which a handful
/// of pools polled every 15 s sits far inside — one `getMultipleAccounts` per
/// pool per poll, plus two mint reads once at startup.
pub const PUBLIC_MAINNET_RPC: &str = "https://api.mainnet-beta.solana.com";

/// How long one endpoint gets to answer one call before the next is tried.
///
/// This is an **outer** bound on purpose. The Solana client retries a 429
/// internally — up to five times, each after whatever `Retry-After` asks for,
/// up to two minutes apiece — so a rate-limited primary would otherwise stall a
/// poll for minutes before ever reporting the 429 this list exists to fail over
/// on. Capping the whole call turns that stall into a timeout.
pub const CALL_BUDGET: Duration = Duration::from_secs(10);

/// SPL Token and Token-2022, the two programs whose mint layout puts
/// `decimals` at [`MINT_DECIMALS_OFFSET`].
const TOKEN_PROGRAMS: [Pubkey; 2] = [
    Pubkey::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"),
    Pubkey::from_str_const("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb"),
];

/// The SPL mint's `decimals` byte: after the 36-byte optional mint authority
/// and the 8-byte supply. Token-2022 keeps the base layout and appends its
/// extensions, so the offset holds for both.
const MINT_DECIMALS_OFFSET: usize = 44;

/// The base SPL mint's length; anything shorter is not a mint.
const MINT_LEN: usize = 82;

/// The [`Source::name`] an [`OrcaSpot`] over `product_id` reports — per
/// product, on the Coinbase ticker's pattern, while the stored source label
/// stays the bare `orca`.
pub fn spot_feed_name(product_id: &str) -> String {
    format!("orca:{product_id}")
}

/// One account as the adapter needs it: who owns it, and its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountData {
    pub owner: Pubkey,
    pub data: Vec<u8>,
}

/// Why a read failed, split by whether another endpoint could do better.
///
/// A classification for [`RpcFailover`] to act on rather than a reportable
/// error: the failover turns it into an `anyhow` error at its own boundary, so
/// it carries no `Display` of its own.
#[derive(Debug)]
pub enum ReadError {
    /// This endpoint could not serve the read, though another might — a
    /// connection failure or reset, a timeout, a 429 or a 5xx, a refused
    /// credential, or a reply that was not JSON-RPC at all. The next endpoint
    /// is tried.
    Unavailable(String),
    /// The endpoint answered with a JSON-RPC error, which ends the call. For a
    /// malformed request another endpoint would say the same; a node reporting
    /// itself unhealthy (-32005) is the known exception, and is not yet failed
    /// over.
    Other(anyhow::Error),
}

/// The one RPC call this adapter makes, behind a seam so the failover order and
/// the poll path are testable without a cluster.
///
/// The same idea as the indexer transport's `RpcTransport` (`crate::rpc`), but
/// its own trait: that one lives behind the `rpc` feature, which this adapter
/// does not pull in, and models a signature walk rather than an account read.
#[async_trait]
pub trait AccountReader: Send + Sync {
    /// `getMultipleAccounts` over `keys`, in order; `None` for an account that
    /// does not exist.
    async fn accounts(&self, keys: &[Pubkey]) -> Result<Vec<Option<AccountData>>, ReadError>;
}

#[async_trait]
impl AccountReader for RpcClient {
    async fn accounts(&self, keys: &[Pubkey]) -> Result<Vec<Option<AccountData>>, ReadError> {
        let accounts = self.get_multiple_accounts(keys).await.map_err(classify)?;
        Ok(accounts
            .into_iter()
            .map(|account| {
                account.map(|a| AccountData {
                    owner: a.owner,
                    data: a.data,
                })
            })
            .collect())
    }
}

/// Sort a client error into [`ReadError`]'s two classes.
///
/// **Only an answered JSON-RPC error is formatted.** `reqwest`'s `Display`
/// names the request URL, and a keyed endpoint's URL is its credential, so a
/// transport error is described from its flags, and any other kind — the
/// middleware one can wrap a `reqwest` error — by its kind alone. Formatting
/// either would write the Helius key into every log line a failover produces.
fn classify(err: ClientError) -> ReadError {
    match err.kind() {
        ClientErrorKind::Reqwest(e) => transport_class(
            e.status().map(|s| s.as_u16()),
            e.is_timeout(),
            e.is_connect(),
            e.is_builder(),
        ),
        ClientErrorKind::Io(e) => ReadError::Unavailable(format!("I/O error: {}", e.kind())),
        // A reply that is not JSON-RPC (a proxy's HTML error page served with a
        // 200) is that endpoint's problem, not the request's.
        ClientErrorKind::SerdeJson(_) => {
            ReadError::Unavailable("response was not JSON-RPC".to_string())
        }
        ClientErrorKind::RpcError(_) => ReadError::Other(anyhow!("{err}")),
        _ => ReadError::Other(anyhow!("RPC client error")),
    }
}

/// The failover rule for a `reqwest` error, from the facts that decide it.
///
/// With a status: a 429, a 5xx, or a refused credential (401, 402, 403) is this
/// endpoint's to fix — a revoked or exhausted key must not take the feed down
/// while the keyless fallback would serve — and any other status is the
/// request's. With no status, the request never got an answer — a timeout, a
/// failed connect, or a connection reset after it was set up — so the next
/// endpoint is tried, unless the request could not even be built, which is
/// local and the same everywhere.
fn transport_class(status: Option<u16>, timeout: bool, connect: bool, builder: bool) -> ReadError {
    match status {
        Some(code @ (401..=403 | 429 | 500..=599)) => {
            ReadError::Unavailable(format!("HTTP {code}"))
        }
        Some(code) => ReadError::Other(anyhow!("RPC endpoint answered HTTP {code}")),
        None if builder => ReadError::Other(anyhow!("RPC request could not be built")),
        None if timeout => ReadError::Unavailable("request timed out".to_string()),
        None if connect => ReadError::Unavailable("connection failed".to_string()),
        // A connection reset mid-request, or a body that would not decode.
        None => ReadError::Unavailable("no usable answer".to_string()),
    }
}

/// An ordered list of RPC endpoints, read primary-first (see the module docs
/// for the failover rule and why it keeps no health state).
pub struct RpcFailover<T = RpcClient> {
    endpoints: Vec<(String, T)>,
    budget: Duration,
    /// The endpoint that answered last, so a change is logged once rather than
    /// on every poll. `usize::MAX` until the first answer.
    active: AtomicUsize,
}

impl RpcFailover<RpcClient> {
    /// Build the list from `(label, url)` pairs, primary first. The label is
    /// what every log line names; the URL is never logged.
    ///
    /// Reads at `confirmed`: a spot tape wants the price now, and `finalized`
    /// trails it by a dozen seconds — most of a poll interval.
    pub fn new(endpoints: Vec<(String, String)>) -> Result<Self> {
        Self::with_readers(
            endpoints
                .into_iter()
                .map(|(label, url)| {
                    let client = RpcClient::new_with_timeout_and_commitment(
                        url,
                        CALL_BUDGET,
                        CommitmentConfig::confirmed(),
                    );
                    (label, client)
                })
                .collect(),
            CALL_BUDGET,
        )
    }
}

impl<T: AccountReader> RpcFailover<T> {
    /// Build the list over arbitrary readers — the test seam behind
    /// [`RpcFailover::new`].
    pub fn with_readers(endpoints: Vec<(String, T)>, budget: Duration) -> Result<Self> {
        if endpoints.is_empty() {
            bail!("an RPC endpoint list needs at least one endpoint");
        }
        Ok(Self {
            endpoints,
            budget,
            active: AtomicUsize::new(usize::MAX),
        })
    }

    /// Read `keys` from the first endpoint that serves them.
    pub async fn accounts(&self, keys: &[Pubkey]) -> Result<Vec<Option<AccountData>>> {
        let mut failures = Vec::new();
        for (index, (label, reader)) in self.endpoints.iter().enumerate() {
            let outcome = match tokio::time::timeout(self.budget, reader.accounts(keys)).await {
                Ok(outcome) => outcome,
                Err(_) => Err(ReadError::Unavailable(format!(
                    "no answer within {:?}",
                    self.budget
                ))),
            };
            match outcome {
                Ok(accounts) => {
                    self.note_active(index, label);
                    return Ok(accounts);
                }
                Err(ReadError::Unavailable(why)) => {
                    tracing::warn!(endpoint = %label, reason = %why, "RPC endpoint unavailable");
                    failures.push(format!("{label}: {why}"));
                }
                Err(ReadError::Other(err)) => {
                    return Err(err.context(format!("RPC endpoint {label} refused the read")));
                }
            }
        }
        Err(anyhow!(
            "every RPC endpoint is unavailable ({})",
            failures.join("; ")
        ))
    }

    /// Log the endpoint now serving reads, once per change.
    fn note_active(&self, index: usize, label: &str) {
        if self.active.swap(index, Ordering::Relaxed) != index {
            tracing::info!(endpoint = %label, "RPC endpoint active");
        }
    }

    /// The label of the endpoint that answered last, if any has.
    pub fn active_label(&self) -> Option<&str> {
        let index = self.active.load(Ordering::Relaxed);
        self.endpoints.get(index).map(|(label, _)| label.as_str())
    }
}

/// The fields of a Whirlpool this adapter reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PoolState {
    pub mint_a: Pubkey,
    pub mint_b: Pubkey,
    pub sqrt_price: u128,
    /// The liquidity in range at the current tick — zero when every position
    /// sits outside it or has been withdrawn.
    pub liquidity: u128,
}

/// Decode a Whirlpool account through Orca's generated decoder, refusing
/// anything the Whirlpool program does not own or that is not a pool.
///
/// The owner check is what makes the discriminator mean something: any program
/// can write those eight bytes into an account of its own, so a mistyped pinned
/// address could otherwise decode as a pool with a price of its owner's choosing.
pub fn parse_whirlpool(account: &AccountData) -> Result<PoolState> {
    if account.owner != WHIRLPOOL_ID {
        bail!(
            "account is owned by {}, not the Whirlpool program — the pinned \
             address is not a Whirlpool",
            account.owner
        );
    }
    if account.data.len() < Whirlpool::LEN || account.data[..8] != WHIRLPOOL_DISCRIMINATOR {
        bail!("account is not a Whirlpool pool (wrong length or discriminator)");
    }
    let pool = Whirlpool::from_bytes(&account.data).context("decoding the Whirlpool account")?;
    Ok(PoolState {
        mint_a: pool.token_mint_a,
        mint_b: pool.token_mint_b,
        sqrt_price: pool.sqrt_price,
        liquidity: pool.liquidity,
    })
}

/// A mint's `decimals`, from an SPL Token or Token-2022 mint account.
pub fn parse_mint_decimals(account: &AccountData) -> Result<u8> {
    if !TOKEN_PROGRAMS.contains(&account.owner) {
        bail!("account is owned by {}, not a token program", account.owner);
    }
    if account.data.len() < MINT_LEN {
        bail!(
            "account is {} bytes, too short for a mint",
            account.data.len()
        );
    }
    Ok(account.data[MINT_DECIMALS_OFFSET])
}

/// Whether a pool must be inverted to read as quote-per-base: `false` when it is
/// `base`/`quote` as mints A/B, `true` when it is the other way round, and an
/// error when it is not exactly that pair.
///
/// Both legs are checked, not only the quote: a pinned pool that trades some
/// other token against the quote would otherwise pass and be stored under the
/// wrong pair.
pub fn is_inverted(state: &PoolState, base_mint: &Pubkey, quote_mint: &Pubkey) -> Result<bool> {
    if state.mint_a == *base_mint && state.mint_b == *quote_mint {
        Ok(false)
    } else if state.mint_a == *quote_mint && state.mint_b == *base_mint {
        Ok(true)
    } else {
        bail!(
            "the pool trades {} against {}, not {base_mint} against \
             {quote_mint} — the pinned address is the wrong pool",
            state.mint_a,
            state.mint_b
        )
    }
}

/// The pool's price in quote per base, or `None` when it is not a positive
/// finite number (an uninitialized pool).
pub fn pool_price(sqrt_price: u128, decimals_a: u8, decimals_b: u8, inverted: bool) -> Option<f64> {
    let b_per_a = sqrt_price_to_price(sqrt_price, decimals_a, decimals_b);
    let price = if inverted { 1.0 / b_per_a } else { b_per_a };
    (price.is_finite() && price > 0.0).then_some(price)
}

/// What the first poll learns about a pool and later polls reuse.
#[derive(Clone, Copy, Debug)]
struct PoolMeta {
    mint_a: Pubkey,
    mint_b: Pubkey,
    decimals_a: u8,
    decimals_b: u8,
    inverted: bool,
}

/// A poll [`Source`] over one Whirlpool's spot price.
pub struct OrcaSpot<T = RpcClient> {
    rpc: Arc<RpcFailover<T>>,
    name: String,
    product_id: String,
    pool: Pubkey,
    base_mint: Pubkey,
    quote_mint: Pubkey,
    meta: Option<PoolMeta>,
}

impl<T: AccountReader> OrcaSpot<T> {
    /// Build the source for `product_id` over the pinned `pool`, whose legs are
    /// `base_mint` and `quote_mint`. Several sources share one [`RpcFailover`],
    /// so a roster draws on one endpoint list and logs one active endpoint.
    pub fn new(
        rpc: Arc<RpcFailover<T>>,
        product_id: impl Into<String>,
        pool: Pubkey,
        base_mint: Pubkey,
        quote_mint: Pubkey,
    ) -> Self {
        let product_id = product_id.into();
        Self {
            rpc,
            name: spot_feed_name(&product_id),
            product_id,
            pool,
            base_mint,
            quote_mint,
            meta: None,
        }
    }

    /// Read the pool's current price, or `None` when it has no usable one.
    pub async fn poll(&mut self) -> Result<Option<f64>> {
        let account = self
            .rpc
            .accounts(&[self.pool])
            .await?
            .into_iter()
            .next()
            .flatten()
            .ok_or_else(|| anyhow!("pool account {} does not exist", self.pool))?;
        let state = parse_whirlpool(&account)?;
        // The pair check first, so a mis-pinned pool fails loudly even when it
        // holds no liquidity, rather than reading as an empty feed forever.
        is_inverted(&state, &self.base_mint, &self.quote_mint)?;
        // No in-range liquidity: the pool keeps its last `sqrt_price`, but no
        // swap can trade at it, so it is not a price anyone could take.
        if state.liquidity == 0 {
            return Ok(None);
        }
        let meta = match self.meta {
            // A pool's mints are fixed at creation, so this only re-reads them
            // if the account somehow changed under the pinned address.
            Some(meta) if meta.mint_a == state.mint_a && meta.mint_b == state.mint_b => meta,
            _ => self.load_meta(&state).await?,
        };
        Ok(pool_price(
            state.sqrt_price,
            meta.decimals_a,
            meta.decimals_b,
            meta.inverted,
        ))
    }

    /// Read both mints once and fix the pool's orientation.
    async fn load_meta(&mut self, state: &PoolState) -> Result<PoolMeta> {
        let inverted = is_inverted(state, &self.base_mint, &self.quote_mint)?;
        let mints = self.rpc.accounts(&[state.mint_a, state.mint_b]).await?;
        let decimals = |i: usize, mint: &Pubkey| -> Result<u8> {
            let account = mints
                .get(i)
                .cloned()
                .flatten()
                .ok_or_else(|| anyhow!("mint {mint} does not exist"))?;
            parse_mint_decimals(&account).with_context(|| format!("reading mint {mint}"))
        };
        let meta = PoolMeta {
            mint_a: state.mint_a,
            mint_b: state.mint_b,
            decimals_a: decimals(0, &state.mint_a)?,
            decimals_b: decimals(1, &state.mint_b)?,
            inverted,
        };
        self.meta = Some(meta);
        Ok(meta)
    }
}

#[async_trait]
impl<T: AccountReader + 'static> Source for OrcaSpot<T> {
    /// The product id and its price — the Coinbase ticker's record shape, so a
    /// collector maps both the same way.
    type Record = (String, f64);

    fn name(&self) -> &str {
        &self.name
    }

    async fn next(&mut self) -> Result<Batch<Self::Record>> {
        // An unusable price is no record, never a zero: a consumer keyed by
        // product would cache a zero as a reading, where an empty batch lets
        // the last real one age out.
        let records = self
            .poll()
            .await?
            .map(|price| vec![(self.product_id.clone(), price)])
            .unwrap_or_default();
        Ok(Batch::new(records).with_caught_up(true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// The tGBP-USDC Whirlpool (`9aJrewHLYMjXiZK5u2UU4aWi4R3jFGtoKJJo4AaHYyjt`)
    /// as `getAccountInfo` returned it at slot 459796781. Orca's own API priced
    /// the pool at 1.33686020799 USDC per tGBP at the same moment.
    const FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/orca_whirlpool_tgbp_usdc.bin");
    const TGBP: Pubkey = Pubkey::from_str_const("2zMqyX4AYCk6mgy5UZ2S7zUaLxwERhK5WjqDzkPPbSpW");
    const USDC: Pubkey = Pubkey::from_str_const("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");
    const POOL: Pubkey = Pubkey::from_str_const("9aJrewHLYMjXiZK5u2UU4aWi4R3jFGtoKJJo4AaHYyjt");

    fn pool_account() -> AccountData {
        AccountData {
            owner: WHIRLPOOL_ID,
            data: FIXTURE.to_vec(),
        }
    }

    fn mint_account(decimals: u8) -> AccountData {
        let mut data = vec![0u8; MINT_LEN];
        data[MINT_DECIMALS_OFFSET] = decimals;
        AccountData {
            owner: TOKEN_PROGRAMS[0],
            data,
        }
    }

    #[test]
    fn the_captured_pool_decodes_to_its_known_price() {
        let state = parse_whirlpool(&pool_account()).unwrap();
        assert_eq!(state.mint_a, TGBP);
        assert_eq!(state.mint_b, USDC);
        assert!(!is_inverted(&state, &TGBP, &USDC).unwrap());
        assert!(state.liquidity > 0, "the captured pool is live");
        // tGBP has 9 decimals and USDC 6.
        let price = pool_price(state.sqrt_price, 9, 6, false).unwrap();
        assert!((price - 1.336_860_207_99).abs() < 1e-9, "got {price}");
    }

    #[test]
    fn a_pool_quoted_the_other_way_round_is_inverted() {
        let state = PoolState {
            mint_a: USDC,
            mint_b: TGBP,
            sqrt_price: 0,
            liquidity: 1,
        };
        assert!(is_inverted(&state, &TGBP, &USDC).unwrap());
        // The same pool seen from USDC's side: 1/1.3368… tGBP per USDC, and
        // inverting it recovers USDC per tGBP.
        let sqrt_price = parse_whirlpool(&pool_account()).unwrap().sqrt_price;
        let direct = pool_price(sqrt_price, 9, 6, false).unwrap();
        let inverted = pool_price(sqrt_price, 9, 6, true).unwrap();
        assert!((direct * inverted - 1.0).abs() < 1e-12);
    }

    #[test]
    fn a_pool_without_the_quote_mint_is_refused() {
        let state = parse_whirlpool(&pool_account()).unwrap();
        let err = is_inverted(&state, &TGBP, &Pubkey::new_unique()).unwrap_err();
        assert!(err.to_string().contains("wrong pool"), "{err}");
    }

    #[test]
    fn a_pool_trading_another_token_against_the_quote_is_refused() {
        // The quote matches, so a quote-only check would pass this and store
        // some other token's USDC price as tGBP's.
        let state = parse_whirlpool(&pool_account()).unwrap();
        let err = is_inverted(&state, &Pubkey::new_unique(), &USDC).unwrap_err();
        assert!(err.to_string().contains("wrong pool"), "{err}");
    }

    #[test]
    fn the_failover_rule_tries_the_next_endpoint_only_where_it_could_help() {
        let fails_over = |status, timeout, connect, builder| {
            matches!(
                transport_class(status, timeout, connect, builder),
                ReadError::Unavailable(_)
            )
        };
        // Rate limit, server error, and a refused credential: this endpoint's.
        for code in [401, 402, 403, 429, 500, 503] {
            assert!(fails_over(Some(code), false, false, false), "HTTP {code}");
        }
        // Any other status is the request's, the same everywhere.
        for code in [400, 404, 413] {
            assert!(!fails_over(Some(code), false, false, false), "HTTP {code}");
        }
        // No answer at all: a timeout, a failed connect, or a reset after one.
        assert!(fails_over(None, true, false, false));
        assert!(fails_over(None, false, true, false));
        assert!(fails_over(None, false, false, false));
        // A request that could not be built is local.
        assert!(!fails_over(None, false, false, true));
    }

    #[test]
    fn a_non_transport_error_is_described_without_its_text() {
        // `Custom` stands in for every kind `classify` declines to format: its
        // message must not reach the log, since a wrapped transport error
        // would carry the endpoint URL.
        let err = ClientError::from(ClientErrorKind::Custom(
            "https://keyed.example/?api-key=secret".to_string(),
        ));
        let ReadError::Other(described) = classify(err) else {
            panic!("an unrecognized kind is not a reason to fail over");
        };
        assert!(!described.to_string().contains("secret"), "{described}");
    }

    #[test]
    fn an_account_the_whirlpool_program_does_not_own_is_refused() {
        // The discriminator alone proves nothing: any program can write it.
        let forged = AccountData {
            owner: Pubkey::new_unique(),
            data: FIXTURE.to_vec(),
        };
        assert!(parse_whirlpool(&forged).is_err());
    }

    #[test]
    fn a_short_or_mislabelled_account_is_refused() {
        let mut short = pool_account();
        short.data.truncate(100);
        assert!(parse_whirlpool(&short).is_err());
        let mut relabelled = pool_account();
        relabelled.data[0] ^= 0xff;
        assert!(parse_whirlpool(&relabelled).is_err());
    }

    #[test]
    fn mint_decimals_are_read_only_from_a_token_program_mint() {
        assert_eq!(parse_mint_decimals(&mint_account(9)).unwrap(), 9);
        let mut stranger = mint_account(9);
        stranger.owner = Pubkey::new_unique();
        assert!(parse_mint_decimals(&stranger).is_err());
        let mut short = mint_account(9);
        short.data.truncate(MINT_DECIMALS_OFFSET);
        assert!(parse_mint_decimals(&short).is_err());
    }

    #[test]
    fn a_zero_sqrt_price_is_no_price_rather_than_zero() {
        assert_eq!(pool_price(0, 9, 6, false), None);
        assert_eq!(pool_price(0, 9, 6, true), None);
    }

    #[test]
    fn spot_sources_are_named_per_product() {
        assert_eq!(spot_feed_name("TGBP-USDC"), "orca:TGBP-USDC");
    }

    /// One scripted endpoint: either it serves the account map, or it fails
    /// with the given class. Counts its calls so a test can see it was skipped.
    struct FakeEndpoint {
        accounts: HashMap<Pubkey, AccountData>,
        /// Interior so a test can bring a failed endpoint back mid-run.
        fail: Mutex<Option<fn() -> ReadError>>,
        hang: bool,
        calls: Mutex<usize>,
    }

    impl FakeEndpoint {
        fn serving() -> Self {
            Self {
                accounts: HashMap::from([
                    (POOL, pool_account()),
                    (TGBP, mint_account(9)),
                    (USDC, mint_account(6)),
                ]),
                fail: Mutex::new(None),
                hang: false,
                calls: Mutex::new(0),
            }
        }

        fn failing(fail: fn() -> ReadError) -> Self {
            Self {
                fail: Mutex::new(Some(fail)),
                ..Self::serving()
            }
        }

        fn recover(&self) {
            *self.fail.lock().unwrap() = None;
        }

        fn hanging() -> Self {
            Self {
                hang: true,
                ..Self::serving()
            }
        }

        fn calls(&self) -> usize {
            *self.calls.lock().unwrap()
        }
    }

    #[async_trait]
    impl AccountReader for FakeEndpoint {
        async fn accounts(&self, keys: &[Pubkey]) -> Result<Vec<Option<AccountData>>, ReadError> {
            *self.calls.lock().unwrap() += 1;
            if self.hang {
                std::future::pending::<()>().await;
            }
            if let Some(fail) = *self.fail.lock().unwrap() {
                return Err(fail());
            }
            Ok(keys.iter().map(|k| self.accounts.get(k).cloned()).collect())
        }
    }

    fn unavailable() -> ReadError {
        ReadError::Unavailable("HTTP 429 Too Many Requests".to_string())
    }

    fn refused() -> ReadError {
        ReadError::Other(anyhow!("invalid param"))
    }

    fn failover(endpoints: Vec<FakeEndpoint>) -> Arc<RpcFailover<FakeEndpoint>> {
        let labelled = endpoints
            .into_iter()
            .enumerate()
            .map(|(i, e)| (format!("endpoint-{i}"), e))
            .collect();
        Arc::new(RpcFailover::with_readers(labelled, Duration::from_millis(50)).unwrap())
    }

    #[tokio::test]
    async fn the_source_prices_the_pool_end_to_end() {
        let mut source = OrcaSpot::new(
            failover(vec![FakeEndpoint::serving()]),
            "TGBP-USDC",
            POOL,
            TGBP,
            USDC,
        );
        let batch = source.next().await.unwrap();
        assert_eq!(batch.records.len(), 1);
        let (product, price) = &batch.records[0];
        assert_eq!(product, "TGBP-USDC");
        assert!((price - 1.336_860_207_99).abs() < 1e-9, "got {price}");
        // The mints are read once and reused: three calls for two polls
        // (pool + mints, then pool alone), not four.
        source.next().await.unwrap();
        assert_eq!(source.rpc.endpoints[0].1.calls(), 3);
    }

    #[tokio::test]
    async fn a_pool_with_no_liquidity_in_range_yields_no_record() {
        // The captured pool with its `liquidity` (bytes 49..65, after the
        // discriminator, config, bump, tick spacing, seed and both fee rates)
        // zeroed: its `sqrt_price` is still the live one, which is the trap.
        let mut endpoint = FakeEndpoint::serving();
        let drained = endpoint.accounts.get_mut(&POOL).unwrap();
        drained.data[49..65].fill(0);
        assert_eq!(parse_whirlpool(drained).unwrap().liquidity, 0);
        let mut source = OrcaSpot::new(failover(vec![endpoint]), "TGBP-USDC", POOL, TGBP, USDC);
        assert!(source.next().await.unwrap().records.is_empty());
    }

    #[tokio::test]
    async fn an_unavailable_primary_falls_back_and_is_retried_next_poll() {
        let rpc = failover(vec![
            FakeEndpoint::failing(unavailable),
            FakeEndpoint::serving(),
        ]);
        rpc.accounts(&[POOL]).await.unwrap();
        assert_eq!(rpc.active_label(), Some("endpoint-1"));
        // Every read starts from the top, so the primary is asked again.
        rpc.accounts(&[POOL]).await.unwrap();
        assert_eq!(rpc.endpoints[0].1.calls(), 2);
    }

    #[tokio::test]
    async fn a_primary_that_answers_again_is_active_again() {
        let rpc = failover(vec![
            FakeEndpoint::failing(unavailable),
            FakeEndpoint::serving(),
        ]);
        rpc.accounts(&[POOL]).await.unwrap();
        assert_eq!(rpc.active_label(), Some("endpoint-1"));
        rpc.endpoints[0].1.recover();
        rpc.accounts(&[POOL]).await.unwrap();
        assert_eq!(rpc.active_label(), Some("endpoint-0"));
        assert_eq!(
            rpc.endpoints[1].1.calls(),
            1,
            "the fallback is not asked once the primary answers"
        );
    }

    #[tokio::test]
    async fn a_hung_primary_is_cut_off_by_the_call_budget() {
        // The 429-retry stall the outer budget exists for: the primary never
        // answers, and the read still completes from the fallback.
        let rpc = failover(vec![FakeEndpoint::hanging(), FakeEndpoint::serving()]);
        rpc.accounts(&[POOL]).await.unwrap();
        assert_eq!(rpc.active_label(), Some("endpoint-1"));
    }

    #[tokio::test]
    async fn a_refused_read_does_not_fail_over() {
        // An error the endpoint answered with would be the same anywhere, so
        // the fallback is not asked.
        let rpc = failover(vec![
            FakeEndpoint::failing(refused),
            FakeEndpoint::serving(),
        ]);
        assert!(rpc.accounts(&[POOL]).await.is_err());
        assert_eq!(rpc.endpoints[1].1.calls(), 0);
    }

    #[tokio::test]
    async fn every_endpoint_unavailable_names_each_by_label() {
        let rpc = failover(vec![
            FakeEndpoint::failing(unavailable),
            FakeEndpoint::failing(unavailable),
        ]);
        let err = rpc.accounts(&[POOL]).await.unwrap_err().to_string();
        assert!(
            err.contains("endpoint-0") && err.contains("endpoint-1"),
            "{err}"
        );
    }

    #[test]
    fn an_empty_endpoint_list_is_refused() {
        assert!(RpcFailover::<FakeEndpoint>::with_readers(vec![], CALL_BUDGET).is_err());
    }
}
