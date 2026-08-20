//! # cells/program.rs
//!
//! Program cell evaluator — stateful, side-effectful, async.
//!
//! ## Role in the system
//!
//! A `program` cell is a rhai script that has access to:
//!
//! - `get(id)` — read another cell's value
//! - `set(id, value)` — write a value to another cell
//! - `call(id, input)` — call another cell as a capability
//! - `list()` — list all cell ids
//! - `push(id, value)` — push a value into a sensor/io cell
//!
//! The script is compiled and run by the engine. The runtime handle
//! is async — `get`, `set`, and `call` all return futures. This is
//! the main difference from `quilt-rust`'s sync engine.
//!
//! ## Depends on
//!
//! - `rhai` — embedded scripting language.
//! - `crate::types` — `Cell`, `CellValue`, `CallerContext`.
//! - `crate::error` — `Error`, `Result`.
//!
//! ## Used by
//!
//! - `crate::engine` — dispatches to this on `get`/`call` for
//!   `program` cells.
//!
//! ## Key decisions
//!
//! - Rhai is sandboxed by default. We do not register any of the I/O
//!   packages (`File`, `Http`, etc.) in the engine, so a program cell
//!   cannot read files or make network requests directly. The
//!   `runtime` handle is the *only* way to reach the outside world.
//! - The runtime methods are exposed as top-level scope variables
//!   (`get`, `set`, `call`, `list`, `push`) so the user script can
//!   write `get("a")` directly.
//! - The rhai script returns either a `Value` (becomes the cell's
//!   data) or a `Map` (becomes an object). The script can also
//!   `set`/`push` to other cells as a side effect.

use std::sync::Arc;

use rhai::{Array, Dynamic, Engine, Map, Scope};
use serde_json::Value;

use crate::error::{Error, Result};
use crate::types::{now_millis, Cell, CellStatus, CellValue, CallerContext};

/// The runtime handle exposed to `program` and `router` cells. Async
/// so the user script can `await` get/set/call on other cells.
#[async_trait::async_trait]
pub trait ProgramRuntime: Send + Sync {
    /// Get a cell's current value.
    async fn get_async(&self, id: &str, ctx: &CallerContext) -> Result<CellValue>;
    /// Set a cell's value.
    async fn set_async(&self, id: &str, value: Value, ctx: &CallerContext) -> Result<()>;
    /// Call a cell as a capability.
    async fn call_async(
        &self,
        id: &str,
        input: Option<Value>,
        ctx: &CallerContext,
    ) -> Result<CellValue>;
    /// List all defined cell ids.
    fn list(&self) -> Vec<String>;
    /// Push a value to a sensor/io cell.
    fn push(&self, id: &str, data: Value) -> Result<()>;
}

/// A no-op runtime, used in unit tests that exercise a program cell
/// without the full engine.
pub struct NullRuntime;

#[async_trait::async_trait]
impl ProgramRuntime for NullRuntime {
    async fn get_async(&self, _id: &str, _ctx: &CallerContext) -> Result<CellValue> {
        Ok(CellValue::default())
    }
    async fn set_async(&self, _id: &str, _value: Value, _ctx: &CallerContext) -> Result<()> {
        Ok(())
    }
    async fn call_async(
        &self,
        _id: &str,
        _input: Option<Value>,
        _ctx: &CallerContext,
    ) -> Result<CellValue> {
        Ok(CellValue::default())
    }
    fn list(&self) -> Vec<String> {
        Vec::new()
    }
    fn push(&self, _id: &str, _data: Value) -> Result<()> {
        Ok(())
    }
}

