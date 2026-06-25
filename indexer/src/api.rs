//! The `/v1` REST surface (interface.md §5): one service returning raw
//! state + rollups; heavy derivation (APR, PnL, FX grouping) stays
//! client-side.

use crate::store::Store;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;

pub fn router(store: Store) -> Router {
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/fills", get(fills))
        .route("/v1/takes", get(takes))
        .route("/v1/markets", get(markets))
        .route("/v1/events", get(events))
        .with_state(store)
}

#[derive(Deserialize)]
struct ListQuery {
    market: Option<String>,
    kind: Option<String>,
    limit: Option<i64>,
}

fn clamp(limit: Option<i64>) -> i64 {
    limit.unwrap_or(100).clamp(1, 1000)
}

fn err(e: anyhow::Error) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn fills(
    State(s): State<Store>,
    Query(q): Query<ListQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let rows = s
        .recent_fills(q.market.as_deref(), clamp(q.limit))
        .await
        .map_err(err)?;
    Ok(Json(rows))
}

async fn takes(
    State(s): State<Store>,
    Query(q): Query<ListQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let rows = s
        .list_takes(q.market.as_deref(), clamp(q.limit))
        .await
        .map_err(err)?;
    Ok(Json(rows))
}

async fn markets(State(s): State<Store>) -> Result<impl IntoResponse, (StatusCode, String)> {
    let rows = s.list_markets().await.map_err(err)?;
    Ok(Json(rows))
}

async fn events(
    State(s): State<Store>,
    Query(q): Query<ListQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let rows = s
        .list_events(q.kind.as_deref(), q.market.as_deref(), clamp(q.limit))
        .await
        .map_err(err)?;
    Ok(Json(rows))
}
