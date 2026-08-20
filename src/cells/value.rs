//! # cells/value.rs
//!
//! Value cell evaluator — the simplest kind.
//!
//! ## Role in the system
//!
//! A `value` cell is static data. It has no dependencies and no
//! computation: the `CellDef::value` is the answer. The cell is
//! `Ready` from the moment it's loaded.
//!
//! ## Depends on
//!
//! - `crate::types` — `Cell`, `CellValue`, `CallerContext`.
//!
//! ## Used by
//!
//! - `crate::engine` — dispatches to this on `get` for `value` cells.

use crate::types::{Cell, CellValue, CallerContext};

/// Evaluate a value cell. Returns the static data from the cell
/// definition wrapped in a `Ready` `CellValue`.
///
/// The caller context is ignored — value cells are context-free.
pub fn evaluate_value(cell: &Cell, _ctx: &CallerContext) -> CellValue {
    match &cell.def.value {
        Some(v) => CellValue {
            data: v.clone(),
            status: crate::types::CellStatus::Ready,
            computed_at: Some(crate::types::now_millis()),
            error: None,
            effects: Vec::new(),
        },
        None => CellValue::err("value cell has no value"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CellDef, CellKind};
    use serde_json::json;

    #[test]
    fn returns_static_value() {
        let cell = Cell::new(CellDef {
            id: "v".into(),
            kind: CellKind::Value,
            value: Some(json!("hello")),
            ..Default::default()
        });
        let v = evaluate_value(&cell, &CallerContext::default());
        assert_eq!(v.data, json!("hello"));
        assert!(v.is_ready());
    }

    #[test]
    fn missing_value_returns_error() {
        let cell = Cell::new(CellDef {
            id: "v".into(),
            kind: CellKind::Value,
            ..Default::default()
        });
        let v = evaluate_value(&cell, &CallerContext::default());
        assert!(v.is_error());
    }

    #[test]
    fn numeric_value() {
        let cell = Cell::new(CellDef {
            id: "v".into(),
            kind: CellKind::Value,
            value: Some(json!(42)),
            ..Default::default()
        });
        let v = evaluate_value(&cell, &CallerContext::default());
        assert_eq!(v.data, json!(42));
    }

    #[test]
    fn object_value() {
        let cell = Cell::new(CellDef {
            id: "v".into(),
            kind: CellKind::Value,
            value: Some(json!({"a": 1, "b": "two"})),
            ..Default::default()
        });
        let v = evaluate_value(&cell, &CallerContext::default());
        assert_eq!(v.data, json!({"a": 1, "b": "two"}));
    }

    #[test]
    fn array_value() {
        let cell = Cell::new(CellDef {
            id: "v".into(),
            kind: CellKind::Value,
            value: Some(json!([1, 2, 3])),
            ..Default::default()
        });
        let v = evaluate_value(&cell, &CallerContext::default());
        assert_eq!(v.data, json!([1, 2, 3]));
    }

    #[test]
    fn null_value() {
        let cell = Cell::new(CellDef {
            id: "v".into(),
            kind: CellKind::Value,
            value: Some(json!(null)),
            ..Default::default()
        });
        let v = evaluate_value(&cell, &CallerContext::default());
        assert_eq!(v.data, json!(null));
        // null is still a valid value; the cell is ready.
        assert!(v.is_ready());
    }
}
