//! Cross-cutting plumbing shared by the off-chain crates (the TUI and the
//! maker / taker bots).
//!
//! Three parallel copies of this code had drifted across `tui/`,
//! `bots/maker-bot/`, and `bots/taker-bot/`; hoisting it here means a fix
//! lands once. The two halves serve unrelated paths and are kept apart as
//! modules:
//!
//! * [`localnet`] — **SPL plumbing** for seeding a local validator: the SPL
//!   Token / Associated-Token-Account / System program ids, the canonical ATA
//!   derivation, and the raw byte-instruction builders for `CreateIdempotent`
//!   and `MintTo`. These are *pure*: they return an `Instruction` (or a
//!   `Pubkey`) and take no `RpcClient` or `Keypair`, so each consumer keeps
//!   its own sign-and-send path — the TUI's carries compute-unit measurement
//!   the bots don't need.
//! * [`rpc`] — the Agave `http`→`ws` PubSub-endpoint derivation the fill
//!   subscriptions share. Not localnet-specific: the maker's `logsSubscribe`
//!   path needs it against any cluster.

pub mod localnet;
pub mod rpc;
