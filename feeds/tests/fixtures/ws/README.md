# Public order-book frames

Raw WebSocket frames from the public, unauthenticated book feeds of Kraken
and Coinbase, captured 2026-11-25 from 00:53 UTC by
`cargo run -p dropset-feeds --example capture_ws_books` (see that file's
header). No API key was used. All four sockets ran at the same time, so the
two Coinbase files cover the same window.

Each line is `<receive unix micros>\t<frame>`, the frame text byte for byte
as received.

| File                                 | Feed                                                                   | Pairs                                              |
| ------------------------------------ | ---------------------------------------------------------------------- | -------------------------------------------------- |
| `kraken_book_depth10.tsv`            | Kraken WebSocket v2 `book`, depth 10                                   | `EURC/USD`, `EURC/EUR`, `TGBP/USD`                 |
| `kraken_book_depth100.tsv`           | Kraken WebSocket v2 `book`, depth 100                                  | same                                               |
| `coinbase_advanced_trade_level2.tsv` | Coinbase Advanced Trade `level2`, with `heartbeats` on the same socket | `EURC-USDC`, `TGBP-USDC`, `XSGD-USDC`, `AUDD-USDC` |
| `coinbase_exchange_level2_batch.tsv` | Coinbase Exchange `level2_batch`                                       | `EURC-USDC`, `TGBP-USDC`                           |

## How they were trimmed

The capture ran until every pair had at least 200 updates. To keep the repo
small, each file keeps, per pair, its acknowledgement, its snapshot and its
first 200 update messages, and ends at the last such frame. Status and
heartbeat frames in that span are kept. Frames are dropped whole and never
edited, so every pair's frames are an unbroken prefix of what it received and
replay from its snapshot.

One frame family was dropped entirely: the Kraken `instrument` snapshot and
its acknowledgement on the depth-10 socket. That snapshot lists every pair on
the venue and runs to about a megabyte. The precision fields it gave for the
three pairs were:

| Pair       | `price_precision` | `qty_precision` | `price_increment` |
| ---------- | ----------------- | --------------- | ----------------- |
| `EURC/EUR` | 4                 | 8               | 0.0001            |
| `EURC/USD` | 5                 | 8               | 0.00001           |
| `TGBP/USD` | 5                 | 8               | 0.00001           |

`tests/kraken_book_checksum.rs` replays both Kraken files and reproduces every
`checksum` in them.
