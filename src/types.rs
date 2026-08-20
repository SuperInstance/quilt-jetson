//! # types.rs
//!
//! The fundamental type vocabulary for the Quilt Jetson runtime.
//!
//! ## Role in the system
//!
//! This module is the schema of the universe. Every other module imports
//! from here. The principle: a `CellId` is a stable, location-independent
//! capability pointer (not a coordinate); a `CallerContext` travels with
//! every call so cells can route on position/identity; `CellValue`
//! carries its own status so callers know if it's fresh, computing, or
//! errored.
//!
//! ## Depends on
//!
//! - `serde`, `serde_json` — for serialization of `CellDef` from YAML and
//!   for `CellValue::data` (an arbitrary JSON value).
//!
//! ## Used by
//!
//! - `engine.rs` — constructs `Cell` instances from `CellDef`.
//! - `parser.rs` — produces `CellDef` from YAML.
//! - `cells/*.rs` — every evaluator takes a `Cell` and returns a
//!   `CellValue`.
//!
//! ## Compatibility
//!
//! These types are intentionally **API-compatible** with
//! [`quilt_rust::types`](https://docs.rs/quilt-core) so a sheet
//! written for `quilt-rust` loads in `quilt-jetson` and vice versa. The
//! only difference is the runtime: `quilt-rust` is sync; this crate
//! is fully async (`tokio`).
//!
//! ## Key decisions
//!
//! - `CellId` and `CellRef` are both `String` for now. `CellRef` is a
//!   separate type alias so we can later add expressions like
//!   `fleet.boat*.rudder` (range) or `router.model?caller.row>10`
//!   (conditional).
//! - `Effect` is an enum so the runtime can reason about cost, debounce,
//!   retry, and show effects in the UI.
//! - `CellStatus` is part of `CellValue`, not a separate channel. A
//!   cell always knows its own state.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Identity and capability pointers
// ---------------------------------------------------------------------------

/// The stable identity of a cell. This is what other cells reference.
///
/// NOT a coordinate. Survives reordering, sheet splits, and refactors.
///
/// Examples:
/// - `"compass.heading"`
/// - `"fleet.boat1.rudder"`
/// - `"router.model"`
/// - `"quilt://jetson-lab/perception#vision.obstacles"` — a remote cell
///   in another tier of the federation.
pub type CellId = String;

/// A reference to another cell. Same shape as `CellId` for now, but
/// typed separately so we can later add expressions like
/// `fleet.boat*.rudder` (range) or `router.model?caller.row>10`
/// (conditional).
pub type CellRef = String;

/// A subscription identifier. Returned from `QuiltEngine::subscribe`.
pub type SubscriptionId = String;

// ---------------------------------------------------------------------------
// Cell kinds
// ---------------------------------------------------------------------------

/// The kind of cell — determines evaluation semantics.
///
/// All eight kinds are shared with `quilt-rust` and `quilt-core`. Jetson
/// hardware is reached through the `source` / `endpoint` / `port` fields
/// of the standard kinds, plus the `tensorrt://` and `onnx://` URL
/// schemes recognized by the `api` cell evaluator (which dispatches to
/// the vision module).
///
/// - `Value`:    static data, no dependencies
/// - `Formula`:  pure reactive computation, deps auto-tracked
/// - `Api`:      external endpoint/model call, async, may have effects
/// - `Program`:  stateful, side-effectful logic, async, explicit triggers
/// - `Sensor`:   streaming input, push-based
/// - `Listener`: delta-triggered execution
/// - `Router`:   caller-aware policy, routes the call elsewhere
/// - `Io`:       bidirectional port (form, MCP, GPIO, webhook)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CellKind {
    /// A static piece of data. No dependencies, no computation.
    #[serde(rename = "value")]
    Value,
    /// Pure reactive computation. Dependencies auto-tracked.
    #[serde(rename = "formula")]
    Formula,
    /// External endpoint or model call. Async, may have effects.
    ///
    /// On Jetson, an `api` cell can use a `tensorrt://path/to/engine`
    /// or `onnx://path/to/model.onnx` endpoint — the engine routes
    /// those to the vision module instead of `reqwest`.
    #[serde(rename = "api")]
    Api,
    /// Stateful, side-effectful logic. Async, explicit triggers.
    ///
    /// On Jetson, a `program` cell can talk to ROS2 via the
    /// `ros2_*` helpers in its rhai scope.
    #[serde(rename = "program")]
    Program,
    /// Streaming input. Push-based; external adapters feed values in.
    ///
    /// On Jetson, a `sensor` cell with `source: "i2c:/dev/i2c-1:0x68"`
    /// reads from the IMU, `source: "serial:/dev/ttyTHS1:115200"`
    /// reads NMEA from a GPS, and so on.
    #[serde(rename = "sensor")]
    Sensor,
    /// Delta-triggered execution. Watches other cells, fires actions.
    #[serde(rename = "listener")]
    Listener,
    /// Caller-aware policy. Routes the call to another cell.
    #[serde(rename = "router")]
    Router,
    /// Bidirectional port (form, MCP, GPIO, webhook).
    ///
    /// On Jetson, an `io` cell with `port: "mqtt://broker/topic"` is
    /// a pub/sub MQTT channel. `port: "ws://host:port/path"` is a
    /// WebSocket.
    #[serde(rename = "io")]
    Io,
}

