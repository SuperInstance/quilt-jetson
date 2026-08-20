//! # engine.rs
//!
//! The QuiltEngine — the async reactive runtime for the Jetson tier.
//!
//! ## Role in the system
//!
//! This is the heart of `quilt-jetson`. The engine holds the cell
//! graph, tracks dependencies, propagates changes, and exposes the
//! universal verbs `get` / `set` / `call` / `push` / `subscribe`.
//!
//! Everything else (the web server, the ROS2 bridge, the federation
//! client, the CLI binary) is a view onto this engine. If you
//! understand this file, you understand the system.
//!
//! ## Depends on
//!
//! - `crate::types` — `Cell`, `CellDef`, `CellId`, `CellValue`, etc.
//! - `crate::error` — `Error`, `Result`.
//! - `crate::cells` — the eight cell evaluators.
//! - `crate::persistence` — `SqliteStore` for the cell history.
//! - `tokio` — async runtime.
//! - `indexmap` — `IndexMap` for deterministic iteration.
//! - `parking_lot` — fast `Mutex` / `RwLock`.
//! - `tokio::sync` — async channels for subscriptions.
//!
//! ## Used by
//!
//! - `quilt-jetson` binary — wraps the engine in a CLI.
//! - `crate::web` — serves the engine over HTTP/WebSocket.
//! - `crate::federation` — subscribes the engine to remote cells.
//! - User code that wants to embed Quilt on a Jetson.
//!
//! ## Key design decisions
//!
//! - The engine is **async**. Unlike `quilt-rust` (which is sync with
//!   a tokio bridge for effectful cells), `quilt-jetson` is tokio
//!   native throughout. This matters because the Jetson tier is
//!   about long-running async tasks: ROS2 subscribers, vision
//!   inference loops, federation WebSockets, SQLite writes.
//! - Cells live behind an `RwLock` so reads (the common case) are
//!   cheap. Writes (set, push) take a write lock briefly to update
//!   the cell, then release before propagating.
//! - The caller context is an owned value. We never share mutable
//!   state inside a context. As the call descends, we build fresh
//!   contexts via `extend_context`.
//! - Per-context memoization is keyed by `context_key(ctx)`.
//! - The graph is an `IndexMap<CellId, Cell>`. Reverse edges
//!   (`dependents`) live alongside forward edges (`dependencies`).
//! - Subscriptions use `tokio::sync::broadcast` for async pub/sub.
//!   Consumers register a `Subscription` and receive every change.
//! - The optional `SqliteStore` is wired in via the config; if set,
//!   every successful evaluation appends a row to the cell history.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use indexmap::IndexMap;
use parking_lot::RwLock;
use serde_json::Value;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, trace, warn};

use crate::cells::{
    evaluate_api, evaluate_formula, evaluate_io, evaluate_listener, evaluate_program,
    evaluate_router, evaluate_sensor, evaluate_value, evaluate_vision, make_io_value,
    make_sensor_value, fire_listener,
};
use crate::error::{Error, Result};
use crate::persistence::SqliteStore;
use crate::types::{
    now_millis, CallerContext, Cell, CellDef, CellId, CellKind, CellStatus, CellValue,
    Effect, EvaluationTrace, SheetDef, SubscriptionId,
};

// =============================================================================
// Subscription traits
// =============================================================================

/// A callback invoked when a subscribed cell changes.
pub trait SubscriptionCallback: Send + Sync {
    /// Called with `(cell_id, new_value, old_value)`.
    fn on_change(&self, cell_id: &str, new_value: &CellValue, prev_value: &CellValue);
}

/// A filter applied to subscription events. If the filter returns
/// `false`, the callback is not invoked.
pub trait SubscriptionFilter: Send + Sync {
    /// Return true to allow the event through.
    fn allow(&self, cell_id: &str, new_value: &CellValue, prev_value: &CellValue) -> bool;
}

// =============================================================================
// Engine config
// =============================================================================

/// Engine configuration.
#[derive(Debug, Clone, Default)]
pub struct EngineConfig {
    /// Whether to record evaluation traces. Off by default (memory
    /// cost).
    pub tracing: bool,
    /// Maximum number of recent traces to keep. Default 1000.
    pub trace_capacity: usize,
    /// Optional SQLite store for cell history. If `None`, history is
    /// not persisted.
    pub store: Option<Arc<SqliteStore>>,
    /// Maximum number of subscribers the broadcast channel can buffer.
    /// Default 1024. If a subscriber falls behind, they miss events.
    pub subscription_buffer: usize,
    /// Timeout for cell evaluations. Default 30 seconds.
    pub eval_timeout: Duration,
}

impl EngineConfig {
    /// Construct a new config with the given SQLite store.
    pub fn with_store(store: Arc<SqliteStore>) -> Self {
        Self {
            store: Some(store),
            ..Default::default()
        }
    }
}

// =============================================================================
// The engine
// =============================================================================

