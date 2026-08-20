//! # web/routes.rs
//!
//! The axum routes for the Quilt Jetson web server.
//!
//! ## Endpoints
//!
//! - `GET /` — static UI
//! - `GET /api/sheet` — current sheet (JSON)
//! - `GET /api/cells` — list of cell ids
//! - `GET /api/cell/:id` — current value of a cell
//! - `POST /api/cell/:id` — set a cell's value (JSON body)
//! - `GET /api/cell/:id/history?limit=N` — recent history
//! - `GET /api/meta` — engine metadata
//! - `GET /ws` — WebSocket for live events
//!
//! ## Depends on
//!
//! - `axum` — the web framework.
//! - `crate::engine` — the engine.
//! - `crate::types` — `CellValue`, `CellDef`.
//!
//! ## Used by
//!
//! - `crate::web::serve` — assembles the router and binds it.

use std::sync::Arc;

use axum::extract::{Path, Query, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Router;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::broadcast;
use tracing::warn;

use crate::engine::SubscriptionEvent;
use crate::types::{CellDef, CellValue};
use crate::QuiltEngine;

/// The shared application state.
pub struct AppState {
    /// The Quilt engine.
    pub engine: Arc<QuiltEngine>,
    /// The current sheet (cached after load).
    pub sheet: RwLock<Option<crate::types::SheetDef>>,
}

impl AppState {
    /// Build new app state.
    pub fn new(engine: Arc<QuiltEngine>) -> Self {
        Self {
            engine,
            sheet: RwLock::new(None),
        }
    }
}

/// A handle to the web server, used for testing.
pub struct WebServer {
    state: Arc<AppState>,
    router: Router,
}

impl WebServer {
    /// Build a new web server.
    pub fn new(engine: Arc<QuiltEngine>) -> Self {
        let state = Arc::new(AppState::new(engine));
        let router = make_router(state.clone());
        Self { state, router }
    }

    /// Get the router (for testing with `axum::body::Body`).
    pub fn router(&self) -> &Router {
        &self.router
    }

    /// Get the state (for testing).
    pub fn state(&self) -> &AppState {
        &self.state
    }
}

/// Build the axum router.
pub fn make_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(root))
        .route("/api/meta", get(get_meta))
        .route("/api/sheet", get(get_sheet).post(post_sheet))
        .route("/api/cells", get(get_cells))
        .route("/api/cell/:id", get(get_cell).post(set_cell))
        .route("/api/cell/:id/history", get(get_cell_history))
        .route("/ws", get(ws_handler))
        .with_state(state)
}

/// `GET /`
async fn root() -> Response {
    match include_str!("static/index.html").to_string().into_response() {
        r => r,
    }
}

/// `GET /api/meta` — engine metadata.
async fn get_meta(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({
        "engine_id": state.engine.id(),
        "cell_count": state.engine.list_ids().len(),
        "store": state.engine.store().is_some(),
    }))
}

/// `GET /api/sheet` — the current sheet.
async fn get_sheet(State(state): State<Arc<AppState>>) -> Json<Value> {
    let sheet = state.sheet.read();
    match sheet.as_ref() {
        Some(s) => match serde_json::to_value(s) {
            Ok(v) => Json(v),
            Err(_) => Json(json!({"error": "could not serialize sheet"})),
        },
        None => Json(json!({"error": "no sheet loaded"})),
    }
}

/// `POST /api/sheet` — load a new sheet.
async fn post_sheet(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let yaml = match body.get("yaml").and_then(|v| v.as_str()) {
        Some(y) => y,
        None => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "expected {yaml: '...'}")),
            ))
        }
    };
    let sheet: crate::types::SheetDef = match serde_yaml::from_str(yaml) {
        Ok(s) => s,
        Err(e) => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("YAML parse: {e}")})),
            ))
        }
    };
    if let Err(e) = state.engine.load_sheet(sheet.clone()).await {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": e.to_string()}))));
    }
    *state.sheet.write() = Some(sheet);
    Ok(Json(json!({"ok": true, "cell_count": state.engine.list_ids().len()})))
}

/// `GET /api/cells` — list of cell ids.
async fn get_cells(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({"cells": state.engine.list_ids()}))
}

/// `GET /api/cell/:id` — current value.
async fn get_cell(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    match state
        .engine
        .get(&id, crate::types::CallerContext::default())
        .await
    {
        Ok(v) => match serde_json::to_value(&v) {
            Ok(v) => Ok(Json(v)),
            Err(_) => Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "could not serialize value"})),
            )),
        },
        Err(e) => Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": e.to_string()})),
        )),
    }
}