impl CellKind {
    /// All kinds, in declaration order. Used by the parser for
    /// validation and by the CLI for symbol rendering.
    pub const ALL: &'static [CellKind] = &[
        CellKind::Value,
        CellKind::Formula,
        CellKind::Api,
        CellKind::Program,
        CellKind::Sensor,
        CellKind::Listener,
        CellKind::Router,
        CellKind::Io,
    ];

    /// Short string label for the kind. Used in CLI output and the
    /// web UI.
    pub fn as_str(self) -> &'static str {
        match self {
            CellKind::Value => "value",
            CellKind::Formula => "formula",
            CellKind::Api => "api",
            CellKind::Program => "program",
            CellKind::Sensor => "sensor",
            CellKind::Listener => "listener",
            CellKind::Router => "router",
            CellKind::Io => "io",
        }
    }

    /// Try to parse a kind from a string. Inverse of `as_str`.
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "value" => Some(CellKind::Value),
            "formula" => Some(CellKind::Formula),
            "api" => Some(CellKind::Api),
            "program" => Some(CellKind::Program),
            "sensor" => Some(CellKind::Sensor),
            "listener" => Some(CellKind::Listener),
            "router" => Some(CellKind::Router),
            "io" => Some(CellKind::Io),
            _ => None,
        }
    }

    /// Is this a pure cell (pull-based, no side effects)?
    pub fn is_pure(self) -> bool {
        matches!(self, CellKind::Value | CellKind::Formula)
    }

    /// Is this an effectful cell (async, may have side effects)?
    pub fn is_effectful(self) -> bool {
        matches!(
            self,
            CellKind::Api | CellKind::Program | CellKind::Router
        )
    }
}

// ---------------------------------------------------------------------------
// Cell status & values
// ---------------------------------------------------------------------------

/// The status of a cell's current value.
///
/// - `Idle`:      has a value but hasn't been touched
/// - `Computing`: evaluation in flight
/// - `Ready`:     value is fresh
/// - `Error`:     last evaluation failed
/// - `Stale`:     dependencies changed, needs recompute
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CellStatus {
    /// Has a value but hasn't been touched.
    #[serde(rename = "idle")]
    Idle,
    /// Evaluation in flight.
    #[serde(rename = "computing")]
    Computing,
    /// Value is fresh.
    #[serde(rename = "ready")]
    Ready,
    /// Last evaluation failed.
    #[serde(rename = "error")]
    Error,
    /// Dependencies changed, needs recompute.
    #[serde(rename = "stale")]
    Stale,
}

impl CellStatus {
    /// Short string label. Inverse of `from_str` for the well-known
    /// cases.
    pub fn as_str(self) -> &'static str {
        match self {
            CellStatus::Idle => "idle",
            CellStatus::Computing => "computing",
            CellStatus::Ready => "ready",
            CellStatus::Error => "error",
            CellStatus::Stale => "stale",
        }
    }
}

/// An error attached to a `CellValue` when the last evaluation failed.
///
/// We keep the structure minimal but typed so callers can render the
/// message without parsing strings.
#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
pub struct CellError {
    /// Human-readable error message.
    pub message: String,
    /// Optional source location (file:line, function name, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
}

impl CellError {
    /// Convenience constructor.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            stack: None,
        }
    }

    /// Constructor that attaches a stack trace.
    pub fn with_stack(message: impl Into<String>, stack: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            stack: Some(stack.into()),
        }
    }
}