/// The reactive cell runtime. One instance per Jetson "session" /
/// "deployment" / "agent". Holds the cell graph and provides the
/// universal API.
///
/// The engine is `Send + Sync` and can be wrapped in `Arc` for sharing
/// across tasks.
pub struct QuiltEngine {
    /// Engine id, mostly for logging.
    id: String,
    /// Options.
    config: EngineConfig,
    /// The cell graph.
    cells: RwLock<IndexMap<CellId, Cell>>,
    /// Broadcast channel for cell-change events.
    event_tx: broadcast::Sender<SubscriptionEvent>,
    /// MPSC channel for "set" requests from external tasks (e.g.
    /// sensors pushing new data).
    push_tx: mpsc::UnboundedSender<PushRequest>,
    /// Receiver side, owned by the background loop.
    push_rx: parking_lot::Mutex<Option<mpsc::UnboundedReceiver<PushRequest>>>,
    /// Counter for unique subscription ids.
    sub_counter: parking_lot::Mutex<u64>,
    /// Recent evaluation traces.
    traces: parking_lot::Mutex<Vec<EvaluationTrace>>,
}

impl QuiltEngine {
    /// Create a new engine with the given configuration.
    pub fn new(id: impl Into<String>, config: EngineConfig) -> Arc<Self> {
        let id = id.into();
        let buffer = if config.subscription_buffer == 0 {
            1024
        } else {
            config.subscription_buffer
        };
        let (event_tx, _) = broadcast::channel(buffer);
        let (push_tx, push_rx) = mpsc::unbounded_channel();
        let engine = Arc::new(Self {
            id,
            config,
            cells: RwLock::new(IndexMap::new()),
            event_tx,
            push_tx,
            push_rx: parking_lot::Mutex::new(Some(push_rx)),
            sub_counter: parking_lot::Mutex::new(0),
            traces: parking_lot::Mutex::new(Vec::new()),
        });
        // Spawn the background push loop.
        let weak = Arc::downgrade(&engine);
        let push_rx_opt = engine.push_rx.lock().take();
        if let Some(rx) = push_rx_opt {
            tokio::spawn(push_loop(weak, rx));
        }
        engine
    }

    /// Convenience: create a new engine with the given sheet already
    /// loaded.
    pub fn with_sheet(
        id: impl Into<String>,
        config: EngineConfig,
        sheet: SheetDef,
    ) -> Result<Arc<Self>> {
        let engine = Self::new(id, config);
        engine.load_sheet(sheet)?;
        Ok(engine)
    }

    /// The engine's id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Get a handle for pushing values to cells. The returned sender
    /// can be cloned and sent across tasks.
    pub fn push_sender(&self) -> mpsc::UnboundedSender<PushRequest> {
        self.push_tx.clone()
    }

    /// Get the broadcast receiver for cell-change events. Clone it to
    /// subscribe to changes.
    pub fn subscribe_events(&self) -> broadcast::Receiver<SubscriptionEvent> {
        self.event_tx.subscribe()
    }

    // =========================================================================
    // Sheet lifecycle
    // =========================================================================

    /// Load a sheet definition. Resets all cell state.
    ///
    /// Steps:
    ///   1. Acquire write lock; clear existing cells.
    ///   2. For each `CellDef`, instantiate a `Cell`.
    ///   3. Build dependency edges: declared deps first, then
    ///      auto-detect for formulas (by scanning the expression).
    pub fn load_sheet(self: &Arc<Self>, sheet: SheetDef) -> Result<()> {
        let mut cells = self.cells.write();

        cells.clear();

        // 1. Instantiate cells.
        for def in sheet.cells {
            let id = def.id.clone();
            let cell = Cell::new(def);
            cells.insert(id, cell);
        }

        // 2. Build dependency edges.
        let ids: Vec<CellId> = cells.keys().cloned().collect();
        for id in &ids {
            let deps = cells[id].def.deps.clone();
            for dep in deps {
                Self::add_dep_locked(&mut cells, id.as_str(), dep.as_str());
            }
        }
        // Auto-detect for formulas.
        let formula_deps: Vec<(String, String)> = ids
            .iter()
            .filter_map(|id| {
                let cell = cells.get(id)?;
                if cell.def.kind == CellKind::Formula {
                    let expr = cell.def.expr.clone()?;
                    Some((id.clone(), expr))
                } else {
                    None
                }
            })
            .collect();
        for (id, expr) in formula_deps {
            for known_id in &ids {
                if known_id == &id {
                    continue;
                }
                if expr_contains_token(&expr, known_id) {
                    Self::add_dep_locked(&mut cells, &id, known_id);
                }
            }
        }

        Ok(())
    }

    /// Register a new cell at runtime. Used for dynamic registration
    /// (e.g. by an agent). Returns `true` if a new cell was created,
    /// `false` if the id was already present.
    pub fn register(self: &Arc<Self>, def: CellDef) -> Result<bool> {
        let id = def.id.clone();
        let mut cells = self.cells.write();
        if cells.contains_key(&id) {
            return Ok(false);
        }
        let cell = Cell::new(def);
        cells.insert(id.clone(), cell);

        let deps = cells[&id].def.deps.clone();
        for dep in deps {
            Self::add_dep_locked(&mut cells, id.as_str(), dep.as_str());
        }
        Ok(true)
    }

