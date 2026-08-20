//! # tests/persistence.rs
//!
//! Integration tests for the SQLite persistence layer.

use quilt_jetson::persistence::{MemoryStore, SqliteStore};
use quilt_jetson::types::CellValue;
use serde_json::json;
use tempfile::tempdir;

#[tokio::test]
async fn sqlite_persists_values() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("test.db");
    let store = SqliteStore::open(&path).await.unwrap();
    store
        .append("a", &CellValue::ready(json!(1)))
        .await
        .unwrap();
    store
        .append("a", &CellValue::ready(json!(2)))
        .await
        .unwrap();
    let h = store.history("a", 10).await.unwrap();
    assert_eq!(h.len(), 2);
}

#[tokio::test]
async fn sqlite_counts_correct() {
    let store = SqliteStore::in_memory().await.unwrap();
    for _ in 0..5 {
        store
            .append("a", &CellValue::ready(json!(1)))
            .await
            .unwrap();
    }
    assert_eq!(store.count("a").await.unwrap(), 5);
}

#[tokio::test]
async fn memory_store_roundtrip() {
    let s = MemoryStore::new(100);
    s.append("a", &json!(1));
    s.append("a", &json!(2));
    s.append("b", &json!("x"));
    let h = s.history("a", 10);
    assert_eq!(h.len(), 2);
    assert_eq!(h[0].value, json!(1));
    assert_eq!(h[1].value, json!(2));
    let h = s.history("b", 10);
    assert_eq!(h.len(), 1);
}

#[tokio::test]
async fn sqlite_clears() {
    let store = SqliteStore::in_memory().await.unwrap();
    store
        .append("a", &CellValue::ready(json!(1)))
        .await
        .unwrap();
    assert_eq!(store.count("a").await.unwrap(), 1);
    store.clear("a").await.unwrap();
    assert_eq!(store.count("a").await.unwrap(), 0);
}

#[tokio::test]
async fn sqlite_error_value_persisted() {
    let store = SqliteStore::in_memory().await.unwrap();
    let v = CellValue::err("oops");
    store.append("a", &v).await.unwrap();
    let h = store.history("a", 10).await.unwrap();
    assert_eq!(h.len(), 1);
    assert_eq!(h[0].status, "error");
}

#[tokio::test]
async fn sqlite_history_isolated_by_cell() {
    let store = SqliteStore::in_memory().await.unwrap();
    store
        .append("a", &CellValue::ready(json!(1)))
        .await
        .unwrap();
    store
        .append("b", &CellValue::ready(json!(2)))
        .await
        .unwrap();
    let ha = store.history("a", 10).await.unwrap();
    let hb = store.history("b", 10).await.unwrap();
    assert_eq!(ha.len(), 1);
    assert_eq!(hb.len(), 1);
    assert_eq!(ha[0].value, json!(1));
    assert_eq!(hb[0].value, json!(2));
}

#[tokio::test]
async fn sqlite_limit_respected() {
    let store = SqliteStore::in_memory().await.unwrap();
    for i in 0..10 {
        store
            .append("a", &CellValue::ready(json!(i)))
            .await
            .unwrap();
    }
    let h = store.history("a", 3).await.unwrap();
    assert_eq!(h.len(), 3);
    // Most recent first.
    assert_eq!(h[0].value, json!(9));
}