impl std::fmt::Display for CellError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// Effects are what a cell *did* during evaluation. Pure cells have no
/// effects. Effectful cells declare their effects so the runtime can:
/// reason about cost, debounce/retry, show them in the UI, and
/// decompose them into cheaper cells over time.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Effect {
    /// A network call (HTTP, MCP, etc.).
    Network {
        /// The URL or address contacted.
        url: String,
        /// The HTTP method (or transport-level verb).
        method: String,
    },
    /// A storage read or write.
    Storage {
        /// Read or write.
        op: StorageOp,
        /// The key touched.
        key: String,
    },
    /// An I/O port direction event.
    Io {
        /// The port name.
        port: String,
        /// Inbound or outbound.
        direction: Direction,
    },
    /// A model invocation.
    Model {
        /// The provider name (e.g. `"openai"`, `"anthropic"`,
        /// `"tensorrt"`, `"onnx"`).
        provider: String,
        /// Optional input token count.
        #[serde(skip_serializing_if = "Option::is_none")]
        tokens_in: Option<u64>,
        /// Optional output token count.
        #[serde(skip_serializing_if = "Option::is_none")]
        tokens_out: Option<u64>,
    },
    /// A vision inference call (TensorRT or ONNX).
    Vision {
        /// The model identifier (path or name).
        model: String,
        /// The task (e.g. `"detect"`, `"classify"`, `"segment"`).
        task: String,
        /// Latency in milliseconds.
        latency_ms: u64,
    },
    /// A ROS2 publish/subscribe call.
    Ros2 {
        /// The topic name.
        topic: String,
        /// The message type (e.g. `"geometry_msgs/Twist"`).
        message_type: String,
        /// The direction (publish or subscribe).
        direction: Direction,
    },
    /// Generic compute cost marker.
    Compute {
        /// Duration in milliseconds.
        ms: u64,
    },
}

/// Storage operation kind. Used by the `Effect::Storage` variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageOp {
    /// A read.
    Read,
    /// A write.
    Write,
}

/// Direction for an `Effect::Io` / `Effect::Ros2` event or a
/// `CellDef::direction` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Inbound to the cell.
    #[serde(rename = "in", alias = "input")]
    In,
    /// Outbound from the cell.
    #[serde(rename = "out", alias = "output")]
    Out,
    /// Both directions.
    Bidirectional,
}

/// The current value of a cell, with metadata. Cells always know their
/// own status — there is no separate "is it ready?" check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CellValue {
    /// The data. For a value cell this is the literal `value` from the
    /// `CellDef`. For a formula it's the computed result. For a program
    /// it's whatever the script returned. For an API cell it's the
    /// parsed response body. For a sensor it's the most recent push.
    pub data: Value,
    /// The current status.
    pub status: CellStatus,
    /// Wall-clock time (millis since epoch) when the value was last
    /// computed. `None` if the cell has never been evaluated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub computed_at: Option<u64>,
    /// Error attached to this value, if any. Present iff
    /// `status == Error`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<CellError>,
    /// Effects that produced this value. Empty for pure cells.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<Effect>,
}

impl Default for CellValue {
    fn default() -> Self {
        Self {
            data: Value::Null,
            status: CellStatus::Idle,
            computed_at: None,
            error: None,
            effects: Vec::new(),
        }
    }
}

impl CellValue {
    /// Construct a fresh ready value.
    pub fn ready(data: impl Into<Value>) -> Self {
        Self {
            data: data.into(),
            status: CellStatus::Ready,
            computed_at: Some(now_millis()),
            error: None,
            effects: Vec::new(),
        }
    }

    /// Construct an error value.
    pub fn err(message: impl Into<String>) -> Self {
        Self {
            data: Value::Null,
            status: CellStatus::Error,
            computed_at: Some(now_millis()),
            error: Some(CellError::new(message)),
            effects: Vec::new(),
        }
    }

    /// Construct an error value with a stack trace.
    pub fn err_with_stack(message: impl Into<String>, stack: impl Into<String>) -> Self {
        Self {
            data: Value::Null,
            status: CellStatus::Error,
            computed_at: Some(now_millis()),
            error: Some(CellError::with_stack(message, stack)),
            effects: Vec::new(),
        }
    }