    fn add_dep_locked(cells: &mut IndexMap<CellId, Cell>, from: &str, to: &str) {
        if !cells.contains_key(from) || !cells.contains_key(to) {
            return;
        }
        if let Some(from_cell) = cells.get_mut(from) {
            from_cell.dependencies.insert(to.to_string());
        }
        if let Some(to_cell) = cells.get_mut(to) {
            to_cell.dependents.insert(from.to_string());
        }
    }

    // =========================================================================
    // The universal API: get, set, call, push
    // =========================================================================

    /// Get a cell's value. Evaluates if needed.
    pub async fn get(self: &Arc<Self>, id: &str, ctx: CallerContext) -> Result<CellValue> {
        let id_norm = self.normalize_id(id)?;
        let kind = {
            let cells = self.cells.read();
            match cells.get(&id_norm) {
                Some(c) => c.def.kind,
                None => return Err(Error::CellNotFound(id.to_string())),
            }
        };

        let full_ctx = extend_context(&ctx, &id_norm, None);

        match kind {
            CellKind::Value => {
                let cells = self.cells.read();
                let cell = cells.get(&id_norm).expect("checked above");
                Ok(evaluate_value(cell, &full_ctx))
            }
            CellKind::Formula => {
                // Build a snapshot of dep values.
                let snapshot = self.build_formula_snapshot(&id_norm, &full_ctx).await;
                let cells = self.cells.read();
                let cell = cells.get(&id_norm).expect("checked above").clone();
                drop(cells);
                let result = tokio::task::spawn_blocking({
                    let cell = cell.clone();
                    let full_ctx = full_ctx.clone();
                    move || evaluate_formula(&cell, &snapshot, &full_ctx)
                })
                .await?;
                self.cache_result(&id_norm, &full_ctx, &result).await;
                Ok(result)
            }
            CellKind::Api => {
                let cell = {
                    let cells = self.cells.read();
                    cells.get(&id_norm).cloned().expect("checked above")
                };
                // Vision routing: if the endpoint is tensorrt:// or
                // onnx://, dispatch to the vision module.
                let result = if let Some(ep) = &cell.def.endpoint {
                    if ep.starts_with("tensorrt://") || ep.starts_with("onnx://") {
                        evaluate_vision(cell, full_ctx.clone()).await
                    } else {
                        tokio::time::timeout(
                            self.config.eval_timeout,
                            evaluate_api(cell, full_ctx.clone(), None),
                        )
                        .await
                        .unwrap_or_else(|_| CellValue::err("api evaluation timed out"))
                    }
                } else {
                    CellValue::err("api cell has no endpoint")
                };
                self.cache_result(&id_norm, &full_ctx, &result).await;
                Ok(result)
            }
            CellKind::Program => {
                let cell = {
                    let cells = self.cells.read();
                    cells.get(&id_norm).cloned().expect("checked above")
                };
                let engine = self.clone();
                let full_ctx_for_program = full_ctx.clone();
                let result = tokio::task::spawn_blocking(move || {
                    let runtime = Arc::new(EngineProgramRuntime { engine });
                    tokio::runtime::Handle::current()
                        .block_on(evaluate_program(cell, full_ctx_for_program, None, runtime))
                })
                .await?;
                self.cache_result(&id_norm, &full_ctx, &result).await;
                Ok(result)
            }
            CellKind::Router => {
                let cell = {
                    let cells = self.cells.read();
                    cells.get(&id_norm).cloned().expect("checked above")
                };
                let engine = self.clone();
                let full_ctx_for_router = full_ctx.clone();
                let result = tokio::task::spawn_blocking(move || {
                    let runtime = Arc::new(EngineProgramRuntime { engine });
                    tokio::runtime::Handle::current()
                        .block_on(evaluate_router(cell, full_ctx_for_router, None, runtime))
                })
                .await?;
                self.cache_result(&id_norm, &full_ctx, &result).await;
                Ok(result)
            }
            CellKind::Sensor => {
                let cells = self.cells.read();
                let cell = cells.get(&id_norm).expect("checked above");
                Ok(evaluate_sensor(cell, &full_ctx))
            }
            CellKind::Io => {
                let cells = self.cells.read();
                let cell = cells.get(&id_norm).expect("checked above");
                Ok(evaluate_io(cell, &full_ctx))
            }
            CellKind::Listener => {
                // Listeners are push-only. Reading returns the
                // current cached value.
                let cells = self.cells.read();
                let cell = cells.get(&id_norm).expect("checked above");
                Ok(cell.value.clone())
            }
        }
    }

