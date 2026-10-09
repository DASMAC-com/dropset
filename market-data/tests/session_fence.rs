//! The session fence reader against the real view.
//!
//! The unit tests pin how a consumer turns held spans into a session; these pin
//! that the reader actually gets those spans out of `fx_session_window` — the
//! epoch rendering, the half-open boundary, the two-row lookahead — and that the
//! instants move with US daylight saving, which is the reason the boundary lives
//! in Postgres at all. The anchors are one autumn week under EDT and one winter
//! week under EST, so a reader that rendered the instants in the wrong zone
//! fails one of them by an hour.
//!
//! Needs a Docker daemon, so `#[ignore]`d like the other store tests:
//!
//! ```sh
//! cargo test -p dropset-market-data -- --ignored
//! ```
//!
//! **A merge gate** — the Tests (Postgres) job's `--run-ignored all`
//! invocation selects `dropset-market-data`, so this runs in the merge queue.

mod common;

use common::start_pg;
use dropset_fair_value::FxSession;
use dropset_market_data::session_fence::{read_spans, session_at, FenceSpan};

/// Fri 2026-10-09 17:00 EDT — the close, at 21:00 UTC.
const EDT_CLOSE: i64 = 1_791_579_600;
/// Sun 2026-10-11 17:00 EDT — the reopen, at 21:00 UTC.
const EDT_REOPEN: i64 = 1_791_752_400;
/// Fri 2026-12-04 17:00 EST — the close, at 22:00 UTC.
const EST_CLOSE: i64 = 1_796_421_600;
/// Sun 2026-12-06 17:00 EST — the reopen, at 22:00 UTC.
const EST_REOPEN: i64 = 1_796_594_400;
/// 2037-01-01, past the view's generated horizon.
const PAST_HORIZON: i64 = 2_114_380_800;

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn the_close_is_read_half_open_with_the_next_span_behind_it() {
    let (_pg, pool) = start_pg().await;

    // One second before the close: the open span covers it, and the closed span
    // it hands over to is already held.
    let before = read_spans(&pool, EDT_CLOSE - 1)
        .await
        .expect("read the fence");
    assert_eq!(before.spans.len(), 2);
    assert_eq!(before.spans[0].session, FxSession::Open);
    assert_eq!(before.spans[0].ends_at, EDT_CLOSE);
    assert_eq!(
        before.spans[1],
        FenceSpan {
            session: FxSession::Closed,
            starts_at: EDT_CLOSE,
            ends_at: EDT_REOPEN,
        }
    );
    assert_eq!(session_at(&before, EDT_CLOSE - 1), FxSession::Open);
    assert_eq!(
        session_at(&before, EDT_CLOSE),
        FxSession::Closed,
        "the held lookahead answers the close without a second read"
    );

    // AT the close, the open span no longer covers it — half-open.
    let at = read_spans(&pool, EDT_CLOSE).await.expect("read the fence");
    assert_eq!(at.spans[0].session, FxSession::Closed);
    assert_eq!(at.spans[0].starts_at, EDT_CLOSE);
}

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn the_boundary_moves_with_daylight_saving() {
    let (_pg, pool) = start_pg().await;

    let winter = read_spans(&pool, EST_CLOSE).await.expect("read the fence");
    assert_eq!(
        winter.spans[0],
        FenceSpan {
            session: FxSession::Closed,
            starts_at: EST_CLOSE,
            ends_at: EST_REOPEN,
        },
        "17:00 ET is 22:00 UTC under EST, an hour later than under EDT"
    );
    assert_eq!(EST_CLOSE % 86_400, 22 * 3_600);
    assert_eq!(EDT_CLOSE % 86_400, 21 * 3_600);
}

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn past_the_horizon_nothing_covers_the_tick() {
    let (_pg, pool) = start_pg().await;

    // A successful read with nothing in it — not an error. The consumer turns
    // it into `Unknown` and halts, which is the view's intended failure.
    let held = read_spans(&pool, PAST_HORIZON)
        .await
        .expect("read the fence");
    assert!(held.spans.is_empty());
    assert_eq!(session_at(&held, PAST_HORIZON), FxSession::Unknown);
}