/// Evaluate a program cell. The script is compiled and run inside
/// the current tokio runtime; the runtime handle is the `EngineRuntime`
/// provided by `QuiltEngine`.
pub async fn evaluate_program(
    cell: Cell,
    ctx: CallerContext,
    _input: Option<Value>,
    runtime: Arc<dyn ProgramRuntime>,
) -> CellValue {
    let started_at = now_millis();
    let code = match &cell.def.code {
        Some(c) => c.clone(),
        None => return CellValue::err("program cell has no code"),
    };

    // Set up the rhai engine.
    let mut engine = Engine::new();
    // No I/O packages. The runtime handle is the only way out.

    // Bind runtime methods to top-level scope variables.
    let get_id = cell.id.clone();
    let get_ctx = ctx.clone();
    let rt_for_get = runtime.clone();
    engine.register_fn("get", move |id: String| -> Dynamic {
        // We block on the async runtime here. This is the same
        // trick `quilt-rust` uses: inside a tokio runtime, we can
        // call `Handle::block_on`.
        let v = tokio::runtime::Handle::current()
            .block_on(rt_for_get.get_async(&id, &get_ctx))
            .unwrap_or_default();
        let _ = &get_id; // suppress unused
        json_to_dynamic(v.data)
    });

    let set_id = cell.id.clone();
    let set_ctx = ctx.clone();
    let rt_for_set = runtime.clone();
    engine.register_fn("set", move |id: String, value: Dynamic| -> bool {
        let v = dynamic_to_json(value);
        let _ = &set_id;
        tokio::runtime::Handle::current()
            .block_on(rt_for_set.set_async(&id, v, &set_ctx))
            .is_ok()
    });

    let call_id = cell.id.clone();
    let call_ctx = ctx.clone();
    let rt_for_call = runtime.clone();
    engine.register_fn("call", move |id: String, input: Dynamic| -> Dynamic {
        let v = dynamic_to_json(input);
        let r = tokio::runtime::Handle::current().block_on(rt_for_call.call_async(
            &id,
            Some(v),
            &call_ctx,
        ));
        let _ = &call_id;
        match r {
            Ok(cv) => json_to_dynamic(cv.data),
            Err(_) => Dynamic::UNIT,
        }
    });

    let list_rt = runtime.clone();
    engine.register_fn("list", move || -> Array {
        list_rt
            .list()
            .into_iter()
            .map(|s| s.into())
            .collect::<Array>()
    });

    let push_rt = runtime.clone();
    engine.register_fn("push", move |id: String, value: Dynamic| -> bool {
        let v = dynamic_to_json(value);
        push_rt.push(&id, v).is_ok()
    });

    // Inject the caller context.
    let mut scope = Scope::new();
    let mut caller = Map::new();
    caller.insert("row".into(), json_to_dynamic(ctx.row.clone().unwrap_or(Value::Null)));
    caller.insert(
        "column".into(),
        json_to_dynamic(ctx.column.clone().unwrap_or(Value::Null)),
    );
    caller.insert(
        "sheet".into(),
        json_to_dynamic(ctx.sheet.clone().map(Value::String).unwrap_or(Value::Null)),
    );
    let mut meta = Map::new();
    for (k, v) in &ctx.metadata {
        meta.insert(k.clone().into(), json_to_dynamic(v.clone()));
    }
    caller.insert("metadata".into(), meta.into());
    scope.push_dynamic("caller", caller.into());

    // Run the script. The script's return value becomes the cell's
    // data (or a side effect via set/push).
    let result = match engine.eval_with_scope::<Dynamic>(&mut scope, &code) {
        Ok(v) => v,
        Err(e) => return CellValue::err_with_stack(e.to_string(), format!("program:{}", cell.id)),
    };
    let data = dynamic_to_json(result);
    let duration = now_millis().saturating_sub(started_at);

    CellValue {
        data,
        status: CellStatus::Ready,
        computed_at: Some(now_millis()),
        error: None,
        effects: vec![crate::types::Effect::Compute { ms: duration }],
    }
}

// ---------------------------------------------------------------------------
// serde_json ↔ rhai conversion (mirrors formula.rs)
// ---------------------------------------------------------------------------

