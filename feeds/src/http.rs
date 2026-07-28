//! The HTTP-REST poll transport (`http` feature).

use anyhow::{Context, Result};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
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
    /// Headers sent on every request — the seam for an auth key a source's API
    /// requires on each call (CoinMarketCap's `X-CMC_PRO_API_KEY`, a Circle
    /// bearer token), set with [`HttpClient::with_header`].
    headers: HeaderMap,
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
            headers: HeaderMap::new(),
        })
    }

    /// Add a header sent on every request, for an API that authenticates a poll
    /// with a static key (an errored `name` / `value` is rejected here rather
    /// than per request). Chains from [`HttpClient::new`].
    pub fn with_header(mut self, name: &str, value: &str) -> Result<Self> {
        let name = HeaderName::from_bytes(name.as_bytes())
            .with_context(|| format!("invalid header name {name:?}"))?;
        let value = HeaderValue::from_str(value).context("invalid header value")?;
        self.headers.insert(name, value);
        Ok(self)
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
            .headers(self.headers.clone())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_header_accepts_a_valid_pair_and_rejects_a_malformed_name() {
        // A valid auth-style header composes onto the client.
        let ok = HttpClient::new("https://example.test")
            .unwrap()
            .with_header("X-CMC_PRO_API_KEY", "secret");
        assert!(ok.is_ok());
        // A space is not legal in a header name — caught at wiring time, not on
        // the first (network) request.
        let bad = HttpClient::new("https://example.test")
            .unwrap()
            .with_header("bad name", "v");
        assert!(bad.is_err());
    }
}
