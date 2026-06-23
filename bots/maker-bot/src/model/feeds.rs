//! Price feeds (§1).
//!
//! Three blocking HTTP sources behind one [`Feeds`] poller: CoinGecko
//! (primary CADC/USD), Oanda Practice (FX CAD/USD, inverted from USD/CAD, for
//! the peg sanity bound), and — when configured — Aerodrome via GeckoTerminal
//! (on-chain CADC/USDC). Each `poll_*` returns the latest value; the caller
//! stamps the read time for the freshness rules in [`super::fair_mid`]. The
//! JSON shapes are decoded by the free `parse_*` functions, which are unit
//! tested against captured responses; only the transport needs a network.

use crate::config::{AerodromeConfig, FeedConfig};
use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::time::Duration;

/// Environment variable holding the Oanda Practice API key (never committed).
pub const OANDA_KEY_ENV: &str = "OANDA_API_KEY";

/// Blocking poller over the configured feeds.
pub struct Feeds {
    agent: ureq::Agent,
    cfg: FeedConfig,
    oanda_key: Option<String>,
}

impl Feeds {
    /// Build a poller, reading the Oanda key from the environment if present.
    pub fn new(cfg: FeedConfig) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(5))
            .build();
        let oanda_key = std::env::var(OANDA_KEY_ENV).ok().filter(|k| !k.is_empty());
        Self {
            agent,
            cfg,
            oanda_key,
        }
    }

    /// CoinGecko CADC/USD (primary CADC market price).
    pub fn poll_coingecko(&self) -> Result<f64> {
        let url = "https://api.coingecko.com/api/v3/simple/price";
        let body: Value = self
            .agent
            .get(url)
            .query("ids", &self.cfg.coingecko_id)
            .query("vs_currencies", "usd")
            .call()
            .context("coingecko request")?
            .into_json()
            .context("coingecko json")?;
        parse_coingecko(&body, &self.cfg.coingecko_id)
    }

    /// Oanda CAD/USD (FX spot), inverted from the USD/CAD instrument — the
    /// peg sanity feed, not a `fair_mid` input.
    pub fn poll_oanda(&self) -> Result<f64> {
        let key = self
            .oanda_key
            .as_ref()
            .ok_or_else(|| anyhow!("{OANDA_KEY_ENV} not set"))?;
        let url = format!(
            "{}/v3/instruments/{}/candles",
            self.cfg.oanda_base_url, self.cfg.oanda_instrument
        );
        let body: Value = self
            .agent
            .get(&url)
            .set("Authorization", &format!("Bearer {key}"))
            .query("count", "1")
            .query("granularity", "M1")
            .query("price", "M")
            .call()
            .context("oanda request")?
            .into_json()
            .context("oanda json")?;
        parse_oanda(&body)
    }

    /// Aerodrome CADC/USDC via GeckoTerminal, when configured.
    pub fn poll_aerodrome(&self) -> Result<f64> {
        let ae = self
            .cfg
            .aerodrome
            .as_ref()
            .ok_or_else(|| anyhow!("aerodrome feed not configured"))?;
        let url = aerodrome_url(ae);
        let body: Value = self
            .agent
            .get(&url)
            .call()
            .context("aerodrome request")?
            .into_json()
            .context("aerodrome json")?;
        parse_aerodrome(&body)
    }

    /// Whether the Aerodrome feed is wired up this run.
    pub fn aerodrome_enabled(&self) -> bool {
        self.cfg.aerodrome.is_some()
    }
}

/// GeckoTerminal pool endpoint for an Aerodrome pool.
fn aerodrome_url(ae: &AerodromeConfig) -> String {
    format!(
        "https://api.geckoterminal.com/api/v2/networks/{}/pools/{}",
        ae.network, ae.pool
    )
}

/// Decode CoinGecko's `{"<id>":{"usd":<n>}}` simple-price response.
pub fn parse_coingecko(body: &Value, id: &str) -> Result<f64> {
    body.get(id)
        .and_then(|v| v.get("usd"))
        .and_then(Value::as_f64)
        .ok_or_else(|| anyhow!("coingecko: missing {id}.usd in {body}"))
}

/// Decode Oanda's candles response and invert the latest complete USD/CAD
/// close to CAD/USD. Oanda renders prices as JSON strings.
pub fn parse_oanda(body: &Value) -> Result<f64> {
    let candles = body
        .get("candles")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("oanda: no candles array"))?;
    let close = candles
        .iter()
        .rev()
        .find(|c| c.get("complete").and_then(Value::as_bool).unwrap_or(false))
        .or_else(|| candles.last())
        .and_then(|c| c.get("mid"))
        .and_then(|m| m.get("c"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("oanda: no candle close"))?;
    let usd_cad: f64 = close.parse().context("oanda close not a number")?;
    if usd_cad <= 0.0 {
        return Err(anyhow!("oanda: non-positive close {usd_cad}"));
    }
    Ok(1.0 / usd_cad)
}

/// Decode GeckoTerminal's pool response — the base token (CADC) price in USD.
/// The pool's base/quote orientation is verified during live feed testing; if
/// inverted, the quote-token price is the CADC/USD reading instead.
pub fn parse_aerodrome(body: &Value) -> Result<f64> {
    body.get("data")
        .and_then(|d| d.get("attributes"))
        .and_then(|a| a.get("base_token_price_usd"))
        .and_then(|p| match p {
            Value::String(s) => s.parse().ok(),
            Value::Number(n) => n.as_f64(),
            _ => None,
        })
        .ok_or_else(|| anyhow!("aerodrome: missing data.attributes.base_token_price_usd"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_coingecko_simple_price() {
        let body = json!({ "cad-coin": { "usd": 0.7301 } });
        assert_eq!(parse_coingecko(&body, "cad-coin").unwrap(), 0.7301);
    }

    #[test]
    fn coingecko_missing_id_errors() {
        let body = json!({ "other-coin": { "usd": 1.0 } });
        assert!(parse_coingecko(&body, "cad-coin").is_err());
    }

    #[test]
    fn parses_and_inverts_oanda_close() {
        let body = json!({
            "candles": [
                { "complete": true, "mid": { "c": "1.3699" } },
                { "complete": false, "mid": { "c": "1.3705" } }
            ]
        });
        // Inverts the latest *complete* USD/CAD close to CAD/USD.
        let cad_usd = parse_oanda(&body).unwrap();
        assert!((cad_usd - 1.0 / 1.3699).abs() < 1e-9);
    }

    #[test]
    fn oanda_falls_back_to_last_candle_when_none_complete() {
        let body = json!({ "candles": [ { "complete": false, "mid": { "c": "1.37" } } ] });
        assert!((parse_oanda(&body).unwrap() - 1.0 / 1.37).abs() < 1e-9);
    }

    #[test]
    fn parses_aerodrome_string_price() {
        let body = json!({ "data": { "attributes": { "base_token_price_usd": "0.7299" } } });
        assert!((parse_aerodrome(&body).unwrap() - 0.7299).abs() < 1e-9);
    }

    #[test]
    fn builds_geckoterminal_pool_url() {
        let ae = AerodromeConfig {
            network: "base".into(),
            pool: "0xpool".into(),
            poll: Duration::from_secs(10),
        };
        assert_eq!(
            aerodrome_url(&ae),
            "https://api.geckoterminal.com/api/v2/networks/base/pools/0xpool"
        );
    }
}