    /// True if this value is fresh and not errored.
    pub fn is_ready(&self) -> bool {
        self.status == CellStatus::Ready
    }

    /// True if this value carries an error.
    pub fn is_error(&self) -> bool {
        self.status == CellStatus::Error
    }
}

// ---------------------------------------------------------------------------
// Caller context
// ---------------------------------------------------------------------------

/// The context carried with every cell call. This is what makes
/// routing possible. The runtime fills in `trace` and `timestamp`
/// automatically; callers attach `identity` and `metadata`.
///
/// Row/column are the spatial axes. The TypeScript original lets them
/// be either numbers or strings; we follow the same convention but
/// serialize them as `Value` so the YAML can carry whatever the user
/// wants.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallerContext {
    /// Spatial position — what row this call is coming from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row: Option<Value>,
    /// Spatial position — what column this call is coming from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<Value>,
    /// Which sheet the caller is in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sheet: Option<String>,
    /// The cell that initiated this call (the immediate caller).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller: Option<CellId>,
    /// The full ancestor chain (for provenance/tracing).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trace: Vec<CellId>,
    /// Who is making the call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// Arbitrary metadata. Common uses: `{ text: "set heading to 270" }`
    /// for voice commands, `{ boat: "boat-1" }` for fleet routing.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, Value>,
    /// When this call started (millis since epoch).
    pub timestamp: u64,
}

impl Default for CallerContext {
    fn default() -> Self {
        Self {
            row: None,
            column: None,
            sheet: None,
            caller: None,
            trace: Vec::new(),
            identity: None,
            metadata: BTreeMap::new(),
            timestamp: now_millis(),
        }
    }
}

/// Identity carried in the caller context. Used by routers to pick
/// different models / cells for different callers (e.g. `premium` users
/// get `gpt-4o`, everyone else gets `gpt-4o-mini`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    /// The id of this identity (e.g. user id, boat id).
    pub id: String,
    /// The kind of identity (e.g. `"user"`, `"boat"`, `"service"`).
    #[serde(default = "default_identity_kind")]
    pub kind: String,
    /// Tags for routing rules.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

fn default_identity_kind() -> String {
    "user".to_string()
}

// ---------------------------------------------------------------------------
// Router rules
// ---------------------------------------------------------------------------

/// A router rule. The `when` is a small expression evaluated in the
/// caller's context. The `route` can be:
///
/// - a string cell id to delegate to
/// - a `cell` reference (with optional `with` extras) to delegate to
/// - a `model` spec to swap implementations
/// - a `value` literal to return
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterRule {
    /// The condition. A small expression evaluated in caller scope.
    pub when: String,
    /// Where to send the call when `when` is true.
    pub route: RouteTarget,
}

/// The destination of a router rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RouteTarget {
    /// A bare cell id. The router delegates the call to this cell.
    CellId(String),
    /// A cell reference with optional extra context to merge.
    Cell {
        /// The cell to delegate to.
        cell: CellRef,
        /// Optional context overrides to merge into the result.
        #[serde(skip_serializing_if = "Option::is_none")]
        with: Option<BTreeMap<String, Value>>,
    },
    /// A model swap. Implementation lives in the model provider layer.
    Model {
        /// The model name (e.g. `"gpt-4o"`, `"claude-sonnet-4-5"`).
        model: String,
    },
    /// A literal value to return.
    Value {
        /// The literal value.
        value: Value,
    },
}

// ---------------------------------------------------------------------------
// Cell definitions and live cells
// ---------------------------------------------------------------------------