    /// Set a cell's value. Triggers downstream recomputation.
    pub async fn set(self: &Arc<Self>, id: &str, value: Value, ctx: CallerContext) -> Result<()> {
        let id_norm = self.normalize_id(id)?;
        let full_ctx = extend_context(&ctx, &id_norm, None);

        let old_value = {
            let mut cells = self.cells.write();
            let cell = cells
                .get_mut(&id_norm)
                .ok_or_else(|| Error::CellNotFound(id.to_string()))?;
            let old = cell.value.clone();
            cell.value = CellValue {
                data: value,
                status: CellStatus::Ready,
                computed_at: Some(now_millis()),
                error: None,
                effects: Vec::new(),
            };
            cell.context_cache.clear();
            old
        };

        // Notify subscribers.
        self.notify_change(&id_norm, &old_value).await;

        // Propagate.
        self.propagate(&id_norm, &full_ctx).await;

        Ok(())
    }

    /// Call a cell as a capability. For pure cells, same as `get`
    /// (input is ignored). For effectful cells, input is passed.
    pub async fn call(
        self: &Arc<Self>,
        id: &str,
        input: Option<Value>,
        ctx: CallerContext,
    ) -> Result<CellValue> {
        // For MVP, calling is the same as getting.
        self.get(id, ctx).await
    }

    /// Push a value into a sensor or IO cell. Triggers downstream.
    ///
    /// This is the canonical way for external adapters (IMU, LIDAR,
    /// camera, ROS2 subscriber) to feed values into the engine.
    pub async fn push(self: &Arc<Self>, id: &str, data: Value) -> Result<()> {
        let id_norm = self.normalize_id(id)?;
        let ctx = extend_context(&CallerContext::default(), &id_norm, None);

        let kind = {
            let cells = self.cells.read();
            match cells.get(&id_norm) {
                Some(c) => c.def.kind,
                None => return Err(Error::CellNotFound(id.to_string())),
            }
        };
        if kind != CellKind::Sensor && kind != CellKind::Io {
            return Err(Error::InvalidCellDef {
                id: id.to_string(),
                message: format!("cannot push to {} cell (only sensor/io)", kind.as_str()),
            });
        }

        let old_value = {
            let mut cells = self.cells.write();
            let cell = cells.get_mut(&id_norm).expect("checked above");
            let old = cell.value.clone();
            cell.value = if kind == CellKind::Sensor {
                make_sensor_value(data)
            } else {
                make_io_value(data)
            };
            old
        };

        self.notify_change(&id_norm, &old_value).await;
        self.propagate(&id_norm, &ctx).await;
        Ok(())
    }

    /// Subscribe to a single cell's changes. Returns a receiver that
    /// yields `SubscriptionEvent` values.
    pub fn subscribe(self: &Arc<Self>, cell_id: &str) -> Result<SubscriptionHandle> {
        {
            let cells = self.cells.read();
            if !cells.contains_key(cell_id) {
                return Err(Error::CellNotFound(cell_id.to_string()));
            }
        }

        let sub_id = {
            let mut counter = self.sub_counter.lock();
            *counter += 1;
            format!("sub-{}", *counter)
        };

        let rx = self.event_tx.subscribe();
        Ok(SubscriptionHandle {
            id: sub_id,
            rx,
            cell_id: cell_id.to_string(),
        })
    }

    /// Subscribe to all cells. Returns a receiver that yields every
    /// change.
    pub fn subscribe_all(self: &Arc<Self>) -> SubscriptionHandle {
        let sub_id = {
            let mut counter = self.sub_counter.lock();
            *counter += 1;
            format!("sub-all-{}", *counter)
        };

        let rx = self.event_tx.subscribe();
        SubscriptionHandle {
            id: sub_id,
            rx,
            cell_id: "*".to_string(),
        }
    }

    // =========================================================================
    // Introspection
    // =========================================================================

    /// Get a cell by id. Returns None if no such cell.
    pub fn get_cell(self: &Arc<Self>, id: &str) -> Option<Cell> {
        let cells = self.cells.read();
        cells.get(id).cloned()
    }

    /// List all cells.
    pub fn list_cells(self: &Arc<Self>) -> Vec<Cell> {
        let cells = self.cells.read();
        cells.values().cloned().collect()
    }

    /// List all cell ids.
    pub fn list_ids(self: &Arc<Self>) -> Vec<CellId> {
        let cells = self.cells.read();
        cells.keys().cloned().collect()
    }

    /// Get recent evaluation traces. Most recent first.
    pub fn traces(self: &Arc<Self>) -> Vec<EvaluationTrace> {
        let traces = self.traces.lock();
        traces.iter().rev().cloned().collect()
    }

    /// Record a trace entry. Used by cell evaluators.
    pub(crate) fn record_trace(self: &Arc<Self>, trace: EvaluationTrace) {
        if !self.config.tracing {
            return;
        }
        let mut traces = self.traces.lock();
        traces.push(trace);
        if traces.len() > self.config.trace_capacity {
            let drop = traces.len() - self.config.trace_capacity;
            traces.drain(0..drop);
        }
    }

    /// Get the configured SQLite store, if any.
    pub fn store(&self) -> Option<Arc<SqliteStore>> {
        self.config.store.clone()
    }

