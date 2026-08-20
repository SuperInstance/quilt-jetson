//! # cells/formula.rs
//!
//! Formula cell evaluator — pure reactive computation.
//!
//! ## Role in the system
//!
//! Pure reactive computation. The expression references other cells
//! by id; the runtime auto-tracks dependencies and recomputes when
//! any of them change. Pure: no effects, same input → same output
//! (modulo caller context).
//!
//! ## Depends on
//!
//! - `crate::types` — `Cell`, `CellValue`, `CellStatus`, `CallerContext`,
//!   `CellId`.
//! - `rhai` — embedded scripting language, used to evaluate the
//!   expression. Rhai is sandboxed by default and we don't register
//!   any I/O packages, so a formula cell cannot escape.
//!
//! ## Used by
//!
//! - `crate::engine` — calls this on `get` for a `formula` cell, after
//!   refreshing dependencies.
//!
//! ## Key decisions
//!
//! - We use rhai instead of a hand-rolled DSL because rhai is a real
//!   expression language: `+`, `-`, `*`, `/`, `%`, `==`, `!=`, `>`,
//!   `<`, `&&`, `||`, ternary, function calls, array/map literals.
//! - Per-context memoization lives on the `Cell::context_cache` map.
//! - The `FormulaEngine` is a small wrapper that owns a rhai `Engine`
//!   and registers helpers. We construct one per evaluation because
//!   rhai engines are cheap; alternatively a single engine could be
//!   cached.

use std::collections::HashMap;

use rhai::{Array, Engine, Map, Scope, AST};
use serde_json::Value;

use crate::error::{Error, Result};
use crate::types::{now_millis, Cell, CellId, CellStatus, CellValue, CallerContext};

/// Build the context-key for per-context memoization. Exposed for
/// reuse by the engine.
pub fn context_key(ctx: &CallerContext) -> String {
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

/// An owned formula evaluator. Holds the compiled AST. Compiling once
/// and re-running is much cheaper than re-parsing on every call when a
/// formula is hot.
#[derive(Debug, Clone)]
pub struct FormulaEngine {
    /// The original source, for error messages.
    pub source: String,
    /// The compiled AST.
    pub ast: AST,
    /// The list of known cell ids that the engine pre-processes the
    /// expression for. Used at compile time to rewrite `id` →
    /// `cells["id"]` so the user can write `a + b` instead of
    /// `cells["a"] + cells["b"]`.
    pub known_ids: Vec<String>,
}

impl FormulaEngine {
    /// Compile a formula expression. Strips a leading `=` if present.
    ///
    /// `known_ids` is the list of cell ids that the user is allowed to
    /// reference by their bare name. Each occurrence of an id in the
    /// expression is rewritten to `cells["id"]` at compile time.
    pub fn compile(source: &str, known_ids: &[String]) -> Result<Self> {
        let body = source.strip_prefix('=').unwrap_or(source).trim().to_string();

        // Rewrite known ids to `cells["id"]` bracket access.
        let rewritten = rewrite_known_ids(&body, known_ids);

        let mut engine = Engine::new();
        register_helpers(&mut engine);
        let ast = engine
            .compile(&rewritten)
            .map_err(|e| Error::ScriptError {
                cell: "<compile>".into(),
                message: format!("could not compile formula: {e}"),
            })?;
        Ok(Self {
            source: source.to_string(),
            ast,
            known_ids: known_ids.to_vec(),
        })
    }

    /// Evaluate the compiled formula with a snapshot of cell values
    /// and a caller context.
    pub fn eval(&self, cell_values: &HashMap<CellId, Value>, ctx: &CallerContext) -> Result<Value> {
        let mut engine = Engine::new();
        register_helpers(&mut engine);

        let mut scope = Scope::new();

        // Build the `cells` object: a rhai map keyed by cell id.
        let mut cells_map = Map::new();
        for (id, value) in cell_values {
            cells_map.insert(id.as_str().into(), json_to_dynamic(value.clone()));
        }
        scope.push_dynamic("cells", cells_map.into());

        // Build the `caller` object.
        let mut caller = Map::new();
        caller.insert(
            "row".into(),
            json_to_dynamic(ctx.row.clone().unwrap_or(Value::Null)),
        );
        caller.insert(
            "column".into(),
            json_to_dynamic(ctx.column.clone().unwrap_or(Value::Null)),
        );
        caller.insert(
            "sheet".into(),
            json_to_dynamic(ctx
                .sheet
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null)),
        );
        if let Some(identity) = &ctx.identity {
            let mut id_map = Map::new();
            id_map.insert("id".into(), identity.id.clone().into());
            id_map.insert("type".into(), identity.kind.clone().into());
            let tags: Array = identity.tags.iter().cloned().map(|t| t.into()).collect();
            id_map.insert("tags".into(), tags.into());
            caller.insert("identity".into(), id_map.into());
        } else {
            caller.insert("identity".into(), rhai::Dynamic::UNIT);
        }
        let mut meta = Map::new();
        for (k, v) in &ctx.metadata {
            meta.insert(k.clone().into(), json_to_dynamic(v.clone()));
        }
        caller.insert("metadata".into(), meta.into());
        scope.push_dynamic("caller", caller.into());

        let result = engine
            .eval_ast_with_scope::<rhai::Dynamic>(&mut scope, &self.ast)
            .map_err(|e| Error::ScriptError {
                cell: "<formula>".into(),
                message: e.to_string(),
            })?;
        Ok(dynamic_to_json(result))
    }
}