/// The definition of a cell — what the YAML/DSL compiles to. The
/// runtime instantiates a `Cell` from a `CellDef`.
///
/// Every field except `id` and `kind` is optional. The semantics of
/// each field are determined by `kind`; unused fields are simply
/// ignored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CellDef {
    /// The stable id of the cell. Required.
    pub id: CellId,
    /// What kind of cell this is. Required.
    pub kind: CellKind,

    // --- value cells ---
    /// Static value, used by `value` cells. Anything JSON-serializable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,

    // --- formula cells ---
    /// Expression body. For `formula` cells, the `=` prefix is optional
    /// (we strip it if present). For `program` cells this is the rhai
    /// script body.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expr: Option<String>,

    // --- api cells ---
    /// The endpoint URL or pseudo-URL (`model:foo`, `mcp://server/tool`,
    /// `tensorrt://path/to/engine`, `onnx://path/to/model`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// HTTP method (default `GET`). Used by `api` cells.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// HTTP headers. Used by `api` cells.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,

    // --- program cells ---
    /// Code body. For `program` cells this is a rhai script.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,

    // --- sensor cells ---
    /// Logical source identifier. The runtime does not interpret this
    /// for non-Jetson platforms; on Jetson, the `ros2_bridge` and
    /// hardware adapter do. Examples: `"imu:i2c-1:0x68"`,
    /// `"nmea:/dev/ttyTHS1:115200"`, `"lidar:/dev/ttyUSB0"`,
    /// `"camera:/dev/video0"`, `"simulated"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Sampling rate in Hz (advisory only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate: Option<f64>,
    /// Initial value for sensor cells. If a sensor has no real data
    /// yet, the engine uses this value until something pushes a real
    /// one. Lets demo sheets work without an adapter wired up.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,

    // --- listener cells ---
    /// Cells this listener watches. Required for `listener` kind.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub watch: Vec<CellRef>,
    /// Optional condition expression. Evaluated in caller scope; if it
    /// returns falsy the action does not fire.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    /// The cell id to call when the condition fires.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,

    // --- router cells ---
    /// Routing rules. Required for `router` kind.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<RouterRule>,

    // --- io cells ---
    /// The I/O port name. Required for `io` kind.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<String>,
    /// I/O direction. Required for `io` kind.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direction: Option<Direction>,

    // --- metadata ---
    /// Human-readable description. Shown in the CLI and web UI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Engineering unit (e.g. `"degrees"`, `"celsius"`). Cosmetic.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Declared input type. Cosmetic for now.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_type: Option<String>,
    /// Declared output type. Cosmetic for now.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_type: Option<String>,

    // --- dependencies ---
    /// Explicit dependencies. For `formula` cells dependencies are
    /// auto-detected from `expr` and merged with this list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<CellRef>,

    // --- permissions (cosmetic) ---
    /// Permission hints. Not enforced in the MVP.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub permissions: BTreeMap<String, Value>,
}

impl Default for CellDef {
    fn default() -> Self {
        Self {
            id: String::new(),
            kind: CellKind::Value,
            description: None,
            value: None,
            expr: None,
            default: None,
            endpoint: None,
            method: None,
            headers: BTreeMap::new(),
            code: None,
            source: None,
            rate: None,
            watch: Vec::new(),
            condition: None,
            action: None,
            rules: Vec::new(),
            port: None,
            direction: None,
            unit: None,
            input_type: None,
            output_type: None,
            deps: Vec::new(),
            permissions: BTreeMap::new(),
        }
    }
}

impl CellDef {
    /// Validate a `CellDef` against its kind. Returns a list of
    /// human-readable problems; empty list means OK.
    ///
    /// Validation here is light — it catches obvious shape errors.
    /// Semantic checks (e.g. "the formula references a cell that
    /// doesn't exist") happen at engine load time.
    pub fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();
        match self.kind {
            CellKind::Value => {
                if self.value.is_none() {
                    problems.push(format!("value cell '{}' has no value", self.id));
                }
            }
            CellKind::Formula => {
                if self.expr.is_none() {
                    problems.push(format!("formula cell '{}' has no expr", self.id));
                }
            }
            CellKind::Api => {
                if self.endpoint.is_none() {
                    problems.push(format!("api cell '{}' has no endpoint", self.id));
                }
            }
            CellKind::Program => {
                if self.code.is_none() {
                    problems.push(format!("program cell '{}' has no code", self.id));
                }
            }
            CellKind::Sensor => {
                // source is optional; a "simulated" sensor has no source.
            }
            CellKind::Listener => {
                if self.watch.is_empty() {
                    problems.push(format!("listener cell '{}' has no watch list", self.id));
                }
            }
            CellKind::Router => {
                if self.rules.is_empty() {
                    problems.push(format!("router cell '{}' has no rules", self.id));
                }
            }
            CellKind::Io => {
                if self.port.is_none() {
                    problems.push(format!("io cell '{}' has no port", self.id));
                }
                if self.direction.is_none() {
                    problems.push(format!("io cell '{}' has no direction", self.id));
                }
            }
        }
        problems
    }
}

