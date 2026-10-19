//! `dropset-tui` — the localnet control-plane modules.
//!
//! Exposed as a library so both binaries share one code path:
//! the interactive panel (`main.rs`) and the headless `dropset-teardown`
//! reclaim script (`bin/teardown.rs`). The teardown / rent-reclamation flow
//! lives in [`teardown`] and is driven identically from the TUI's
//! "Teardown & reclaim" action and from the standalone binary — there is no
//! second implementation to drift.

pub mod accounts;
pub mod action;
pub mod app;
pub mod book;
pub mod bot;
pub mod ceremony;
pub mod chain;
pub mod cluster;
pub mod deploy;
#[cfg(test)]
mod e2e;
pub mod emergency;
pub mod explorer;
pub mod fills;
pub mod job;
pub mod leader;
pub mod market;
pub mod passive;
pub mod publish;
pub mod teardown;
pub mod ui;
pub mod validator;
pub mod wallet;
