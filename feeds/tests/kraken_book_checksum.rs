//! Replays the captured Kraken WebSocket v2 `book` frames
//! (`tests/fixtures/ws/`, see the README there) and reproduces every
//! `checksum` they carry, by the documented book-maintenance rules: apply a
//! message's levels in order, remove a level on zero quantity, truncate each
//! side to the subscribed depth, then CRC32 the top 10 levels per side.
//!
//! The capture settles two things the checksum guide leaves unstated, and this
//! test pins them: the wire numbers carry the trailing zeros the checksum
//! needs (so the digits come from the JSON text itself, never from a float or
//! the `instrument` precision fields), and the CRC32 is the common IEEE
//! variant (`crc32fast`, the zlib / PNG polynomial).

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::value::RawValue;

/// One price level as it is on the wire, the number text kept verbatim.
#[derive(Deserialize)]
struct Level<'a> {
    #[serde(borrow)]
    price: &'a RawValue,
    #[serde(borrow)]
    qty: &'a RawValue,
}

#[derive(Deserialize)]
struct BookData<'a> {
    symbol: String,
    #[serde(borrow, default)]
    bids: Vec<Level<'a>>,
    #[serde(borrow, default)]
    asks: Vec<Level<'a>>,
    checksum: u32,
}

/// Any frame, read only far enough to route it.
#[derive(Deserialize)]
struct Head {
    channel: Option<String>,
}

/// A `book` snapshot or update, parsed strictly.
#[derive(Deserialize)]
struct Frame<'a> {
    #[serde(rename = "type")]
    kind: String,
    #[serde(borrow)]
    data: Vec<BookData<'a>>,
}

/// A decimal's exact value as an integer at a fixed scale of 10^-18: prices
/// order exactly without a float, and a quantity tests zero exactly.
fn decimal_key(text: &str) -> u128 {
    let (whole, frac) = text.split_once('.').unwrap_or((text, ""));
    assert!(frac.len() <= 18, "{text} is finer than the key scale");
    let digits = format!("{whole}{frac:0<18}");
    digits
        .parse()
        .unwrap_or_else(|e| panic!("decimal {text}: {e}"))
}

/// One side of the book: price key → (price text, qty text).
type Side = BTreeMap<u128, (String, String)>;

#[derive(Default)]
struct Book {
    bids: Side,
    asks: Side,
}

fn apply(side: &mut Side, levels: &[Level<'_>]) {
    for level in levels {
        let (price, qty) = (level.price.get(), level.qty.get());
        let key = decimal_key(price);
        if decimal_key(qty) == 0 {
            side.remove(&key);
        } else {
            side.insert(key, (price.to_owned(), qty.to_owned()));
        }
    }
}

impl Book {
    fn update(&mut self, data: &BookData<'_>, depth: usize) {
        apply(&mut self.bids, &data.bids);
        apply(&mut self.asks, &data.asks);
        // Bids keep the highest `depth`, asks the lowest.
        while self.bids.len() > depth {
            self.bids.pop_first();
        }
        while self.asks.len() > depth {
            self.asks.pop_last();
        }
    }

    fn checksum(&self) -> u32 {
        fn digits(text: &str) -> String {
            text.replace('.', "").trim_start_matches('0').to_owned()
        }
        let mut input = String::new();
        let asks = self.asks.values().take(10);
        let bids = self.bids.values().rev().take(10);
        for (price, qty) in asks.chain(bids) {
            input.push_str(&digits(price));
            input.push_str(&digits(qty));
        }
        crc32fast::hash(input.as_bytes())
    }
}

/// Replays one fixture and returns how many checksums it verified.
fn replay(fixture: &str, depth: usize) -> usize {
    let mut books: BTreeMap<String, Book> = BTreeMap::new();
    let mut verified = 0;
    for (n, line) in fixture.lines().enumerate() {
        let (_, text) = line.split_once('\t').expect("a `<micros>\\t<frame>` line");
        // Route on the channel alone first: acknowledgement, status and
        // heartbeat frames are skipped here, and a `book` frame that fails
        // the strict parse below fails the test rather than vanishing.
        let head: Head = serde_json::from_str(text)
            .unwrap_or_else(|e| panic!("line {}: not a JSON object: {e}", n + 1));
        if head.channel.as_deref() != Some("book") {
            continue;
        }
        let frame: Frame<'_> = serde_json::from_str(text)
            .unwrap_or_else(|e| panic!("line {}: malformed `book` frame: {e}", n + 1));
        for data in &frame.data {
            let book = books.entry(data.symbol.clone()).or_default();
            if frame.kind == "snapshot" {
                *book = Book::default();
            }
            book.update(data, depth);
            assert_eq!(
                book.checksum(),
                data.checksum,
                "line {}: {} checksum mismatch",
                n + 1,
                data.symbol
            );
            verified += 1;
        }
    }
    verified
}

#[test]
fn kraken_depth10_fixture_reproduces_every_checksum() {
    let fixture = include_str!("fixtures/ws/kraken_book_depth10.tsv");
    assert!(replay(fixture, 10) > 100);
}

#[test]
fn kraken_depth100_fixture_reproduces_every_checksum() {
    let fixture = include_str!("fixtures/ws/kraken_book_depth100.tsv");
    assert!(replay(fixture, 100) > 100);
}

#[test]
fn checksum_guide_worked_example() {
    // The input string from Kraken's book-checksum-v2 guide, whose stated
    // result is 3310070434: confirms the CRC32 variant without the network.
    let input = concat!(
        "45285210000045286415457195345286615457110945289615456091145290215890660",
        "452918154553491452947445474945296135380000452975994554245299518772827",
        "452835100000004528341545820154528211000000045281010000000452803154592586",
        "452790799000045277633101034527753000000045277315460273745276615445238",
    );
    assert_eq!(crc32fast::hash(input.as_bytes()), 3_310_070_434);
}
