//! The `Source` trait — where records come from.

use crate::record::Batch;
use anyhow::Result;
use async_trait::async_trait;

/// A data source. Implemented by poll adapters (REST via [`crate::HttpClient`],
/// RPC via [`crate::RpcPollSource`]) and subscribe adapters (a stream bridged
/// through [`crate::ChannelSource`]). The runner drives one source and neither
/// it nor the sinks know which drive shape it is.
#[async_trait]
pub trait Source: Send {
    /// The typed record this source yields.
    type Record: Send + 'static;

    /// A stable identifier — the cursor key, and the label in logs and
    /// metrics, e.g. `cex:coinbase:EURC-USDC`.
    fn name(&self) -> &str;

    /// Fetch or receive the next batch. A poll source computes its window from
    /// its resume cursor and reports `caught_up = false` while a backlog
    /// remains; a subscribe source awaits the next pushed records.
    async fn next(&mut self) -> Result<Batch<Self::Record>>;
}
