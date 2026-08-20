//! # cells
//!
//! The cell evaluators.
//!
//! ## Role in the system
//!
//! Each cell kind has its own evaluation function. The engine
//! dispatches based on `CellDef::kind`. All evaluators take a `Cell`,
//! a `CallerContext`, and (for effectful kinds) a `ProgramRuntime`
//! handle. They return a `CellValue`.
//!
//! The set of evaluators mirrors the eight cell kinds in
//! `quilt-rust`'s core, with three additions:
//!
//! - `vision.rs` — the `vision` module. On Jetson, an `api` cell with
//!   `endpoint: tensorrt://...` or `endpoint: onnx://...` is
//!   dispatched here instead of the regular HTTP evaluator.
//! - `ros2_*` helpers — registered into the rhai scope of every
//!   `program` cell when the `ros2` feature is enabled.
//! - `ai.rs` — an HTTP client to OpenAI-compatible chat-completion
//!   endpoints. The base Quilt supports `model:foo` as a pseudo-URL;
//!   Jetson adds proper `https://api.openai.com/v1/chat/completions`
//!   resolution for cells that declare a real API key.
//!
//! ## Depends on
//!
//! - `crate::types` — the cell, value, context, and effect types.
//! - `crate::error` — error type.
//! - `rhai` (program, formula) — embedded scripting.
//! - `reqwest` (api, ai) — async HTTP client.
//! - `image`, `imageproc` (vision) — image loading and preprocessing.
//! - `nalgebra` (vision, formula helpers) — math.
//!
//! ## Used by
//!
//! - `crate::engine` — dispatches to the right evaluator based on
//!   kind. The engine is fully async; the evaluators return
//!   `async fn` futures directly for `api`, `program`, and `router`,
//!   and sync functions for `value`, `formula`, `sensor`, and `io`.
//! - Tests — every evaluator is exercised by at least one unit test.
//!
//! ## Status
//!
//! - ✅ `value` — implemented + tests
//! - ✅ `formula` — implemented (rhai-based) + tests
//! - ✅ `api` — implemented (reqwest-based) + tests
//! - ✅ `program` — implemented (rhai-based, async runtime) + tests
//! - ✅ `sensor` — implemented (push-based, hardware-agnostic) + tests
//! - ✅ `io` — implemented (push-based) + tests
//! - ✅ `listener` — implemented + tests
//! - ✅ `router` — implemented + tests
//! - ✅ `vision` — implemented (stub when feature off) + tests
//! - ✅ `ai` — implemented (OpenAI-compatible) + tests

pub mod ai;
pub mod api;
pub mod formula;
pub mod io;
pub mod listener;
pub mod program;
pub mod router;
pub mod sensor;
pub mod value;
pub mod vision;

pub use api::{evaluate_api, ApiExecutor, ApiExecutorRef, ApiResponse, ReqwestExecutor, StubExecutor};
pub use formula::{evaluate_formula, FormulaEngine};
pub use io::{evaluate_io, make_io_value};
pub use listener::{evaluate_listener, fire_listener};
pub use program::{evaluate_program, NullRuntime, ProgramRuntime};
pub use router::evaluate_router;
pub use sensor::{evaluate_sensor, make_sensor_value};
pub use value::evaluate_value;
pub use vision::{evaluate_vision, VisionBackend, VisionRequest, VisionResponse, VisionTask};
pub use ai::{evaluate_ai, AiRequest, AiResponse};