/// Evaluate a formula cell. Looks up the per-context cache first, then
/// compiles + runs the expression against a snapshot of dependency
/// values.
pub fn evaluate_formula(
    cell: &Cell,
    cell_values: &HashMap<CellId, Value>,
    ctx: &CallerContext,
) -> CellValue {
    if cell.def.expr.is_none() {
        return CellValue::err("formula cell has no expr");
    }
    let expr = cell.def.expr.clone().unwrap();

    // Per-context cache.
    let key = context_key(ctx);
    if let Some(cached) = cell.context_cache.get(&key) {
        if cached.is_ready() && cached.error.is_none() {
            return cached.clone();
        }
    }

    let known_ids: Vec<String> = cell_values.keys().cloned().collect();

    let engine = match FormulaEngine::compile(&expr, &known_ids) {
        Ok(e) => e,
        Err(err) => {
            return CellValue::err(format!("compile error: {err}"));
        }
    };

    let result = engine.eval(cell_values, ctx);
    match result {
        Ok(v) => CellValue {
            data: v,
            status: CellStatus::Ready,
            computed_at: Some(now_millis()),
            error: None,
            effects: Vec::new(),
        },
        Err(err) => CellValue::err(format!("{err}")),
    }
}

// ---------------------------------------------------------------------------
// Helpers registered with the rhai engine
// ---------------------------------------------------------------------------

fn register_helpers(engine: &mut Engine) {
    engine.register_fn("abs", abs_i64_fn);
    engine.register_fn("abs", abs_f64_fn);
    engine.register_fn("min", min_i64_fn);
    engine.register_fn("min", min_f64_fn);
    engine.register_fn("min", min_array_fn);
    engine.register_fn("max", max_i64_fn);
    engine.register_fn("max", max_f64_fn);
    engine.register_fn("max", max_array_fn);
    engine.register_fn("clamp", clamp_i64_fn);
    engine.register_fn("clamp", clamp_f64_fn);
    engine.register_fn("clamp", clamp_array_fn);
}

fn abs_i64_fn(x: i64) -> i64 {
    x.abs()
}
fn abs_f64_fn(x: f64) -> f64 {
    x.abs()
}
fn min_i64_fn(a: i64, b: i64) -> i64 {
    a.min(b)
}
fn min_f64_fn(a: f64, b: f64) -> f64 {
    a.min(b)
}
fn max_i64_fn(a: i64, b: i64) -> i64 {
    a.max(b)
}
fn max_f64_fn(a: f64, b: f64) -> f64 {
    a.max(b)
}
fn clamp_i64_fn(n: i64, lo: i64, hi: i64) -> i64 {
    n.clamp(lo, hi)
}
fn clamp_f64_fn(n: f64, lo: f64, hi: f64) -> f64 {
    n.clamp(lo, hi)
}