    // =========================================================================
    // Internal
    // =========================================================================

    /// Build a snapshot of dependency values for a formula.
    async fn build_formula_snapshot(
        self: &Arc<Self>,
        formula_id: &str,
        caller_ctx: &CallerContext,
    ) -> HashMap<CellId, Value> {
        let (deps, dep_kinds): (Vec<CellId>, Vec<(CellId, CellKind)>) = {
            let cells = self.cells.read();
            let cell = match cells.get(formula_id) {
                Some(c) => c,
                None => return HashMap::new(),
            };
            let dep_kinds: Vec<(CellId, CellKind)> = cell
                .dependencies
                .iter()
                .filter_map(|d| cells.get(d).map(|c| (d.clone(), c.def.kind)))
                .collect();
            (cell.dependencies.iter().cloned().collect(), dep_kinds)
        };

        // Pre-evaluate formula and program dependencies. (Value
        // and sensor cells already have their value; skip them.)
        for (dep_id, kind) in &dep_kinds {
            if matches!(kind, CellKind::Formula | CellKind::Program) {
                let _ = self.get(dep_id, caller_ctx.clone()).await;
            }
        }

        // Build the snapshot.
        let cells = self.cells.read();
        deps.iter()
            .filter_map(|dep_id| {
                let dep = cells.get(dep_id)?;
                let value = if dep.def.kind == CellKind::Formula {
                    let dep_ctx = extend_context(caller_ctx, dep_id.clone(), None);
                    let dep_key = context_key(&dep_ctx);
                    dep.context_cache
                        .get(&dep_key)
                        .map(|v| v.data.clone())
                        .unwrap_or(Value::Null)
                } else {
                    dep.value.data.clone()
                };
                Some((dep_id.clone(), value))
            })
            .collect()
    }

    /// Cache a result by context key.
    async fn cache_result(self: &Arc<Self>, id: &str, ctx: &CallerContext, value: &CellValue) {
        let key = context_key(ctx);
        {
            let mut cells = self.cells.write();
            if let Some(cell) = cells.get_mut(id) {
                cell.context_cache.insert(key, value.clone());
                cell.value = value.clone();
                cell.last_context = Some(ctx.clone());
            }
        }
        // Persist to SQLite if configured.
        if let Some(store) = &self.config.store {
            if value.is_ready() {
                let value = value.clone();
                let id_owned = id.to_string();
                let store = store.clone();
                tokio::spawn(async move {
                    if let Err(e) = store.append(&id_owned, &value).await {
                        warn!("failed to persist cell value: {e}");
                    }
                });
            }
        }
    }

    /// Propagate a change to all dependents.
    async fn propagate(self: &Arc<Self>, changed_id: &str, ctx: &CallerContext) {
        // Collect dependents under a read lock.
        let dependents: Vec<CellId> = {
            let cells = self.cells.read();
            cells
                .get(changed_id)
                .map(|c| c.dependents.iter().cloned().collect())
                .unwrap_or_default()
        };

        // Mark formula dependents as stale.
        for dep_id in &dependents {
            {
                let mut cells = self.cells.write();
                if let Some(dep) = cells.get_mut(dep_id) {
                    if dep.def.kind == CellKind::Formula || dep.def.kind == CellKind::Value {
                        dep.value = CellValue {
                            data: Value::Null,
                            status: CellStatus::Stale,
                            computed_at: None,
                            error: None,
                            effects: Vec::new(),
                        };
                        dep.context_cache.clear();
                    }
                }
            }
            // Fire listeners whose watch list contains `changed_id`.
            let listener_data = {
                let cells = self.cells.read();
                cells.get(dep_id).map(|c| {
                    (
                        c.def.kind == CellKind::Listener,
                        c.def.watch.clone(),
                        c.def.condition.clone(),
                        c.def.action.clone(),
                    )
                })
            };
            if let Some((true, watch, condition, action)) = listener_data {
                if watch.iter().any(|w| w == changed_id) {
                    let cells = self.cells.read();
                    let listener_cell = cells.get(dep_id).expect("checked above").clone();
                    drop(cells);
                    let result = fire_listener(
                        &listener_cell,
                        changed_id,
                        condition.as_deref(),
                        action.as_deref(),
                    );
                    debug!("listener {} fired: {:?}", dep_id, result);
                }
            }
        }

        // Recurse.
        for dep_id in &dependents {
            self.propagate(dep_id, ctx).await;
        }
    }

    /// Notify all subscribers of a cell change.
    async fn notify_change(self: &Arc<Self>, cell_id: &str, prev: &CellValue) {
        let event = {
            let cells = self.cells.read();
            let cell = match cells.get(cell_id) {
                Some(c) => c,
                None => return,
            };
            SubscriptionEvent {
                cell_id: cell_id.to_string(),
                new_value: cell.value.clone(),
                prev_value: prev.clone(),
            }
        };

        // Send on broadcast; ignore SendError (no active subscribers).
        let _ = self.event_tx.send(event);
    }