/// A live cell instance — the runtime's working representation. Built
/// from a `CellDef` at load time and mutated as the engine runs.
#[derive(Debug, Clone)]
pub struct Cell {
    /// The cell id, copied from the def.
    pub id: CellId,
    /// The original definition. Immutable after load.
    pub def: CellDef,
    /// The current value. Initially `idle` with `Null` data.
    pub value: CellValue,
    /// Outgoing edges — cells this cell reads from.
    pub dependencies: HashSet<CellId>,
    /// Incoming edges — cells that read from this cell. Used for
    /// propagation.
    pub dependents: HashSet<CellId>,
    /// Per-context cache. The key is `context_key(ctx)`. The same cell
    /// called from different callers can have different cached values.
    pub context_cache: indexmap::IndexMap<String, CellValue>,
    /// Last evaluation context, for debugging.
    pub last_context: Option<CallerContext>,
}

impl Cell {
    /// Construct a fresh cell from a def. The cell is in `idle` state.
    pub fn new(def: CellDef) -> Self {
        // For value cells, seed the cell with the YAML-provided value
        // so `get` returns the right thing immediately. For sensor
        // cells, seed with `def.default` if present (lets demo sheets
        // work without a real adapter).
        let value = if let Some(v) = &def.value {
            CellValue {
                data: v.clone(),
                status: CellStatus::Ready,
                computed_at: Some(now_millis()),
                error: None,
                effects: Vec::new(),
            }
        } else if matches!(def.kind, CellKind::Sensor) {
            if let Some(d) = &def.default {
                CellValue {
                    data: d.clone(),
                    status: CellStatus::Ready,
                    computed_at: Some(now_millis()),
                    error: None,
                    effects: Vec::new(),
                }
            } else {
                CellValue::default()
            }
        } else {
            CellValue::default()
        };
        Self {
            id: def.id.clone(),
            def,
            value,
            dependencies: HashSet::new(),
            dependents: HashSet::new(),
            context_cache: indexmap::IndexMap::new(),
            last_context: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Sheet definition
// ---------------------------------------------------------------------------

/// The configuration for a Quilt sheet. A sheet is the unit of load —
/// you feed one of these to `QuiltEngine::load_sheet`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SheetDef {
    /// Stable id for the sheet. Used as the MCP resource name and the
    /// default engine id.
    pub id: String,
    /// Human-readable title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Human-readable description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Sheet version string. Cosmetic.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Semantic axes — what rows and columns mean in this sheet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub axes: Option<SheetAxes>,
    /// All the cells in this sheet.
    pub cells: Vec<CellDef>,
}

/// The semantic axes of a sheet. These describe what the rows and
/// columns of the conceptual grid mean; the runtime does not interpret
/// them but the UI and the MCP server do.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SheetAxes {
    /// What each row represents. Example: `{ name: "boat" }`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows: Option<AxisDef>,
    /// What each column represents. Example: `{ name: "capability" }`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cols: Option<AxisDef>,
}

/// A single axis definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxisDef {
    /// The semantic name of the axis.
    pub name: String,
    /// Optional explicit enumeration of values the axis can take.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<Value>>,
}

// ---------------------------------------------------------------------------
// Subscriptions and traces
// ---------------------------------------------------------------------------

/// A subscription — what a listener or external client is watching.
pub struct Subscription {
    /// Subscription id. Unique within an engine.
    pub id: SubscriptionId,
    /// The cell being watched.
    pub cell_id: CellId,
    /// The callback. Called with `(new_value, old_value)`.
    pub callback: Box<dyn crate::engine::SubscriptionCallback>,
    /// Optional filter. If present, the callback is only invoked when
    /// the filter returns true.
    pub filter: Option<Box<dyn crate::engine::SubscriptionFilter>>,
}

impl std::fmt::Debug for Subscription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subscription")
            .field("id", &self.id)
            .field("cell_id", &self.cell_id)
            .field("callback", &"<fn>")
            .field("filter", &"<fn>")
            .finish()
    }
}

/// The trace of how a particular evaluation happened. Used for
/// debugging, time-travel, and decomposition. Every cell evaluation
/// emits one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationTrace {
    /// The cell that was evaluated.
    pub cell_id: CellId,
    /// When the evaluation started (millis since epoch).
    pub started_at: u64,
    /// When the evaluation completed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<u64>,
    /// Total duration in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// The caller context this evaluation happened in.
    pub context: CallerContext,
    /// Effects produced.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<Effect>,
    /// Error, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<CellError>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Wall-clock milliseconds since the UNIX epoch.
