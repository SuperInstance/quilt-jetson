//! # tests/cells.rs
//!
//! Integration tests for the cell evaluators.
//!
//! These tests drive the cell evaluators directly (without going
//! through the engine) to make sure each kind produces the right
//! shape of result.

use quilt_jetson::cells::{
    evaluate_api, evaluate_formula, evaluate_listener, evaluate_program, evaluate_router,
    evaluate_sensor, evaluate_value, evaluate_vision, fire_listener, make_io_value,
    make_sensor_value, NullRuntime, StubExecutor, ApiResponse,
};
use quilt_jetson::types::{Cell, CellDef, CellKind, RouteTarget, RouterRule};
use serde_json::json;
use std::sync::Arc;

fn value_cell(id: &str, v: serde_json::Value) -> Cell {
    Cell::new(CellDef {
        id: id.to_string(),
        kind: CellKind::Value,
        value: Some(v),
        ..Default::default()
    })
}

fn formula_cell(id: &str, expr: &str) -> Cell {
    Cell::new(CellDef {
        id: id.to_string(),
        kind: CellKind::Formula,
        expr: Some(expr.to_string()),
        ..Default::default()
    })
}

#[test]
fn value_returns_its_data() {
    let cell = value_cell("a", json!("hi"));
    let v = evaluate_value(&cell, &quilt_jetson::CallerContext::default());
    assert_eq!(v.data, json!("hi"));
}

#[test]
fn formula_basic_arithmetic() {
    let cell = formula_cell("f", "1 + 2");
    let mut snapshot = std::collections::HashMap::new();
    let v = evaluate_formula(&cell, &snapshot, &quilt_jetson::CallerContext::default());
    assert_eq!(v.data, json!(3));
    snapshot.insert("a".into(), json!(10));
    snapshot.insert("b".into(), json!(20));
    let cell = formula_cell("f", "=a + b");
    let v = evaluate_formula(&cell, &snapshot, &quilt_jetson::CallerContext::default());
    assert_eq!(v.data, json!(30));
}

#[test]
fn sensor_returns_pushed_value() {
    let mut cell = value_cell("s", json!(0));
    cell.def.kind = CellKind::Sensor;
    cell.value = make_sensor_value(json!(42));
    let v = evaluate_sensor(&cell, &quilt_jetson::CallerContext::default());
    assert_eq!(v.data, json!(42));
}

#[test]
fn io_returns_pushed_value() {
    let mut cell = value_cell("io", json!(0));
    cell.def.kind = CellKind::Io;
    cell.value = make_io_value(json!("x"));
    let v = quilt_jetson::cells::evaluate_io(&cell, &quilt_jetson::CallerContext::default());
    assert_eq!(v.data, json!("x"));
}

#[tokio::test]
async fn api_stub_returns_canned() {
    let stub = Arc::new(StubExecutor {
        response: ApiResponse {
            status: 200,
            status_text: "OK".into(),
            headers: Default::default(),
            body: json!({"hello": "world"}),
        },
    });
    let cell = Cell::new(CellDef {
        id: "api".into(),
        kind: CellKind::Api,
        endpoint: Some("https://example.com/".into()),
        ..Default::default()
    });
    let v = evaluate_api(
        cell,
        quilt_jetson::CallerContext::default(),
        None,
        Some(stub),
    )
    .await;
    assert_eq!(v.data, json!({"hello": "world"}));
}

#[tokio::test]
async fn vision_stub_returns_detections() {
    let cell = Cell::new(CellDef {
        id: "v".into(),
        kind: CellKind::Api,
        endpoint: Some("tensorrt:///opt/yolo.engine".into()),
        ..Default::default()
    });
    let v = evaluate_vision(cell, quilt_jetson::CallerContext::default()).await;
    assert!(v.is_ready());
    assert!(v.data["detections"].is_array());
}

#[tokio::test]
async fn program_returns_value() {
    let cell = Cell::new(CellDef {
        id: "p".into(),
        kind: CellKind::Program,
        code: Some("return 42;".into()),
        ..Default::default()
    });
    let v = evaluate_program(
        cell,
        quilt_jetson::CallerContext::default(),
        None,
        Arc::new(NullRuntime),
    )
    .await;
    assert_eq!(v.data, json!(42));
}

#[tokio::test]
async fn program_can_call_runtime() {
    let cell = Cell::new(CellDef {
        id: "p".into(),
        kind: CellKind::Program,
        code: Some("let ids = list(); return ids.len();".into()),
        ..Default::default()
    });
    let v = evaluate_program(
        cell,
        quilt_jetson::CallerContext::default(),
        None,
        Arc::new(NullRuntime),
    )
    .await;
    assert_eq!(v.data, json!(0));
}

#[test]
fn listener_fires() {
    let cell = Cell::new(CellDef {
        id: "l".into(),
        kind: CellKind::Listener,
        watch: vec!["a".into()],
        condition: None,
        action: Some("alert".into()),
        ..Default::default()
    });
    let r = fire_listener(&cell, "a", None, Some("alert"));
    assert!(r.condition_passed);
    assert!(r.action_invoked);
    assert_eq!(r.action_id.as_deref(), Some("alert"));
}

#[test]
fn listener_evaluate_returns_idle() {
    let cell = Cell::new(CellDef {
        id: "l".into(),
        kind: CellKind::Listener,
        watch: vec!["a".into()],
        ..Default::default()
    });
    let v = evaluate_listener(&cell);
    assert_eq!(v.status, quilt_jetson::CellStatus::Idle);
}

#[tokio::test]
async fn router_dispatches_to_first_match() {
    let cell = Cell::new(CellDef {
        id: "r".into(),
        kind: CellKind::Router,
        rules: vec![
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
        ],
        ..Default::default()
    });
    let v = evaluate_router(
        cell,
        quilt_jetson::CallerContext::default(),
        None,
        Arc::new(NullRuntime),
    )
    .await
    .unwrap();
    assert_eq!(v.data, json!("first"));
}