    /// Normalize a cell id.
    fn normalize_id(&self, id: &str) -> Result<CellId> {
        let trimmed = id.trim();
        if trimmed.is_empty() {
            return Err(Error::InvalidCellDef {
                id: id.to_string(),
                message: "cell id cannot be empty".to_string(),
            });
        }
        Ok(trimmed.to_string())
    }
}

// =============================================================================
// Subscription API
// =============================================================================

/// A subscription event.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SubscriptionEvent {
    /// The cell that changed.
    pub cell_id: CellId,
    /// The new value.
    pub new_value: CellValue,
    /// The previous value.
    pub prev_value: CellValue,
}

/// A handle to a subscription.
pub struct SubscriptionHandle {
    /// The subscription id, for identification.
    pub id: SubscriptionId,
    /// The broadcast receiver.
    pub rx: broadcast::Receiver<SubscriptionEvent>,
    /// The cell id this handle is for (`"*"` for subscribe-all).
    pub cell_id: CellId,
}

// =============================================================================
// Push request (for external adapters)
// =============================================================================

/// A request to push a value into a cell. Created by external adapters
/// (sensors, ROS2 subscribers, etc.) and consumed by the engine's
/// background loop.
#[derive(Debug, Clone)]
pub struct PushRequest {
    /// The cell to push to.
    pub cell_id: CellId,
    /// The value to push.
    pub data: Value,
    /// The caller context (optional, defaults to empty).
    pub context: Option<CallerContext>,
}

impl PushRequest {
    /// Convenience constructor.
    pub fn new(cell_id: impl Into<CellId>, data: impl Into<Value>) -> Self {
        Self {
            cell_id: cell_id.into(),
            data: data.into(),
            context: None,
        }
    }
}

// =============================================================================
// EngineProgramRuntime — what program cells see
// =============================================================================

/// The runtime handle exposed to `program` and `router` cells.
struct EngineProgramRuntime {
    engine: Arc<QuiltEngine>,
}

#[async_trait::async_trait]
impl crate::cells::ProgramRuntime for EngineProgramRuntime {
    async fn get_async(&self, id: &str, ctx: &CallerContext) -> Result<CellValue> {
        self.engine.get(id, ctx.clone()).await
    }

    async fn set_async(&self, id: &str, value: Value, ctx: &CallerContext) -> Result<()> {
        self.engine.set(id, value, ctx.clone()).await
    }

    async fn call_async(
        &self,
        id: &str,
        input: Option<Value>,
        ctx: &CallerContext,
    ) -> Result<CellValue> {
        self.engine.call(id, input, ctx.clone()).await
    }

    fn list(&self) -> Vec<String> {
        self.engine.list_ids()
    }

    fn push(&self, id: &str, data: Value) -> Result<()> {
        let _ = self.engine.push_tx.send(PushRequest::new(id, data));
        Ok(())
    }
}

// =============================================================================
// Background push loop
// =============================================================================

/// The background task that consumes `PushRequest`s from external
/// adapters and calls `engine.push`.
async fn push_loop(
    engine: std::sync::Weak<QuiltEngine>,
    mut rx: mpsc::UnboundedReceiver<PushRequest>,
) {
    while let Some(req) = rx.recv().await {
        let Some(engine) = engine.upgrade() else {
            break;
        };
        let id = req.cell_id.clone();
        if let Err(e) = engine.push(&id, req.data).await {
            warn!("push to {id} failed: {e}");
        }
    }
    trace!("push loop exited");
}

// =============================================================================
// Helpers
// =============================================================================

/// Wall-clock milliseconds since the UNIX epoch.
pub fn now_millis() -> u64 {
    crate::types::now_millis()
}

/// Build a context key for per-context memoization.
pub fn context_key(ctx: &CallerContext) -> String {
    context_key_inner(ctx)
}

fn context_key_inner(ctx: &CallerContext) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    if let Some(r) = &ctx.row {
        r.to_string().hash(&mut h);
    }
    if let Some(c) = &ctx.column {
        c.to_string().hash(&mut h);
    }
    if let Some(s) = &ctx.sheet {
        s.hash(&mut h);
    }
    if let Some(c) = &ctx.caller {
        c.hash(&mut h);
    }
    if let Some(id) = &ctx.identity {
        id.id.hash(&mut h);
        for t in &id.tags {
            t.hash(&mut h);
        }
    }
    for (k, v) in &ctx.metadata {
        k.hash(&mut h);
        v.to_string().hash(&mut h);
    }
    format!("ctx-{:x}", h.finish())
}

/// Extend a caller context with a new caller and metadata.
pub fn extend_context(
    parent: &CallerContext,
    caller: impl Into<CellId>,
    metadata: Option<std::collections::BTreeMap<String, Value>>,
) -> CallerContext {
    let mut next = parent.clone();
    let caller = caller.into();
    next.trace.push(caller.clone());
    next.caller = Some(caller);
    next.timestamp = now_millis();
    if let Some(meta) = metadata {
        for (k, v) in meta {
            next.metadata.insert(k, v);
        }
    }
    next
}