fn min_array_fn(
    args: rhai::Array,
) -> std::result::Result<rhai::Dynamic, Box<rhai::EvalAltResult>> {
    let mut best: Option<f64> = None;
    for a in args {
        let n = a
            .as_float()
            .or_else(|_| a.as_int().map(|i| i as f64))
            .map_err(|e| {
                Box::new(rhai::EvalAltResult::ErrorRuntime(
                    e.to_string().into(),
                    rhai::Position::NONE,
                ))
            })?;
        best = Some(best.map_or(n, |b| b.min(n)));
    }
    best.map(rhai::Dynamic::from).ok_or_else(|| {
        Box::new(rhai::EvalAltResult::ErrorRuntime(
            "min() requires at least one argument".into(),
            rhai::Position::NONE,
        ))
    })
}
fn max_array_fn(
    args: rhai::Array,
) -> std::result::Result<rhai::Dynamic, Box<rhai::EvalAltResult>> {
    let mut best: Option<f64> = None;
    for a in args {
        let n = a
            .as_float()
            .or_else(|_| a.as_int().map(|i| i as f64))
            .map_err(|e| {
                Box::new(rhai::EvalAltResult::ErrorRuntime(
                    e.to_string().into(),
                    rhai::Position::NONE,
                ))
            })?;
        best = Some(best.map_or(n, |b| b.max(n)));
    }
    best.map(rhai::Dynamic::from).ok_or_else(|| {
        Box::new(rhai::EvalAltResult::ErrorRuntime(
            "max() requires at least one argument".into(),
            rhai::Position::NONE,
        ))
    })
}
fn clamp_array_fn(
    args: rhai::Array,
) -> std::result::Result<rhai::Dynamic, Box<rhai::EvalAltResult>> {
    if args.len() != 3 {
        return Err(Box::new(rhai::EvalAltResult::ErrorRuntime(
            "clamp(arr) needs [n, lo, hi]".into(),
            rhai::Position::NONE,
        )));
    }
    let n = args[0]
        .as_float()
        .or_else(|_| args[0].as_int().map(|i| i as f64))
        .map_err(|e| {
            Box::new(rhai::EvalAltResult::ErrorRuntime(
                e.to_string().into(),
                rhai::Position::NONE,
            ))
        })?;
    let lo = args[1]
        .as_float()
        .or_else(|_| args[1].as_int().map(|i| i as f64))
        .map_err(|e| {
            Box::new(rhai::EvalAltResult::ErrorRuntime(
                e.to_string().into(),
                rhai::Position::NONE,
            ))
        })?;
    let hi = args[2]
        .as_float()
        .or_else(|_| args[2].as_int().map(|i| i as f64))
        .map_err(|e| {
            Box::new(rhai::EvalAltResult::ErrorRuntime(
                e.to_string().into(),
                rhai::Position::NONE,
            ))
        })?;
    Ok(rhai::Dynamic::from(n.clamp(lo, hi)))
}

// ---------------------------------------------------------------------------
// serde_json ↔ rhai conversion
// ---------------------------------------------------------------------------

/// Convert a `serde_json::Value` into a `rhai::Dynamic`.
pub fn json_to_dynamic(v: Value) -> rhai::Dynamic {
    use rhai::{Array, Map};
    match v {
        Value::Null => rhai::Dynamic::UNIT,
        Value::Bool(b) => b.into(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.into()
            } else if let Some(f) = n.as_f64() {
                f.into()
            } else {
                rhai::Dynamic::UNIT
            }
        }
        Value::String(s) => s.into(),
        Value::Array(items) => {
            let arr: Array = items.into_iter().map(json_to_dynamic).collect();
            arr.into()
        }
        Value::Object(map) => {
            let mut m = Map::new();
            for (k, v) in map {
                m.insert(k.into(), json_to_dynamic(v));
            }
            m.into()
        }
    }
}

