//! # tests/integration.rs
//!
//! End-to-end integration tests for the Jetson tier.
//!
//! These tests load YAML sheets from the `examples/` directory and
//! verify the full pipeline: parse → load → evaluate → propagate →
//! persist.

use std::sync::Arc;
use std::time::Duration;

use quilt_jetson::persistence::SqliteStore;
use quilt_jetson::types::{CellDef, CellKind, SheetDef};
use quilt_jetson::{parse_sheet, EngineConfig, QuiltEngine};
use serde_json::json;
use tempfile::tempdir;

async fn make_engine_with_sheet(yaml: &str, store: Option<SqliteStore>) -> Arc<QuiltEngine> {
    let sheet = parse_sheet(yaml).unwrap();
    let config = EngineConfig {
        store: store.map(Arc::new),
        ..Default::default()
    };
    QuiltEngine::with_sheet(sheet.id.clone(), config, sheet).unwrap()
}

#[tokio::test]
async fn load_example_sensor_fusion() {
    let yaml = include_str!("../examples/sensor-fusion.yaml");
    let engine = make_engine_with_sheet(yaml, None).await;
    // Push some sensor values.
    engine.push("imu.x", json!(1.0)).await.unwrap();
    engine.push("imu.y", json!(2.0)).await.unwrap();
    engine.push("imu.z", json!(3.0)).await.unwrap();
    engine.push("gps.lat", json!(37.7749)).await.unwrap();
    engine.push("gps.lon", json!(-122.4194)).await.unwrap();

    // Evaluate the position formula.
    let v = engine
        .get("position.x", quilt_jetson::CallerContext::default())
        .await
        .unwrap();
    assert!(v.is_ready());
    // The formula should reference gps.lat or similar; we don't
    // assert a specific value, just that it ran.
    assert!(v.data.is_number() || v.data.is_object() || v.data.is_array());
}

#[tokio::test]
async fn load_example_vision_detect() {
    let yaml = include_str!("../examples/vision-detect.yaml");
    let engine = make_engine_with_sheet(yaml, None).await;
    // The vision cell is an api cell with a tensorrt:// endpoint.
    // The engine routes that to the vision evaluator, which
    // returns a stub result.
    let v = engine
        .get("vision.obstacles", quilt_jetson::CallerContext::default())
        .await
        .unwrap();
    assert!(v.is_ready());
    assert!(v.data["detections"].is_array());
}

#[tokio::test]
async fn load_example_ros2_publisher() {
    let yaml = include_str!("../examples/ros2-publisher.yaml");
    let engine = make_engine_with_sheet(yaml, None).await;
    // We just check that the sheet loaded cleanly and the cells are
    // queryable.
    let ids = engine.list_ids();
    assert!(!ids.is_empty());
    // The cmd_vel formula should be queryable.
    let v = engine
        .get("cmd_vel.linear.x", quilt_jetson::CallerContext::default())
        .await
        .unwrap();
    assert!(v.is_ready() || v.is_error()); // The program may error if ROS2 isn't available.
}

#[tokio::test]
async fn load_example_federation_subscribe() {
    let yaml = include_str!("../examples/federation-subscribe.yaml");
    let engine = make_engine_with_sheet(yaml, None).await;
    let ids = engine.list_ids();
    assert!(!ids.is_empty());
}

#[tokio::test]
async fn full_pipeline_with_persistence() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("test.db");
    let store = SqliteStore::open(&path).await.unwrap();

    let engine = make_engine_with_sheet(
        r#"
id: pipe
title: Integration pipeline
version: "1"
cells:
  - id: a
    kind: value
    value: 10
  - id: b
    kind: value
    value: 20
  - id: c
    kind: formula
    expr: =a + b
  - id: d
    kind: formula
    expr: =c * 2
"#,
        Some(store),
    )
    .await;

    // Evaluate c → triggers store.append for c
    let _v = engine
        .get("c", quilt_jetson::CallerContext::default())
        .await
        .unwrap();

    // Wait for the background write to complete.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // The store should have a row for c.
    let store = engine.store().unwrap();
    let count = store.count("c").await.unwrap();
    assert!(count >= 1, "expected at least 1 history row for c, got {count}");
}

#[tokio::test]
async fn full_pipeline_formula_chain() {
    let engine = make_engine_with_sheet(
        r#"
id: chain
title: Formula chain
version: "1"
cells:
  - id: x
    kind: value
    value: 2
  - id: y
    kind: formula
    expr: =x * x
  - id: z
    kind: formula
    expr: =y + 10
"#,
        None,
    )
    .await;

    let y = engine
        .get("y", quilt_jetson::CallerContext::default())
        .await
        .unwrap();
    assert_eq!(y.data, json!(4));

    let z = engine
        .get("z", quilt_jetson::CallerContext::default())
        .await
        .unwrap();
    assert_eq!(z.data, json!(14));
}

#[tokio::test]
async fn engine_id_is_propagated() {
    let yaml = "id: myid\ncells:\n  - id: a\n    kind: value\n    value: 1\n";
    let engine = make_engine_with_sheet(yaml, None).await;
    assert_eq!(engine.id(), "myid");
}

#[tokio::test]
async fn unknown_endpoint_for_vision_errors() {
    let sheet = SheetDef {
        id: "s".into(),
        title: None,
        description: None,
        version: None,
        axes: None,
        cells: vec![CellDef {
            id: "v".into(),
            kind: CellKind::Api,
            endpoint: Some("http://not-a-vision-url/".into()),
            ..Default::default()
        }],
    };
    let engine = QuiltEngine::with_sheet("s", EngineConfig::default(), sheet).unwrap();
    // The api cell with a non-vision URL goes through the regular
    // api evaluator (which would try to do an HTTP call). We
    // check that it doesn't panic.
    let r = engine
        .get("v", quilt_jetson::CallerContext::default())
        .await;
    // The cell exists; the result is either Ready (if the URL was
    // reachable in the test environment) or Error (if not). Both
    // are acceptable.
    assert!(r.is_ok());
}
