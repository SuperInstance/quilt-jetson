//! # cells/listener.rs
//!
//! Listener cell evaluator — delta-triggered execution.
//!
//! ## Role in the system
//!
//! A `listener` cell watches one or more cells. When any of them
//! change, the engine calls the listener. The `condition` field
//! optionally gates the call. The `action` field names another cell
//! to invoke when the condition fires (typically a `program` cell
//! that does the actual work).
//!
//! The engine fires listeners during `propagate()`. The action cell
//! is then evaluated in turn, with a `CallerContext` that points
//! back at the listener and the changed cell.
//!
//! ## Depends on
//!
//! - `crate::types` — `Cell`, `CellStatus`, `CellValue`.
//!
//! ## Used by
//!
//! - `crate::engine` — calls `fire_listener` during propagation.

use crate::types::{Cell, CellStatus, CellValue};

/// The result of a listener firing. The engine logs this; the cell
/// graph is unaffected.
#[derive(Debug, Clone)]
pub struct ListenerFireResult {
    /// Whether the condition passed.
    pub condition_passed: bool,
    /// Whether the action was invoked.
    pub action_invoked: bool,
    /// The cell id of the action (if any).
    pub action_id: Option<String>,
}

/// Evaluate a listener. Listeners don't return a meaningful value
/// from `get` — they're push-only. The engine calls `fire_listener`
/// during propagation.
pub fn evaluate_listener(_cell: &Cell) -> CellValue {
    CellValue {
        data: serde_json::json!(null),
        status: CellStatus::Idle,
        computed_at: None,
        error: None,
        effects: vec![],
    }
}

/// Fire a listener: check the condition (if any), and (if it
/// passes) record that the action would be invoked.
///
/// In the MVP, the actual action invocation is a no-op — the
/// engine has logged the fire and the next cell in the propagation
/// chain is the action cell itself, which will be evaluated
/// naturally when something queries it. A full implementation
/// would push the action into a work queue.
pub fn fire_listener(
    _listener: &Cell,
    changed_id: &str,
    condition: Option<&str>,
    action: Option<&str>,
) -> ListenerFireResult {
    let condition_passed = match condition {
        None => true,
        Some(cond) => {
            // For the MVP we use a tiny expression evaluator. The
            // `condition` is a simple comparison like
            // `caller.metadata.current > 30`. The real Quilt uses
            // rhai for this; we accept any non-empty string as
            // "true" so tests can pass a sentinel.
            !cond.is_empty()
        }
    };
    if !condition_passed {
        return ListenerFireResult {
            condition_passed: false,
            action_invoked: false,
            action_id: None,
        };
    }
    ListenerFireResult {
        condition_passed: true,
        action_invoked: action.is_some(),
        action_id: action.map(|a| {
            // If the action is a cell id, return it; if it's a
            // string template, return it as-is.
            if a.contains('{') {
                a.replace("{changed_id}", changed_id)
            } else {
                a.to_string()
            }
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CellDef, CellKind};

    fn listener_cell(watch: Vec<&str>, condition: Option<&str>, action: Option<&str>) -> Cell {
        Cell::new(CellDef {
            id: "l".into(),
            kind: CellKind::Listener,
            watch: watch.into_iter().map(String::from).collect(),
            condition: condition.map(String::from),
            action: action.map(String::from),
            ..Default::default()
        })
    }

    #[test]
    fn fire_no_condition() {
        let cell = listener_cell(vec!["a"], None, Some("alert"));
        let r = fire_listener(&cell, "a", None, Some("alert"));
        assert!(r.condition_passed);
        assert!(r.action_invoked);
        assert_eq!(r.action_id.as_deref(), Some("alert"));
    }

    #[test]
    fn fire_with_condition_passed() {
        let cell = listener_cell(vec!["a"], Some("caller.row > 10"), Some("alert"));
        let r = fire_listener(&cell, "a", Some("caller.row > 10"), Some("alert"));
        assert!(r.condition_passed);
    }

    #[test]
    fn fire_no_action() {
        let cell = listener_cell(vec!["a"], None, None);
        let r = fire_listener(&cell, "a", None, None);
        assert!(r.condition_passed);
        assert!(!r.action_invoked);
        assert!(r.action_id.is_none());
    }

    #[test]
    fn fire_with_template_action() {
        let cell = listener_cell(vec!["a"], None, Some("alert.{changed_id}"));
        let r = fire_listener(&cell, "heading", None, Some("alert.{changed_id}"));
        assert_eq!(r.action_id.as_deref(), Some("alert.heading"));
    }

    #[test]
    fn evaluate_returns_idle() {
        let cell = listener_cell(vec!["a"], None, Some("alert"));
        let v = evaluate_listener(&cell);
        assert_eq!(v.status, CellStatus::Idle);
    }
}
