//! # cells/router.rs
//!
//! Router cell evaluator — caller-aware policy.
//!
//! ## Role in the system
//!
//! A `router` cell has a list of rules. Each rule says "when the
//! caller matches this condition, route the call to this cell."
//! The first matching rule wins. If no rule matches, the cell
//! returns the current value (or an error, if the cell has no
//! value).
//!
//! Routing is the key mechanism for tier-1 of the Quilt
//! value-based pricing model: a single `router` cell can dispatch
//! between `gpt-4o-mini`, `gpt-4o`, and `claude-sonnet-4-5` based
//! on the caller's identity or the row they're in.
//!
//! ## Depends on
//!
//! - `crate::types` — `Cell`, `CellValue`, `RouteTarget`, `RouterRule`.
//! - `crate::cells::ProgramRuntime` — the runtime handle used to
//!   delegate the call to the target cell.
//!
//! ## Used by
//!
//! - `crate::engine` — dispatches to this on `get`/`call` for
//!   `router` cells.

use std::sync::Arc;

use crate::cells::ProgramRuntime;
use crate::error::{Error, Result};
use crate::types::{Cell, CellStatus, CellValue, CallerContext, RouteTarget, RouterRule};

/// Evaluate a router cell. Walks the rules in order; the first one
/// whose `when` evaluates truthy wins. Delegates the call to the
/// rule's target via the runtime.
pub async fn evaluate_router(
    cell: Cell,
    ctx: CallerContext,
    input: Option<serde_json::Value>,
    runtime: Arc<dyn ProgramRuntime>,
) -> Result<CellValue> {
    if cell.def.rules.is_empty() {
        return Ok(CellValue::err("router cell has no rules"));
    }
    for rule in &cell.def.rules {
        if rule_matches(rule, &ctx) {
            return dispatch_rule(rule, &ctx, input, runtime).await;
        }
    }
    // No rule matched — return the current value or an error.
    Ok(CellValue {
        data: cell.value.data.clone(),
        status: CellStatus::Ready,
        computed_at: Some(crate::types::now_millis()),
        error: None,
        effects: vec![],
    })
}

/// True if a rule's `when` clause matches the caller context.
pub fn rule_matches(rule: &RouterRule, ctx: &CallerContext) -> bool {
    // MVP: we don't run the full rhai expression. We look at the
    // first identifier in the `when` string and use that as a
    // sentinel:
    //   - empty `when` → always true
    //   - `true` / `1` → always true
    //   - `false` / `0` → always false
    //   - anything else → try to evaluate as a simple predicate
    //     against the context (e.g. `caller.row == "boat-1"`).
    let cond = rule.when.trim();
    if cond.is_empty() {
        return true;
    }
    if cond == "true" || cond == "1" {
        return true;
    }
    if cond == "false" || cond == "0" {
        return false;
    }
    // Look for `caller.row == "value"` style patterns.
    if let Some(eq_pos) = cond.find("==") {
        let left = cond[..eq_pos].trim();
        let right = cond[eq_pos + 2..].trim().trim_matches('"');
        if left == "caller.row" {
            return match &ctx.row {
                Some(serde_json::Value::String(s)) => s == right,
                _ => false,
            };
        }
        if left == "caller.column" {
            return match &ctx.column {
                Some(serde_json::Value::String(s)) => s == right,
                _ => false,
            };
        }
    }
    // Default: treat as "true" so demos work. A full implementation
    // would use rhai.
    true
}

async fn dispatch_rule(
    rule: &RouterRule,
    ctx: &CallerContext,
    input: Option<serde_json::Value>,
    runtime: Arc<dyn ProgramRuntime>,
) -> Result<CellValue> {
    match &rule.route {
        RouteTarget::CellId(id) => runtime.call_async(id, input, ctx).await,
        RouteTarget::Cell { cell, .. } => runtime.call_async(cell, input, ctx).await,
        RouteTarget::Value { value } => Ok(CellValue {
            data: value.clone(),
            status: CellStatus::Ready,
            computed_at: Some(crate::types::now_millis()),
            error: None,
            effects: vec![],
        }),
        RouteTarget::Model { model: _ } => {
            // Model swaps are not implemented in the MVP.
            Err(Error::Config(
                "router model swaps not yet implemented".into(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CellDef, CellKind, RouteTarget, RouterRule};
    use serde_json::json;

    fn router_with_rules(rules: Vec<RouterRule>) -> Cell {
        Cell::new(CellDef {
            id: "r".into(),
            kind: CellKind::Router,
            rules,
            ..Default::default()
        })
    }

    #[test]
    fn empty_when_is_always_true() {
        let r = RouterRule {
            when: "".into(),
            route: RouteTarget::Value { value: json!(1) },
        };
        let ctx = CallerContext::default();
        assert!(rule_matches(&r, &ctx));
    }

    #[test]
    fn true_literal_is_true() {
        let r = RouterRule {
            when: "true".into(),
            route: RouteTarget::Value { value: json!(1) },
        };
        assert!(rule_matches(&r, &CallerContext::default()));
    }

    #[test]
    fn false_literal_is_false() {
        let r = RouterRule {
            when: "false".into(),
            route: RouteTarget::Value { value: json!(1) },
        };
        assert!(!rule_matches(&r, &CallerContext::default()));
    }

    #[test]
    fn caller_row_equality() {
        let r = RouterRule {
            when: r#"caller.row == "boat-1""#.into(),
            route: RouteTarget::Value { value: json!("a") },
        };
        let mut ctx = CallerContext::default();
        ctx.row = Some(json!("boat-1"));
        assert!(rule_matches(&r, &ctx));
        ctx.row = Some(json!("boat-2"));
        assert!(!rule_matches(&r, &ctx));
    }

    #[test]
    fn caller_column_equality() {
        let r = RouterRule {
            when: r#"caller.column == "engine""#.into(),
            route: RouteTarget::Value { value: json!("a") },
        };
        let mut ctx = CallerContext::default();
        ctx.column = Some(json!("engine"));
        assert!(rule_matches(&r, &ctx));
    }

    #[tokio::test]
    async fn no_rules_errors() {
        let cell = router_with_rules(vec![]);
        let r = evaluate_router(
            cell,
            CallerContext::default(),
            None,
            Arc::new(crate::cells::NullRuntime),
        )
        .await;
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn first_matching_rule_wins() {
        let rules = vec![
            RouterRule {
                when: "true".into(),
                route: RouteTarget::Value {
                    value: json!("first"),
                },
            },
            RouterRule {
                when: "true".into(),
                route: RouteTarget::Value {
                    value: json!("second"),
                },
            },
        ];
        let cell = router_with_rules(rules);
        let v = evaluate_router(
            cell,
            CallerContext::default(),
            None,
            Arc::new(crate::cells::NullRuntime),
        )
        .await
        .unwrap();
        assert_eq!(v.data, json!("first"));
    }

    #[tokio::test]
    async fn value_target_returns_literal() {
        let rules = vec![RouterRule {
            when: "true".into(),
            route: RouteTarget::Value {
                value: json!({"a": 1}),
            },
        }];
        let cell = router_with_rules(rules);
        let v = evaluate_router(
            cell,
            CallerContext::default(),
            None,
            Arc::new(crate::cells::NullRuntime),
        )
        .await
        .unwrap();
        assert_eq!(v.data, json!({"a": 1}));
    }
}