/// Inverse of `json_to_dynamic`.
pub fn dynamic_to_json(d: rhai::Dynamic) -> Value {
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
        let items: Vec<Value> = arr.into_iter().map(dynamic_to_json).collect();
        return Value::Array(items);
    }
    Value::Null
}

// ---------------------------------------------------------------------------
// Token-aware id rewriter
// ---------------------------------------------------------------------------

/// Rewrite occurrences of known cell ids in an expression to
/// `cells["id"]` bracket access. Idempotent: existing `cells["..."]`
/// blocks and string literals are not touched.
fn rewrite_known_ids(body: &str, known_ids: &[String]) -> String {
    if known_ids.is_empty() {
        return body.to_string();
    }

    let mut sorted: Vec<&str> = known_ids.iter().map(|s| s.as_str()).collect();
    sorted.sort_by_key(|s| std::cmp::Reverse(s.len()));

    let chars: Vec<char> = body.chars().collect();
    let mut out = String::with_capacity(body.len() + 64);
    let mut i = 0;
    while i < chars.len() {
        // Inside a string literal — copy through to closing quote.
        if chars[i] == '"' || chars[i] == '\'' {
            let quote = chars[i];
            out.push(chars[i]);
            i += 1;
            while i < chars.len() && chars[i] != quote {
                out.push(chars[i]);
                i += 1;
            }
            if i < chars.len() {
                out.push(chars[i]);
                i += 1;
            }
            continue;
        }
        // Inside an existing `cells[...]` block — copy through.
        if chars[i] == 'c' && i + 5 < chars.len() && &body[i..i + 5] == "cells" {
            if i + 5 < chars.len() && chars[i + 5] == '[' {
                out.push_str("cells[");
                i += 6;
                let mut depth = 1;
                while i < chars.len() && depth > 0 {
                    if chars[i] == '[' {
                        depth += 1;
                    } else if chars[i] == ']' {
                        depth -= 1;
                    }
                    out.push(chars[i]);
                    i += 1;
                }
                continue;
            }
        }
        // Try to match a known id at this position.
        let mut matched = false;
        for id in &sorted {
            let id_chars: Vec<char> = id.chars().collect();
            if i + id_chars.len() > chars.len() {
                continue;
            }
            let mut equal = true;
            for (j, c) in id_chars.iter().enumerate() {
                if chars[i + j] != *c {
                    equal = false;
                    break;
                }
            }
            if !equal {
                continue;
            }
            let left_ok = if i == 0 {
                true
            } else {
                let prev = chars[i - 1];
                !prev.is_alphanumeric() && prev != '_' && prev != '.'
            };
            let right_ok = if i + id_chars.len() == chars.len() {
                true
            } else {
                let next = chars[i + id_chars.len()];
                !next.is_alphanumeric() && next != '_' && next != '.'
            };
            if left_ok && right_ok {
                out.push_str(&format!("cells[\"{}\"]", id));
                i += id_chars.len();
                matched = true;
                break;
            }
        }
        if !matched {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CellDef, CellKind};
    use serde_json::json;

    fn make_formula_cell(expr: &str) -> Cell {
        Cell::new(CellDef {
            id: "f".into(),
            kind: CellKind::Formula,
            expr: Some(expr.to_string()),
            ..Default::default()
        })
    }

    fn run(expr: &str, deps: &[(&str, Value)]) -> CellValue {
        let cell = make_formula_cell(expr);
        let mut cell_values: HashMap<CellId, Value> = HashMap::new();
        for (id, v) in deps {
            cell_values.insert((*id).to_string(), v.clone());
        }
        evaluate_formula(&cell, &cell_values, &CallerContext::default())
    }

    #[test]
    fn simple_arithmetic() {
        let v = run("1 + 2", &[]);
        assert_eq!(v.data, json!(3));
    }

    #[test]
    fn references_cell_value() {
        let v = run("a + b", &[("a", json!(3)), ("b", json!(4))]);
        assert_eq!(v.data, json!(7));
    }

    #[test]
    fn references_via_cells_map() {
        let v = run(
            "cells[\"a\"] + cells[\"b\"]",
            &[("a", json!(3)), ("b", json!(4))],
        );
        assert_eq!(v.data, json!(7));
    }

    #[test]
    fn helper_clamp() {
        let v = run("clamp(temp, 0, 100)", &[("temp", json!(150))]);
        assert_eq!(v.data, json!(100));
    }

    #[test]
    fn caller_row_visible() {
        let cell = make_formula_cell("if caller.row > 10 { \"premium\" } else { \"basic\" }");
        let cell_values = HashMap::new();
        let mut ctx = CallerContext::default();
        ctx.row = Some(json!(5));
        let v1 = evaluate_formula(&cell, &cell_values, &ctx);
        let mut ctx2 = CallerContext::default();
        ctx2.row = Some(json!(50));
        let v2 = evaluate_formula(&cell, &cell_values, &ctx2);
        assert_eq!(v1.data, json!("basic"));
        assert_eq!(v2.data, json!("premium"));
    }

    #[test]
    fn missing_expr_returns_error() {
        let cell = Cell::new(CellDef {
            id: "f".into(),
            kind: CellKind::Formula,
            ..Default::default()
        });
        let v = evaluate_formula(&cell, &HashMap::new(), &CallerContext::default());
        assert!(v.is_error());
    }

    #[test]
    fn compile_error_caught() {
        let v = run("a +", &[("a", json!(1))]);
        // Rhai's parser should error.
        assert!(v.is_error());
    }

    #[test]
    fn json_dynamic_round_trip() {
        assert_eq!(json_to_dynamic(Value::Null), rhai::Dynamic::UNIT);
        assert_eq!(json_to_dynamic(Value::Bool(true)).as_bool().unwrap(), true);
        assert_eq!(json_to_dynamic(json!(42)).as_int().unwrap(), 42);
        assert_eq!(
            json_to_dynamic(json!(3.14)).as_float().unwrap(),
            3.14_f64
        );
        assert_eq!(json_to_dynamic(json!("x")).into_string().unwrap(), "x");
    }

    #[test]
    fn dynamic_to_json_round_trip() {
        assert_eq!(dynamic_to_json(rhai::Dynamic::UNIT), Value::Null);
        assert_eq!(dynamic_to_json(true.into()), Value::Bool(true));
        assert_eq!(dynamic_to_json(rhai::Dynamic::from(42_i64)), json!(42));
    }

    #[test]
    fn rewrite_preserves_string_literals() {
        let rewritten = rewrite_known_ids(r#""hello" + a"#, &["a".into()]);
        assert!(rewritten.contains("\"hello\""));
        assert!(rewritten.contains("cells[\"a\"]"));
    }

    #[test]
    fn rewrite_preserves_cells_brackets() {
        let rewritten = rewrite_known_ids(
            r#"cells["already_there"] + a"#,
            &["a".into(), "already_there".into()],
        );
        assert!(rewritten.contains("cells[\"already_there\"]"));
        // `a` (not already_there) is rewritten.
        assert!(rewritten.contains("cells[\"a\"]"));
    }

    #[test]
    fn rewrite_longest_first() {
        // "a.b" should be matched before "a".
        let rewritten = rewrite_known_ids("a.b + a", &["a".into(), "a.b".into()]);
        assert!(rewritten.contains("cells[\"a.b\"]"));
        // The bare "a" outside of "a.b" should also be rewritten.
        // (Our matcher is word-boundary aware so "a" inside "a.b" is
        // not matched — only the trailing "a" is.)
        assert!(rewritten.matches("cells[\"a\"]").count() == 1);
    }

    #[test]
    fn rewrite_empty_known_ids_passthrough() {
        let s = "1 + 2";
        assert_eq!(rewrite_known_ids(s, &[]), s);
    }

    #[test]
    fn context_keys_differ_for_different_metadata() {
        let mut a = CallerContext::default();
        a.metadata.insert("k".into(), json!("v1"));
        let mut b = CallerContext::default();
        b.metadata.insert("k".into(), json!("v2"));
        assert_ne!(context_key(&a), context_key(&b));
    }
}