pub(crate) fn now_millis() -> u64 {
    chrono::Utc::now().timestamp_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_kind_round_trip() {
        for kind in CellKind::ALL {
            assert_eq!(CellKind::from_str(kind.as_str()), Some(*kind));
        }
    }

    #[test]
    fn cell_value_ready_constructor() {
        let v = CellValue::ready(42);
        assert!(v.is_ready());
        assert!(!v.is_error());
        assert_eq!(v.data, Value::from(42));
    }

    #[test]
    fn cell_value_error_constructor() {
        let v = CellValue::err("oops");
        assert!(v.is_error());
        assert!(!v.is_ready());
        assert_eq!(v.error.as_ref().unwrap().message, "oops");
    }

    #[test]
    fn cell_value_default_is_idle_null() {
        let v = CellValue::default();
        assert_eq!(v.status, CellStatus::Idle);
        assert_eq!(v.data, Value::Null);
    }

    #[test]
    fn cell_def_validate_missing_value() {
        let def = CellDef {
            id: "x".into(),
            kind: CellKind::Value,
            ..Default::default()
        };
        let problems = def.validate();
        assert!(!problems.is_empty());
    }

    #[test]
    fn cell_def_validate_missing_endpoint() {
        let def = CellDef {
            id: "x".into(),
            kind: CellKind::Api,
            ..Default::default()
        };
        let problems = def.validate();
        assert!(!problems.is_empty());
    }

    #[test]
    fn cell_def_validate_missing_expr() {
        let def = CellDef {
            id: "x".into(),
            kind: CellKind::Formula,
            ..Default::default()
        };
        let problems = def.validate();
        assert!(!problems.is_empty());
    }

    #[test]
    fn cell_def_validate_valid_value() {
        let def = CellDef {
            id: "x".into(),
            kind: CellKind::Value,
            value: Some(Value::from(1)),
            ..Default::default()
        };
        let problems = def.validate();
        assert!(problems.is_empty());
    }

    #[test]
    fn cell_new_seeds_value() {
        let def = CellDef {
            id: "x".into(),
            kind: CellKind::Value,
            value: Some(Value::String("hello".into())),
            ..Default::default()
        };
        let c = Cell::new(def);
        assert_eq!(c.value.data, Value::String("hello".into()));
        assert_eq!(c.value.status, CellStatus::Ready);
    }

    #[test]
    fn cell_new_seeds_sensor_default() {
        let def = CellDef {
            id: "s".into(),
            kind: CellKind::Sensor,
            default: Some(Value::from(0.0)),
            ..Default::default()
        };
        let c = Cell::new(def);
        assert_eq!(c.value.data, Value::from(0.0));
        assert_eq!(c.value.status, CellStatus::Ready);
    }

    #[test]
    fn cell_new_no_default_idle() {
        let def = CellDef {
            id: "s".into(),
            kind: CellKind::Sensor,
            ..Default::default()
        };
        let c = Cell::new(def);
        assert_eq!(c.value.status, CellStatus::Idle);
    }

    #[test]
    fn caller_context_default() {
        let ctx = CallerContext::default();
        assert!(ctx.trace.is_empty());
        assert!(ctx.metadata.is_empty());
        assert!(ctx.timestamp > 0);
    }

    #[test]
    fn status_as_str_matches_kind_as_str() {
        // Just verify all status labels are unique.
        let mut labels: Vec<&str> = ["idle", "computing", "ready", "error", "stale"]
            .iter()
            .copied()
            .collect();
        labels.sort();
        labels.dedup();
        assert_eq!(labels.len(), 5);
    }

    #[test]
    fn effect_serializes_with_kind_tag() {
        let effect = Effect::Network {
            url: "https://example.com".into(),
            method: "GET".into(),
        };
        let s = serde_json::to_string(&effect).unwrap();
        assert!(s.contains("\"kind\":\"network\""));
    }

    #[test]
    fn effect_vision_includes_latency() {
        let e = Effect::Vision {
            model: "yolov8n".into(),
            task: "detect".into(),
            latency_ms: 12,
        };
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains("\"kind\":\"vision\""));
        assert!(s.contains("\"latency_ms\":12"));
    }
}