/// `POST /api/cell/:id` — set a value.
async fn set_cell(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let data = body.get("value").cloned().unwrap_or(Value::Null);
    match state
        .engine
        .set(&id, data, crate::types::CallerContext::default())
        .await
    {
        Ok(()) => Ok(Json(json!({"ok": true}))),
        Err(e) => Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": e.to_string()})),
        )),
    }
}

/// Query parameters for `/api/cell/:id/history`.
#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    /// The maximum number of history rows to return.
    pub limit: Option<u32>,
}

/// `GET /api/cell/:id/history` — recent history.
async fn get_cell_history(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(query): Query<HistoryQuery>,
) -> Json<Value> {
    let limit = query.limit.unwrap_or(100);
    let store = state.engine.store();
    match store {
        Some(s) => match s.history(&id, limit).await {
            Ok(rows) => Json(json!({"cell_id": id, "rows": rows})),
            Err(e) => Json(json!({"error": e.to_string()})),
        },
        None => Json(json!({"error": "no store configured"})),
    }
}

/// `GET /ws` — WebSocket for live events.
async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> Response {
    let rx = state.engine.subscribe_events();
    ws.on_upgrade(move |socket| ws_loop(socket, rx))
}

async fn ws_loop(
    mut socket: axum::extract::ws::WebSocket,
    mut rx: broadcast::Receiver<SubscriptionEvent>,
) {
    use futures::StreamExt;
    use tokio::io::AsyncWriteExt;
    while let Some(msg) = rx.next().await {
        let event = match msg {
            Ok(e) => e,
            Err(_) => continue,
        };
        let payload = match serde_json::to_string(&event) {
            Ok(s) => s,
            Err(_) => continue,
        };
        if socket
            .send(axum::extract::ws::Message::Text(payload))
            .await
            .is_err()
        {
            break;
        }
    }
    let _ = socket.close().await;
}

/// JSON response wrapper.
pub fn json_response<T: Serialize>(value: T) -> Response {
    Json(value).into_response()
}

/// A simple health-check response.
pub fn health_response() -> Response {
    Json(json!({"status": "ok"})).into_response()
}

/// JSON response from a `CellValue` (the typical GET /api/cell/:id
/// payload).
pub fn cell_value_response(v: &CellValue) -> Response {
    Json(v).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineConfig;
    use crate::types::{CellDef, CellKind, SheetDef};

    fn engine_with_sheet() -> Arc<QuiltEngine> {
        let engine = QuiltEngine::new("test", EngineConfig::default());
        let sheet = SheetDef {
            id: "s".into(),
            title: Some("Test".into()),
            description: None,
            version: None,
            axes: None,
            cells: vec![CellDef {
                id: "a".into(),
                kind: CellKind::Value,
                value: Some(json!(42)),
                ..Default::default()
            }],
        };
        // We can't await in a sync test, so spawn a task.
        let engine_clone = engine.clone();
        let sheet_clone = sheet.clone();
        let _ = tokio::runtime::Handle::current().spawn(async move {
            let _ = engine_clone.load_sheet(sheet_clone).await;
        });
        engine
    }

    #[test]
    fn make_router_compiles() {
        let engine = QuiltEngine::new("test", EngineConfig::default());
        let state = Arc::new(AppState::new(engine));
        let _r = make_router(state);
    }

    #[test]
    fn webserver_new() {
        let engine = QuiltEngine::new("test", EngineConfig::default());
        let _server = WebServer::new(engine);
    }

    #[test]
    fn health_response_status_ok() {
        let r = health_response();
        assert_eq!(r.status(), StatusCode::OK);
    }

    #[test]
    fn cell_value_response_status_ok() {
        let v = CellValue::ready(json!(42));
        let r = cell_value_response(&v);
        assert_eq!(r.status(), StatusCode::OK);
    }

    #[test]
    fn json_response_status_ok() {
        let r = json_response(json!({"a": 1}));
        assert_eq!(r.status(), StatusCode::OK);
    }

    #[test]
    fn appstate_new() {
        let engine = QuiltEngine::new("test", EngineConfig::default());
        let _state = AppState::new(engine);
    }

    #[test]
    fn engine_with_sheet_works() {
        // This is more of a smoke test; it spawns a task.
        let _ = engine_with_sheet();
    }
}
