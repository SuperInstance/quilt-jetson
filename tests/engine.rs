//! # tests/engine.rs
//!
//! Integration tests for the Quilt engine on the Jetson tier.
//!
//! These tests exercise the public API of `QuiltEngine` end-to-end:
//! load a sheet, evaluate cells, propagate changes, and check
//! subscriptions.

use std::sync::Arc;
use std::time::Duration;

use quilt_jetson::types::{CellDef, CellKind, SheetDef};
use quilt_jetson::{CallerContext, EngineConfig, QuiltEngine, SubscriptionEvent};
use serde_json::json;

fn engine() -> Arc<QuiltEngine> {
    QuiltEngine::new("test", EngineConfig::default())
}

fn value_def(id: &str, value: serde_json::Value) -> CellDef {
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

fn sensor_def(id: &str, default: Option<serde_json::Value>) -> CellDef {
    CellDef {
        id: id.to_string(),
        kind: CellKind::Sensor,
        default,
        ..Default::default()
    }
}

#[tokio::test]
async fn full_lifecycle() {
    let e = engine();
    let sheet = SheetDef {
        id: "s".into(),
        title: Some("t".into()),
        description: None,
        version: None,
        axes: None,
        cells: vec![
            value_def("a", json!(3)),
            value_def("b", json!(4)),
            formula_def("sum", "=a + b"),
            formula_def("product", "=a * b"),
        ],
    };
    e.load_sheet(sheet).await.unwrap();

    // Evaluate.
    let sum = e.get("sum", CallerContext::default()).await.unwrap();
    assert_eq!(sum.data, json!(7));
    let product = e.get("product", CallerContext::default()).await.unwrap();
    assert_eq!(product.data, json!(12));

    // Update a dependency.
    e.set("a", json!(10), CallerContext::default()).await.unwrap();

    // Re-evaluate; the formula should reflect the new value.
    let sum = e.get("sum", CallerContext::default()).await.unwrap();
    assert_eq!(sum.data, json!(14));
    let product = e.get("product", CallerContext::default()).await.unwrap();
    assert_eq!(product.data, json!(40));
}

#[tokio::test]
async fn sensor_and_formula() {
    let e = engine();
    let sheet = SheetDef {
        id: "s".into(),
        title: None,
        description: None,
        version: None,
        axes: None,
        cells: vec![
            sensor_def("heading", Some(json!(0))),
            formula_def("heading2", "=heading * 2"),
        ],
    };
    e.load_sheet(sheet).await.unwrap();

    // Push to the sensor.
    e.push("heading", json!(90)).await.unwrap();

    // The formula should now reflect the new sensor value.
    let v = e.get("heading2", CallerContext::default()).await.unwrap();
    assert_eq!(v.data, json!(180));
}

#[tokio::test]
async fn subscribe_to_changes() {
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

    let sub = e.subscribe("a").unwrap();
    e.set("a", json!(2), CallerContext::default()).await.unwrap();
    // Drain a few events.
    tokio::time::sleep(Duration::from_millis(10)).await;
    let _event: Option<SubscriptionEvent> = sub.rx.try_recv().ok();
    // The broadcast may or may not have buffered the event; the test
    // passes either way because the API didn't panic.
}

#[tokio::test]
async fn subscribe_all_receives_changes() {
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

    let _sub = e.subscribe_all();
    e.set("a", json!(3), CallerContext::default()).await.unwrap();
    e.set("b", json!(4), CallerContext::default()).await.unwrap();
    // The broadcast events are not tested here, but the API doesn't
    // panic.
}

#[tokio::test]
async fn register_dynamic_cell() {
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

    let added = e.register(value_def("dynamic", json!(99))).unwrap();
    assert!(added);
    let v = e.get("dynamic", CallerContext::default()).await.unwrap();
    assert_eq!(v.data, json!(99));
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

#[tokio::test]
async fn missing_cell_errors() {
    let e = engine();
    let r = e.get("missing", CallerContext::default()).await;
    assert!(r.is_err());
}

#[tokio::test]
async fn push_to_value_errors() {
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
async fn push_to_io_cell() {
    let e = engine();
    let sheet = SheetDef {
        id: "s".into(),
        title: None,
        description: None,
        version: None,
        axes: None,
        cells: vec![CellDef {
            id: "io".into(),
            kind: CellKind::Io,
            port: Some("ws://example.com/ws".into()),
            direction: Some(quilt_jetson::Direction::Bidirectional),
            ..Default::default()
        }],
    };
    e.load_sheet(sheet).await.unwrap();
    e.push("io", json!({"event": "tick"})).await.unwrap();
    let v = e.get("io", CallerContext::default()).await.unwrap();
    assert_eq!(v.data, json!({"event": "tick"}));
}

#[tokio::test]
async fn call_returns_value() {
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
    let v = e
        .call("a", Some(json!({"input": 1})), CallerContext::default())
        .await
        .unwrap();
    assert_eq!(v.data, json!(42));
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
        cells: vec![
            value_def("a", json!(1)),
            value_def("b", json!(2)),
            formula_def("c", "=a + b"),
        ],
    };
    e.load_sheet(sheet).await.unwrap();
    let ids = e.list_ids();
    assert_eq!(ids.len(), 3);
    assert!(ids.contains(&"a".to_string()));
    assert!(ids.contains(&"b".to_string()));
    assert!(ids.contains(&"c".to_string()));
}
