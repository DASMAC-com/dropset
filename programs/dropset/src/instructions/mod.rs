pub mod admin;
pub mod close_vault;
pub mod deposit;
pub mod deposit_leader;
pub mod freeze_vault;
pub mod init;
pub mod register_market;
pub mod register_vault;
pub mod set_liquidity_profile;
pub mod set_outside_deposits;
pub mod set_reference_price;
pub mod swap;
pub mod withdraw;
pub mod withdraw_leader;
// Teardown surface. The handlers always compile and are always wired
// into the program, but each dispatcher (see `lib.rs`) short-circuits to
// `DropsetError::TeardownDisabled` unless the `admin-teardown` Cargo
// feature is on — so testnet / early-mainnet builds expose them and the
// final immutable build leaves them present-but-inert. anchor v2's
// `#[program]` macro does not propagate `#[cfg]` from a handler fn onto
// its generated dispatch glue, so a clean per-instruction compile-out
// isn't available; the runtime guard is the supported alternative. See
// the architecture spec, § Account lifecycle and rent reclamation.
pub mod close_market;
pub mod close_registry;
pub mod force_withdraw;

pub use admin::*;
pub use close_vault::*;
pub use deposit::*;
pub use deposit_leader::*;
pub use freeze_vault::*;
pub use init::*;
pub use register_market::*;
pub use register_vault::*;
pub use set_liquidity_profile::*;
pub use set_outside_deposits::*;
pub use set_reference_price::*;
pub use swap::*;
pub use withdraw::*;
pub use withdraw_leader::*;
pub use close_market::*;
pub use close_registry::*;
pub use force_withdraw::*;
