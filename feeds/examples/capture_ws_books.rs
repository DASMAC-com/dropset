//! Capture raw public order-book frames from Kraken and Coinbase, so the
//! wire-level facts the venue docs leave unstated can be read off real
//! messages, and so the frames can be replayed as test fixtures
//! (`feeds/tests/fixtures/ws/`).
//!
//! ```sh
//! cargo run -p dropset-feeds --example capture_ws_books -- \
//!   --out target/ws-capture [--updates 200] [--minutes 10]
//! ```
//!
//! Opens one connection per feed, all at once so the two Coinbase feeds are
//! captured over the same window:
//!
//! - Kraken WebSocket v2 `book`, depth 10 and depth 100 on separate sockets
//!   (a `book` message names its symbol but not its depth, so one socket
//!   carrying both depths would be ambiguous). The depth-10 socket also
//!   subscribes `instrument`, whose precision fields rebuild trimmed digits.
//! - Coinbase Advanced Trade `level2` plus `heartbeats` on one socket.
//! - Coinbase Exchange `level2_batch`.
//!
//! Every frame is written as one line, `<receive unix micros>\t<frame text>`,
//! the frame text byte for byte as received. A socket stops once each of its
//! products has seen `--updates` updates or `--minutes` have passed, whichever
//! comes first. No API key, no authenticated channel, no reconnect: a socket
//! that closes or errors ends that feed's capture, and the error is reported.
//! The handshake sends only the headers the WebSocket upgrade requires.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;
use tokio_tungstenite::tungstenite::Message;

/// Which venue grammar a socket speaks, for counting its updates.
#[derive(Clone, Copy)]
enum Grammar {
    Kraken,
    AdvancedTrade,
    Exchange,
}

/// One socket: where it connects, what it subscribes, and the products whose
/// updates it counts toward the stop condition.
struct Feed {
    name: &'static str,
    url: &'static str,
    grammar: Grammar,
    products: &'static [&'static str],
    subscribe: Vec<Value>,
}

const KRAKEN_URL: &str = "wss://ws.kraken.com/v2";
const KRAKEN_SYMBOLS: &[&str] = &["EURC/USD", "EURC/EUR", "TGBP/USD"];
const ADVANCED_TRADE_URL: &str = "wss://advanced-trade-ws.coinbase.com";
const ADVANCED_TRADE_PRODUCTS: &[&str] = &["EURC-USDC", "TGBP-USDC", "XSGD-USDC", "AUDD-USDC"];
const EXCHANGE_URL: &str = "wss://ws-feed.exchange.coinbase.com";
const EXCHANGE_PRODUCTS: &[&str] = &["EURC-USDC", "TGBP-USDC"];

fn kraken_book(depth: u32) -> Value {
    json!({
        "method": "subscribe",
        "params": {"channel": "book", "symbol": KRAKEN_SYMBOLS, "depth": depth, "snapshot": true},
    })
}

fn feeds() -> Vec<Feed> {
    vec![
        Feed {
            name: "kraken_book_depth10",
            url: KRAKEN_URL,
            grammar: Grammar::Kraken,
            products: KRAKEN_SYMBOLS,
            subscribe: vec![
                kraken_book(10),
                json!({
                    "method": "subscribe",
                    "params": {"channel": "instrument", "snapshot": true},
                }),
            ],
        },
        Feed {
            name: "kraken_book_depth100",
            url: KRAKEN_URL,
            grammar: Grammar::Kraken,
            products: KRAKEN_SYMBOLS,
            subscribe: vec![kraken_book(100)],
        },
        Feed {
            name: "coinbase_advanced_trade_level2",
            url: ADVANCED_TRADE_URL,
            grammar: Grammar::AdvancedTrade,
            products: ADVANCED_TRADE_PRODUCTS,
            // One channel per subscribe message on this API.
            subscribe: vec![
                json!({"type": "subscribe", "product_ids": ADVANCED_TRADE_PRODUCTS, "channel": "level2"}),
                json!({"type": "subscribe", "product_ids": ADVANCED_TRADE_PRODUCTS, "channel": "heartbeats"}),
            ],
        },
        Feed {
            name: "coinbase_exchange_level2_batch",
            url: EXCHANGE_URL,
            grammar: Grammar::Exchange,
            products: EXCHANGE_PRODUCTS,
            subscribe: vec![json!({
                "type": "subscribe",
                "product_ids": EXCHANGE_PRODUCTS,
                "channels": ["level2_batch"],
            })],
        },
    ]
}