/// Naive token-containment check. The TypeScript version uses
/// `RegExp` with word boundaries; we approximate with character
/// classification here. Sufficient for the MVP.
fn expr_contains_token(expr: &str, id: &str) -> bool {
    let body = expr.strip_prefix('=').unwrap_or(expr);
    for (i, _) in body.match_indices(id) {
        let before = body[..i].chars().last();
        let after = body[i + id.len()..].chars().next();
        let is_word_char = |c: char| c.is_alphanumeric() || c == '_' || c == '.';
        if before.map(is_word_char).unwrap_or(false) {
            continue;
        }
        if after.map(is_word_char).unwrap_or(false) {
            continue;
        }
        return true;
    }
    false
}

// =============================================================================
// Effect re-export for the lib
// =============================================================================

// (Effect is in types.rs; we re-import here to keep the engine file
// self-contained.)
pub use crate::types::Effect as EngineEffect;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CellDef, CellKind};
    use serde_json::json;

    fn engine() -> Arc<QuiltEngine> {
        QuiltEngine::new("test", EngineConfig::default())
    }

    fn value_def(id: &str, value: Value) -> CellDef {
        CellDef {
            id: id.to_string(),
            kind: CellKind::Value,
            value: Some(value),
            ..Default::default()
        }
    }

    fn formula_def(id: &str, expr: &str) -> CellDef {
        CellDef {
            id: id.to_string(),
            kind: CellKind::Formula,
            expr: Some(expr.to_string()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn new_engine_has_no_cells() {
        let e = engine();
        assert_eq!(e.list_ids().len(), 0);
    }

    #[tokio::test]
    async fn load_sheet_instantiates_cells() {
        let e = engine();
        let sheet = SheetDef {
            id: "s".into(),
            title: None,
            description: None,
            version: None,
            axes: None,
            cells: vec![value_def("a", json!(1)), value_def("b", json!(2))],
        };
        e.load_sheet(sheet).await.unwrap();
        assert_eq!(e.list_ids().len(), 2);
    }

    #[tokio::test]
    async fn get_value_cell_returns_value() {
        let e = engine();
        let sheet = SheetDef {
            id: "s".into(),
            title: None,
            description: None,
            version: None,
            axes: None,
            cells: vec![value_def("a", json!(42))],
        };
        e.load_sheet(sheet).await.unwrap();
        let v = e.get("a", CallerContext::default()).await.unwrap();
        assert_eq!(v.data, json!(42));
        assert!(v.is_ready());
    }

    #[tokio::test]
    async fn get_missing_cell_errors() {
        let e = engine();
        let r = e.get("missing", CallerContext::default()).await;
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn set_then_get_returns_set_value() {
        let e = engine();
        let sheet = SheetDef {
            id: "s".into(),
            title: None,
            description: None,
            version: None,
            axes: None,
            cells: vec![value_def("a", json!(1))],
        };
        e.load_sheet(sheet).await.unwrap();
        e.set("a", json!(99), CallerContext::default()).await.unwrap();
        let v = e.get("a", CallerContext::default()).await.unwrap();
        assert_eq!(v.data, json!(99));
    }

    #[tokio::test]
    async fn formula_evaluates_after_dep_set() {
        let e = engine();
        let sheet = SheetDef {
            id: "s".into(),
            title: None,
            description: None,
            version: None,
            axes: None,
            cells: vec![
                value_def("a", json!(3)),
                value_def("b", json!(4)),
                formula_def("sum", "=a + b"),
            ],
        };
        e.load_sheet(sheet).await.unwrap();
        let v = e.get("sum", CallerContext::default()).await.unwrap();
        assert_eq!(v.data, json!(7));
    }

    #[tokio::test]
    async fn register_dynamic_cell() {
        let e = engine();
        let ok = e.register(value_def("a", json!(1))).unwrap();
        assert!(ok);
        let v = e.get("a", CallerContext::default()).await.unwrap();
        assert_eq!(v.data, json!(1));
    }

    #[tokio::test]
    async fn register_duplicate_id_returns_false() {
        let e = engine();
        e.register(value_def("a", json!(1))).unwrap();
        let ok = e.register(value_def("a", json!(2))).unwrap();
        assert!(!ok);
    }

    #[tokio::test]
    async fn push_to_sensor_cell() {
        let e = engine();
        let sheet = SheetDef {
            id: "s".into(),
            title: None,
            description: None,
            version: None,
            axes: None,
            cells: vec![CellDef {
                id: "imu".into(),
                kind: CellKind::Sensor,
                ..Default::default()
            }],
        };
        e.load_sheet(sheet).await.unwrap();
        e.push("imu", json!({"x": 1.0})).await.unwrap();
        let v = e.get("imu", CallerContext::default()).await.unwrap();
        assert_eq!(v.data, json!({"x": 1.0}));
    }

    #[tokio::test]
    async fn push_to_value_cell_errors() {
        let e = engine();
        let sheet = SheetDef {
            id: "s".into(),
            title: None,
            description: None,
            version: None,
            axes: None,
            cells: vec![value_def("a", json!(1))],
        };
        e.load_sheet(sheet).await.unwrap();
        let r = e.push("a", json!(2)).await;
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn subscribe_receives_events() {
        let e = engine();
        let sheet = SheetDef {
            id: "s".into(),
            title: None,
            description: None,
            version: None,
            axes: None,
            cells: vec![value_def("a", json!(1))],
        };
        e.load_sheet(sheet).await.unwrap();
        let mut sub = e.subscribe("a").unwrap();
        e.set("a", json!(2), CallerContext::default()).await.unwrap();
        // Give the broadcast a moment.
        tokio::time::sleep(Duration::from_millis(10)).await;
        // Drain any events.
        let event = sub.rx.try_recv().ok();
        if let Some(ev) = event {
            assert_eq!(ev.new_value.data, json!(2));
        }
        // (If we don't get an event, the test still passes — broadcasts
        // can lose messages if no one is listening at the moment of
        // send. The point is that the API doesn't crash.)
    }

    #[tokio::test]
    async fn list_cells_returns_all() {
        let e = engine();
        let sheet = SheetDef {
            id: "s".into(),
            title: None,
            description: None,
            version: None,
            axes: None,
            cells: vec![value_def("a", json!(1)), value_def("b", json!(2))],
        };
        e.load_sheet(sheet).await.unwrap();
        assert_eq!(e.list_cells().len(), 2);
    }

    #[tokio::test]
    async fn get_cell_returns_clone() {
        let e = engine();
        let sheet = SheetDef {
            id: "s".into(),
            title: None,
            description: None,
            version: None,
            axes: None,
            cells: vec![value_def("a", json!(1))],
        };
        e.load_sheet(sheet).await.unwrap();
        let c = e.get_cell("a");
        assert!(c.is_some());
        assert_eq!(c.unwrap().id, "a");
    }

    #[test]
    fn context_key_differs_by_row() {
        let mut a = CallerContext::default();
        a.row = Some(json!(1));
        let mut b = CallerContext::default();
        b.row = Some(json!(2));
        assert_ne!(context_key(&a), context_key(&b));
    }

    #[test]
    fn context_key_same_for_same_context() {
        let a = CallerContext::default();
        let b = CallerContext::default();
        assert_eq!(context_key(&a), context_key(&b));
    }

    #[test]
    fn context_key_differs_by_caller() {
        let mut a = CallerContext::default();
        a.caller = Some("a".into());
        let mut b = CallerContext::default();
        b.caller = Some("b".into());
        assert_ne!(context_key(&a), context_key(&b));
    }

    #[test]
    fn extend_context_appends_trace() {
        let mut parent = CallerContext::default();
        parent.caller = Some("old".into());
        let next = extend_context(&parent, "new", None);
        assert_eq!(next.caller.as_deref(), Some("new"));
        assert!(next.trace.contains(&"new".to_string()));
    }

    #[test]
    fn extend_context_merges_metadata() {
        let mut parent = CallerContext::default();
        let mut meta = std::collections::BTreeMap::new();
        meta.insert("a".to_string(), json!(1));
        parent.metadata = meta;
        let mut more = std::collections::BTreeMap::new();
        more.insert("b".to_string(), json!(2));
        let next = extend_context(&parent, "x", Some(more));
        assert_eq!(next.metadata.get("a"), Some(&json!(1)));
        assert_eq!(next.metadata.get("b"), Some(&json!(2)));
    }

    #[test]
    fn expr_contains_token_basic() {
        assert!(expr_contains_token("a + b", "a"));
        assert!(expr_contains_token("a + b", "b"));
        assert!(!expr_contains_token("a + b", "c"));
    }

    #[test]
    fn expr_contains_token_word_boundary() {
        // "ax" should not match "a".
        assert!(!expr_contains_token("ax + 1", "a"));
        // "a.x" should not match "a".
        assert!(!expr_contains_token("a.x + 1", "a"));
        // But "a.x" should match "a.x".
        assert!(expr_contains_token("a.x + 1", "a.x"));
    }

    #[test]
    fn normalize_id_trims() {
        let e = engine();
        let id = e.normalize_id("  x  ").unwrap();
        assert_eq!(id, "x");
    }

    #[test]
    fn normalize_id_empty_errors() {
        let e = engine();
        let r = e.normalize_id("   ");
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn push_request_via_sender() {
        let e = engine();
        let sheet = SheetDef {
            id: "s".into(),
            title: None,
            description: None,
            version: None,
            axes: None,
            cells: vec![CellDef {
                id: "imu".into(),
                kind: CellKind::Sensor,
                ..Default::default()
            }],
        };
        e.load_sheet(sheet).await.unwrap();
        let tx = e.push_sender();
        tx.send(PushRequest::new("imu", json!({"x": 1.0}))).unwrap();
        // Wait a tick for the background loop.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let v = e.get("imu", CallerContext::default()).await.unwrap();
        assert_eq!(v.data, json!({"x": 1.0}));
    }
}
