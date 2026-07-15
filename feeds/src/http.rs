//! The HTTP-REST poll transport (`http` feature).

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use std::time::Duration;

/// A small JSON-over-HTTPS client REST poll sources compose: a base URL, a
/// shared `reqwest` client, and [`HttpClient::get_json`]. The Coinbase
/// reference feed uses it first; the FX / Circle-rate feeds follow
/// (docs/data-feeds.md §4). It is a transport, not a `Source`: a feed wraps it
/// in its own [`crate::Source`] that decodes the JSON into typed records and
/// computes its cursor.
#[derive(Clone)]
pub struct HttpClient {
    base_url: String,
    client: reqwest::Client,
}

impl HttpClient {
    /// A client rooted at `base_url` (e.g. `https://api.exchange.coinbase.com`),
    /// with a request timeout and a stable user agent.
    pub fn new(base_url: impl Into<String>) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent(concat!("dropset-feeds/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("build HTTP client")?;
        Ok(Self {
            base_url: base_url.into(),
            client,
        })
    }

    /// GET `{base_url}{path}` with optional query params, decoding the JSON
    /// body into `T`. A non-success status is an error.
    pub async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);
        let body = self
            .client
            .get(&url)
            .query(query)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?
            .error_for_status()
            .with_context(|| format!("GET {url} returned an error status"))?
            .json::<T>()
            .await
            .with_context(|| format!("decode JSON from {url}"))?;
        Ok(body)
    }
}
