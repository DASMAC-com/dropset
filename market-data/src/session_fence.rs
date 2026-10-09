//! The FX session fence, read: the imposed session state both consumers price
//! under.
//!
//! [`dropset_fair_value::FxSession`] is an **input** to the composition, never
//! something it works out, because feed liveness carries no information about
//! whether the FX market is trading (see that type). Migration 0015's
//! `fx_session_window` view is the authority that knows the calendar, with
//! Postgres as the one daylight-saving authority. This module is its reader,
//! shared by the maker and the fair-value estimator so the two cannot price
//! under different clocks — which is what they did while each kept a private
//! UTC bracket of its own.
//!
//! The migration's header says the view has no reader and that the maker still
//! decides from a local bracket. That was its staging note and is no longer
//! true; this module is the reader it anticipated, and the brackets are gone.
//! The header cannot be corrected in place — an applied migration is hashed
//! byte for byte — so the current claim lives here.
//!
//! # The answer is a span, not an instant
//!
//! The view answers for an interval, so a consumer caches the spans it read and
//! decides each tick from whichever one covers it ([`session_at`]). That has two
//! consequences, both deliberate:
//!
//! - **A failed read does not darken a span already held.** The span is the
//!   calendar's statement about that interval, and a database outage does not
//!   change the calendar. What an outage *can* do is leave the consumer holding
//!   nothing that covers the tick — at the next boundary, or from startup — and
//!   that is when the session becomes [`FxSession::Unknown`] and the consumer
//!   halts.
//! - **No covering span is `Unknown`, never `Closed`.** The view's horizon is
//!   bounded on purpose, so its running out is a real case; reporting it as a
//!   shut market would read a missing authority as an ordinary weekend, which is
//!   the alarm the third state exists to raise.
//!
//! [`FenceSpans`] carries the covering span **and** the next one, so a consumer
//! does not go dark for one poll at every close and reopen.

use anyhow::{bail, Result};
use async_trait::async_trait;
use dropset_fair_value::FxSession;
use dropset_feeds::{now_secs, Batch, Source};
use sqlx::{PgPool, Row};

/// One row of the fence: `session` holds over `[starts_at, ends_at)`, in epoch
/// seconds.
///
/// Half-open, as the view's COMMENT requires — each boundary instant is the
/// `ends_at` of one row and the `starts_at` of the next, so a closed interval
/// would match two rows there. `session` is only ever `Open` or `Closed`: the
/// view has no row meaning "unknown", which is expressed by the absence of one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FenceSpan {
    pub session: FxSession,
    pub starts_at: i64,
    pub ends_at: i64,
}

impl FenceSpan {
    /// Whether this span covers `t`, read half-open.
    pub fn covers(&self, t: i64) -> bool {
        self.starts_at <= t && t < self.ends_at
    }
}

/// The spans one read returned — the one covering the read instant and the one
/// after, or fewer near the horizon. An empty set is a successful read that
/// found nothing to cover the instant, and stays distinct from a failed read,
/// which yields no snapshot at all.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FenceSpans {
    pub spans: Vec<FenceSpan>,
}

/// The session at `at_unix` under the spans held: the covering span's state,
/// or [`FxSession::Unknown`] when none covers it.
///
/// The one place a consumer turns the fence into a session, so both consumers
/// fail closed identically — and the reason `Unknown` is producible at all.
pub fn session_at(held: &FenceSpans, at_unix: i64) -> FxSession {
    held.spans
        .iter()
        .find(|s| s.covers(at_unix))
        .map_or(FxSession::Unknown, |s| s.session)
}

/// The view's `state` column as a session.
///
/// Anything but the two words the view writes is an error rather than a
/// default. Mapped to `Unknown` it would be a fence outage that never says
/// why; mapped to either real state it would be a guess about whether the
/// market is trading.
fn parse_state(state: &str) -> Result<FxSession> {
    match state {
        "open" => Ok(FxSession::Open),
        "closed" => Ok(FxSession::Closed),
        other => bail!("fx_session_window returned an unknown state {other:?}"),
    }
}

/// Read the span covering `at_unix` and the one after it.
///
/// Runtime-typed and schema-unfenced, like the store's other readers: it reads
/// three columns of one view and asserts nothing else about the schema.
pub async fn read_spans(pool: &PgPool, at_unix: i64) -> Result<FenceSpans> {
    let rows = sqlx::query(include_str!("../queries/session_fence_spans.sql"))
        .bind(at_unix)
        .fetch_all(pool)
        .await?;
    let spans = rows
        .iter()
        .map(|r| {
            Ok(FenceSpan {
                session: parse_state(r.try_get("state")?)?,
                starts_at: r.try_get("starts_unix")?,
                ends_at: r.try_get("ends_unix")?,
            })
        })
        .collect::<Result<_>>()?;
    Ok(FenceSpans { spans })
}

