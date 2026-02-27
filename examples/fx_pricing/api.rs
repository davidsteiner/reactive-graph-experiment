use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};

use reactive_graph::EngineHandle;
use reactive_graph::engine_handle::RemoteSubgraphHandle;

use crate::nodes::build_trade_subgraph;
use crate::types::{CurrencyPair, TradeInfo, TradeRequest, TradeResponse};

pub struct AppState {
    engine: EngineHandle,
    trades: Mutex<HashMap<String, TradeRecord>>,
    redis_url: String,
}

struct TradeRecord {
    info: TradeInfo,
    _handle: RemoteSubgraphHandle,
}

impl AppState {
    pub fn new(engine: EngineHandle, redis_url: String) -> Self {
        AppState {
            engine,
            trades: Mutex::new(HashMap::new()),
            redis_url,
        }
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/trades", post(create_trade))
        .route("/trades", get(list_trades))
        .route("/trades/{id}", delete(remove_trade))
        .with_state(state)
}

async fn create_trade(
    State(state): State<Arc<AppState>>,
    Json(req): Json<TradeRequest>,
) -> Result<(StatusCode, Json<TradeResponse>), (StatusCode, String)> {
    let pair: CurrencyPair = req
        .pair
        .parse()
        .map_err(|e: String| (StatusCode::BAD_REQUEST, e))?;

    let trade_id = uuid::Uuid::new_v4().to_string();
    let client = req.client.clone();
    let spread_bps = req.spread_bps;
    let redis_url = state.redis_url.clone();
    let engine = state.engine.clone();
    let tid = trade_id.clone();

    // merge() blocks on mpsc reply, so run in spawn_blocking
    let handle = tokio::task::spawn_blocking(move || {
        let subgraph = build_trade_subgraph(tid, pair, client, spread_bps, redis_url);
        engine.merge(subgraph)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let info = TradeInfo {
        trade_id: trade_id.clone(),
        pair: pair.to_string(),
        client: req.client.clone(),
        spread_bps,
    };

    let response = TradeResponse {
        trade_id: trade_id.clone(),
        pair: pair.to_string(),
        client: req.client,
        spread_bps,
    };

    state.trades.lock().unwrap().insert(
        trade_id,
        TradeRecord {
            info,
            _handle: handle,
        },
    );

    Ok((StatusCode::CREATED, Json(response)))
}

async fn list_trades(State(state): State<Arc<AppState>>) -> Json<Vec<TradeInfo>> {
    let trades = state.trades.lock().unwrap();
    let list: Vec<TradeInfo> = trades.values().map(|r| r.info.clone()).collect();
    Json(list)
}

async fn remove_trade(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> StatusCode {
    let mut trades = state.trades.lock().unwrap();
    if trades.remove(&id).is_some() {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}