/// The products a frame carries updates for, one entry per update message
/// (not per level), so the count is "update messages per product".
fn updated_products(grammar: Grammar, frame: &Value) -> Vec<String> {
    let text = |v: &Value| v.as_str().map(str::to_owned);
    match grammar {
        Grammar::Kraken => {
            if frame["channel"] != "book" || frame["type"] != "update" {
                return Vec::new();
            }
            frame["data"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|d| text(&d["symbol"]))
                .collect()
        }
        Grammar::AdvancedTrade => {
            if frame["channel"] != "l2_data" {
                return Vec::new();
            }
            frame["events"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|e| e["type"] == "update")
                .filter_map(|e| text(&e["product_id"]))
                .collect()
        }
        Grammar::Exchange => {
            if frame["type"] != "l2update" {
                return Vec::new();
            }
            text(&frame["product_id"]).into_iter().collect()
        }
    }
}

fn unix_micros() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros())
        .unwrap_or(0)
}

struct Outcome {
    frames: usize,
    counts: BTreeMap<String, usize>,
    ended_by: String,
}

async fn capture(feed: Feed, dir: PathBuf, target: usize, window: Duration) -> Result<Outcome> {
    let path = dir.join(format!("{}.raw.tsv", feed.name));
    let mut out = tokio::fs::File::create(&path)
        .await
        .with_context(|| format!("create {}", path.display()))?;
    let (mut socket, _) = tokio_tungstenite::connect_async(feed.url)
        .await
        .with_context(|| format!("{}: connect {}", feed.name, feed.url))?;
    for message in &feed.subscribe {
        socket.send(Message::text(message.to_string())).await?;
    }

    let mut counts: BTreeMap<String, usize> =
        feed.products.iter().map(|p| ((*p).to_owned(), 0)).collect();
    let mut frames = 0;
    let deadline = tokio::time::Instant::now() + window;
    let ended_by = loop {
        let next = match tokio::time::timeout_at(deadline, socket.next()).await {
            Err(_) => break format!("{} minute window elapsed", window.as_secs() / 60),
            Ok(None) => break "socket ended".to_owned(),
            Ok(Some(Err(e))) => break format!("socket error: {e}"),
            Ok(Some(Ok(message))) => message,
        };
        let text = match next {
            Message::Text(text) => text.to_string(),
            Message::Binary(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Message::Close(frame) => break format!("server closed: {frame:?}"),
            // tungstenite answers pings itself.
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
        };
        if text.contains('\n') {
            bail!(
                "{}: a frame spans lines; the one-line format cannot hold it",
                feed.name
            );
        }
        out.write_all(format!("{}\t{text}\n", unix_micros()).as_bytes())
            .await?;
        frames += 1;
        if let Ok(frame) = serde_json::from_str::<Value>(&text) {
            for product in updated_products(feed.grammar, &frame) {
                *counts.entry(product).or_default() += 1;
            }
        }
        if feed
            .products
            .iter()
            .all(|p| counts.get(*p).copied().unwrap_or(0) >= target)
        {
            break format!("{target} updates per product reached");
        }
    };
    out.flush().await?;
    let _ = socket.close(None).await;
    Ok(Outcome {
        frames,
        counts,
        ended_by,
    })
}

fn parse_args() -> Result<(PathBuf, usize, u64)> {
    let mut out = PathBuf::from("target/ws-capture");
    let mut updates = 200;
    let mut minutes = 10;
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .with_context(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--out" => out = PathBuf::from(value),
            "--updates" => updates = value.parse()?,
            "--minutes" => minutes = value.parse()?,
            other => bail!("unknown flag {other}"),
        }
    }
    Ok((out, updates, minutes))
}

fn report(dir: &Path, name: &str, result: Result<Outcome>) {
    match result {
        Ok(o) => println!(
            "{name}: {} frames, {:?}, ended: {} -> {}",
            o.frames,
            o.counts,
            o.ended_by,
            dir.join(format!("{name}.raw.tsv")).display()
        ),
        Err(e) => println!("{name}: FAILED {e:#}"),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let (dir, updates, minutes) = parse_args()?;
    tokio::fs::create_dir_all(&dir).await?;
    let window = Duration::from_secs(minutes * 60);
    let mut tasks = Vec::new();
    for feed in feeds() {
        let name = feed.name;
        tasks.push((
            name,
            tokio::spawn(capture(feed, dir.clone(), updates, window)),
        ));
    }
    for (name, task) in tasks {
        let result = task.await.map_err(anyhow::Error::from).and_then(|r| r);
        report(&dir, name, result);
    }
    Ok(())
}