fn json_to_dynamic(v: Value) -> Dynamic {
    use rhai::{Array, Map};
    match v {
        Value::Null => Dynamic::UNIT,
        Value::Bool(b) => b.into(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.into()
            } else if let Some(f) = n.as_f64() {
                f.into()
            } else {
                Dynamic::UNIT
            }
        }
        Value::String(s) => s.into(),
        Value::Array(items) => items.into_iter().map(json_to_dynamic).collect::<Array>().into(),
        Value::Object(map) => {
            let mut m = Map::new();
            for (k, v) in map {
                m.insert(k.into(), json_to_dynamic(v));
            }
            m.into()
        }
    }
}

fn dynamic_to_json(d: Dynamic) -> Value {
    if d.is_unit() {
        return Value::Null;
    }
    if let Some(b) = d.as_bool().ok() {
        return Value::Bool(b);
    }
    if let Some(i) = d.as_int().ok() {
        return Value::Number(i.into());
    }
    if let Some(f) = d.as_float().ok() {
        if let Some(n) = serde_json::Number::from_f64(f) {
            return Value::Number(n);
        }
    }
    if let Some(s) = d.clone().into_string().ok() {
        return Value::String(s);
    }
    if let Some(arr) = d.clone().into_array().ok() {
        return Value::Array(arr.into_iter().map(dynamic_to_json).collect());
    }
    Value::Null
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CellDef, CellKind};

    fn program_cell(code: &str) -> Cell {
        Cell::new(CellDef {
            id: "p".into(),
            kind: CellKind::Program,
            code: Some(code.to_string()),
            ..Default::default()
        })
    }

    #[tokio::test]
    async fn returns_literal_object() {
        let cell = program_cell("return #{a: 1, b: \"two\"};");
        let v = evaluate_program(
            cell,
            CallerContext::default(),
            None,
            Arc::new(NullRuntime),
        )
        .await;
        assert_eq!(v.data, serde_json::json!({"a": 1, "b": "two"}));
    }

    #[tokio::test]
    async fn returns_number() {
        let cell = program_cell("return 42;");
        let v = evaluate_program(
            cell,
            CallerContext::default(),
            None,
            Arc::new(NullRuntime),
        )
        .await;
        assert_eq!(v.data, serde_json::json!(42));
    }

    #[tokio::test]
    async fn missing_code_errors() {
        let cell = Cell::new(CellDef {
            id: "p".into(),
            kind: CellKind::Program,
            ..Default::default()
        });
        let v = evaluate_program(
            cell,
            CallerContext::default(),
            None,
            Arc::new(NullRuntime),
        )
        .await;
        assert!(v.is_error());
    }

    #[tokio::test]
    async fn script_error_caught() {
        let cell = program_cell("this is not valid rhai; @#$%");
        let v = evaluate_program(
            cell,
            CallerContext::default(),
            None,
            Arc::new(NullRuntime),
        )
        .await;
        assert!(v.is_error());
    }

    #[tokio::test]
    async fn list_returns_array() {
        let cell = program_cell("let ids = list(); return ids.len();");
        let v = evaluate_program(
            cell,
            CallerContext::default(),
            None,
            Arc::new(NullRuntime),
        )
        .await;
        assert_eq!(v.data, serde_json::json!(0));
    }

    #[tokio::test]
    async fn caller_visible() {
        let cell = program_cell("return caller.row;");
        let mut ctx = CallerContext::default();
        ctx.row = Some(serde_json::json!("boat-1"));
        let v = evaluate_program(cell, ctx, None, Arc::new(NullRuntime)).await;
        assert_eq!(v.data, serde_json::json!("boat-1"));
    }

    #[tokio::test]
    async fn null_runtime_get_returns_idle() {
        let cell = program_cell("return get(\"missing\").data;");
        let v = evaluate_program(
            cell,
            CallerContext::default(),
            None,
            Arc::new(NullRuntime),
        )
        .await;
        // NullRuntime returns CellValue::default which has Null data.
        assert_eq!(v.data, serde_json::json!(null));
    }
}