/// Polls the fence for the maker, which takes every input as a background
/// [`Source`] drained once per tick rather than reading inline.
pub struct SessionFenceSource {
    name: String,
    pool: PgPool,
}

impl SessionFenceSource {
    pub fn new(name: impl Into<String>, pool: PgPool) -> Self {
        Self {
            name: name.into(),
            pool,
        }
    }
}

#[async_trait]
impl Source for SessionFenceSource {
    type Record = FenceSpans;

    fn name(&self) -> &str {
        &self.name
    }

    async fn next(&mut self) -> Result<Batch<Self::Record>> {
        // Always emit, even empty: an empty set replaces what the consumer
        // holds, so a horizon that has run out reaches it as `Unknown` instead
        // of being masked by the last span it cached.
        let spans = read_spans(&self.pool, now_secs()).await?;
        Ok(Batch::new(vec![spans]).with_caught_up(true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRI_CLOSE: i64 = 1_700_000_000;
    const SUN_REOPEN: i64 = FRI_CLOSE + 48 * 3_600;

    fn spans(list: &[FenceSpan]) -> FenceSpans {
        FenceSpans {
            spans: list.to_vec(),
        }
    }

    fn closed() -> FenceSpan {
        FenceSpan {
            session: FxSession::Closed,
            starts_at: FRI_CLOSE,
            ends_at: SUN_REOPEN,
        }
    }

    fn reopened() -> FenceSpan {
        FenceSpan {
            session: FxSession::Open,
            starts_at: SUN_REOPEN,
            ends_at: SUN_REOPEN + 120 * 3_600,
        }
    }

    /// Each boundary belongs to the span it starts, never to both — the
    /// half-open reading the view's COMMENT requires.
    #[test]
    fn a_boundary_instant_belongs_to_the_span_it_starts() {
        let held = spans(&[closed(), reopened()]);
        assert_eq!(session_at(&held, FRI_CLOSE), FxSession::Closed);
        assert_eq!(session_at(&held, SUN_REOPEN - 1), FxSession::Closed);
        assert_eq!(session_at(&held, SUN_REOPEN), FxSession::Open);
    }

    /// The second span is what carries a consumer across the boundary without
    /// a fresh read.
    #[test]
    fn the_next_span_carries_the_tick_across_a_boundary() {
        assert_eq!(
            session_at(&spans(&[closed()]), SUN_REOPEN),
            FxSession::Unknown,
            "with the covering span alone, the reopen is a fence outage"
        );
        assert_eq!(
            session_at(&spans(&[closed(), reopened()]), SUN_REOPEN),
            FxSession::Open
        );
    }

    /// Nothing covering the tick is `Unknown`, never a guess — whether the
    /// spans are absent, already over, or not yet begun.
    #[test]
    fn no_covering_span_is_unknown() {
        assert_eq!(
            session_at(&FenceSpans::default(), FRI_CLOSE),
            FxSession::Unknown
        );
        assert_eq!(
            session_at(&spans(&[closed()]), FRI_CLOSE - 1),
            FxSession::Unknown
        );
        assert_eq!(
            session_at(&spans(&[closed(), reopened()]), reopened().ends_at),
            FxSession::Unknown,
            "a held set that has run out halts rather than extrapolating"
        );
    }

    #[test]
    fn only_the_two_written_states_parse() {
        assert_eq!(parse_state("open").unwrap(), FxSession::Open);
        assert_eq!(parse_state("closed").unwrap(), FxSession::Closed);
        assert!(parse_state("Open").is_err());
        assert!(parse_state("unknown").is_err());
    }

    /// The statement binds exactly `$1`, and projects every name the decoder
    /// reads — an alias renamed on one side fails at `try_get`, on the price
    /// path. Scans the statement with its comment header stripped, as the
    /// store reader's twin test does and for the same reason.
    #[test]
    fn the_query_matches_its_bind_and_its_decoder() {
        let sql = include_str!("../queries/session_fence_spans.sql")
            .lines()
            .filter(|l| !l.trim_start().starts_with("--"))
            .collect::<Vec<_>>()
            .join("\n");

        let binds: Vec<&str> = sql
            .match_indices('$')
            .map(|(i, _)| &sql[i..i + 2])
            .collect();
        assert_eq!(binds, vec!["$1"], "placeholder set drifted from the bind");

        let select_list = sql
            .split("SELECT")
            .nth(1)
            .and_then(|rest| rest.split("FROM fx_session_window").next())
            .expect("the statement has a SELECT list");
        let projected: Vec<&str> = select_list
            .split(',')
            .filter_map(|item| {
                let item = item.trim();
                match item.rsplit_once(" AS ") {
                    Some((_, alias)) => Some(alias.trim()),
                    None => item.split_whitespace().next_back(),
                }
            })
            .collect();
        for column in ["state", "starts_unix", "ends_unix"] {
            assert!(
                projected.contains(&column),
                "the decoder reads `{column}` but the statement projects {projected:?}"
            );
        }
    }
}
