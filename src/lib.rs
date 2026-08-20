//! # quilt-jetson
//!
//! Quilt reactive runtime for NVIDIA Jetson devices — the missing
//! mid-tier of the Quilt federation.
//!
//! ## Role in the federation
//!
//! Quilt is a federation of runtimes that share a single sheet format
//! (YAML) and a single cell URI scheme (`quilt://instance/sheet#cell`).
//! The ecosystem has three production tiers:
//!
//! - **Edge micro**: `quilt-esp32` — `no_std` Rust, sensors and motors.
//!   Tight loops, no allocation, no async.
//! - **Edge mid** (this crate): `quilt-jetson` — full Linux on NVIDIA
//!   Jetson. CUDA cores, ROS2, vision models, local persistence,
//!   federation hub.
//! - **Cloud**: `quilt-cloudflare` (Workers) and `quilt-codespace`
//!   (GitHub Codespace) — long-lived, high-resource, always-on.
//!
//! All three tiers read the same YAML sheets and apply the same
//! reactive engine. A `quilt://jetson-lab/perception#vision.obstacles`
//! cell is just a `value`/`formula`/`api` cell on a Jetson that other
//! tiers can subscribe to over HTTP + WebSocket. There is no special
//! "Jetson-only" cell kind — `quilt-jetson` extends the standard eight
//! cell kinds (`value`, `formula`, `api`, `program`, `sensor`,
//! `listener`, `router`, `io`) with hardware-specific implementations
//! of the same kinds:
//!
//! - `sensor` cells read from GPIO, I2C, IMU, LIDAR, NMEA 0183, etc.
//! - `api` cells can target `tensorrt://...` and `onnx://...`
//!   pseudo-URLs (resolved by the vision cell evaluator).
//! - `program` cells can talk to ROS2 via the `ros2_*` helpers
//!   registered in their rhai scope.
//!
//! ## What this crate ships
//!
//! - `QuiltEngine` — async, tokio-native. Holds the cell graph, tracks
//!   dependencies, propagates changes.
//! - `parse_sheet` / `serialize_sheet` — YAML sheet loader/saver.
//! - The eight cell evaluators, Jetson-flavoured.
//! - `Ros2Bridge` — pure-Rust ROS2 client behind a feature flag
//!   (`--features ros2`).
//! - `SqliteStore` — async SQLite history of cell values.
//! - `WebServer` — axum server on `:8080` with the live cell UI.
//! - `FederationClient` — subscribe to `quilt://other/cell` URIs over
//!   HTTP+WebSocket.
//!
//! ## Status (v0.1.0)
//!
//! This is the first cut. The cell evaluators are derived from
//! `quilt-rust`'s core, with three new evaluators for Jetson-specific
//! work (`vision`, the `ros2` `program` helpers, and the
//! `tensorrt://` / `onnx://` `api` URL schemes). The web UI is
//! minimal — list of cells, live values, history view, YAML editor —
//! but functional. ROS2 and ONNX are feature-gated; the default build
//! is the pure-Rust portable subset.
//!
//! ## Quick start
//!
//! ```no_run
//! use quilt_jetson::{QuiltEngine, EngineConfig, parse_sheet};
//! use std::path::Path;
//!
//! # async fn run() -> anyhow::Result<()> {
//! let yaml = std::fs::read_to_string("examples/sensor-fusion.yaml")?;
//! let sheet = parse_sheet(&yaml)?;
//!
//! let engine = std::sync::Arc::new(
//!     QuiltEngine::with_sheet(EngineConfig::default(), sheet)?
//! );
//! let value = engine.get("position.x", Default::default()).await?;
//! println!("x = {}", value.data);
//! # Ok(()) }
//! ```
//!
//! ## Cross-references
//!
//! - [`quilt`](https://github.com/SuperInstance/quilt) — the canonical
//!   TypeScript implementation and landing pages.
//! - [`quilt-rust`](https://github.com/SuperInstance/quilt-rust) —
//!   the pure-Rust sync engine, the source of truth for the cell
//!   semantics.
//! - [`quilt-esp32`](https://github.com/SuperInstance/quilt-esp32) —
//!   the no_std tier.
//! - [`quilt-cloudflare`](https://github.com/SuperInstance/quilt-cloudflare) —
//!   the Workers tier.
//! - [`quilt-codespace`](https://github.com/SuperInstance/quilt-codespace) —
//!   the Codespace tier.
//!
//! ## License
//!
//! Apache-2.0.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(rustdoc::broken_intra_doc_links)]

pub mod cells;
pub mod error;
pub mod federation;
pub mod parser;
pub mod persistence;
pub mod ros2;
pub mod types;
pub mod web;

// Re-exports for convenience. Most users only need these.
pub use crate::engine::{EngineConfig, QuiltEngine, SubscriptionEvent, SubscriptionHandle};
pub use crate::error::{Error, Result};
pub use crate::parser::{parse_sheet, serialize_sheet, validate_sheet};
pub use crate::persistence::{CellHistoryRow, SqliteStore};
pub use crate::types::{
    AxisDef, CallerContext, Cell, CellDef, CellError, CellId, CellKind, CellStatus, CellValue,
    Direction, Effect, EvaluationTrace, RouteTarget, RouterRule, SheetAxes, SheetDef,
    Subscription, SubscriptionId,
};

// ROS2 types are feature-gated.
#[cfg(feature = "ros2")]
pub use crate::ros2::{Ros2Bridge, Ros2Message};

// Federation helpers.
pub use crate::federation::{FederationClient, QuiltRef, RemoteCellEvent};

// Re-export the cell evaluators so downstream callers can drive a cell
// without going through the engine.
pub use crate::cells::{
    evaluate_api, evaluate_formula, evaluate_io, evaluate_listener, evaluate_program,
    evaluate_router, evaluate_sensor, evaluate_value, evaluate_vision, make_io_value,
    make_sensor_value, fire_listener,
};

mod engine;
