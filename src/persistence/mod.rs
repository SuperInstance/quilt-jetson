//! # persistence
//!
//! Local persistence for cell history.
//!
//! ## Role in the system
//!
//! The Jetson tier is local-first. Every successful cell evaluation
//! is recorded in a local SQLite database. The web UI uses this
//! history to draw time-series charts and the federation client
//! uses it to replay state when a remote tier reconnects.
//!
//! Two backends are provided:
//!
//! - `SqliteStore` (default) — async SQLite via `sqlx`. The
//!   recommended backend for the Jetson.
//! - `MemoryStore` (test-only) — an in-memory ring buffer, used by
//!   the unit tests to avoid the SQLite dependency.
//!
//! ## Depends on
//!
//! - `sqlx` — async SQLite.
//! - `serde_json` — for cell values.
//! - `crate::types` — `CellValue`.
//!
//! ## Used by
//!
//! - `crate::engine` — calls `store.append` after every successful
//!   evaluation.
//! - `crate::web` — the `/api/cell/:path/history` endpoint reads
//!   from the store.
//! - `crate::federation` — uses the store to replay state.

pub mod sqlite;

pub use sqlite::{CellHistoryRow, SqliteStore};

/// A history entry — a single cell value at a single point in time.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HistoryEntry {
    /// The cell id.
    pub cell_id: String,
    /// The value.
    pub value: serde_json::Value,
    /// When the value was computed (millis since epoch).
    pub timestamp: u64,
}

/// In-memory history store. Used by tests and as a fallback when
/// SQLite is not available.
#[derive(Debug, Default)]
pub struct MemoryStore {
    /// The history, in insertion order.
    entries: parking_lot::Mutex<Vec<HistoryEntry>>,
    /// Max number of entries per cell.
    capacity: usize,
}

impl MemoryStore {
    /// Create a new memory store with the given per-cell capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: parking_lot::Mutex::new(Vec::new()),
            capacity,
        }
    }

    /// Append a cell value.
    pub fn append(&self, cell_id: &str, value: &serde_json::Value) {
        let mut entries = self.entries.lock();
        entries.push(HistoryEntry {
            cell_id: cell_id.to_string(),
            value: value.clone(),
            timestamp: crate::types::now_millis(),
        });
        // Cap the total size; in practice, the per-cell cap is
        // enforced by filtering on read.
        let max = self.capacity * 1000;
        if entries.len() > max {
            let drop = entries.len() - max;
            entries.drain(0..drop);
        }
    }

    /// Read the history for a cell, most-recent last.
    pub fn history(&self, cell_id: &str, limit: usize) -> Vec<HistoryEntry> {
        let entries = self.entries.lock();
        entries
            .iter()
            .rev()
            .filter(|e| e.cell_id == cell_id)
            .take(limit)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    /// Total number of entries.
    pub fn len(&self) -> usize {
        self.entries.lock().len()
    }

    /// Is the store empty?
    pub fn is_empty(&self) -> bool {
        self.entries.lock().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn memory_store_append_and_read() {
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

    #[test]
    fn memory_store_history_limit() {
        let s = MemoryStore::new(100);
        for i in 0..10 {
            s.append("a", &json!(i));
        }
        let h = s.history("a", 3);
        assert_eq!(h.len(), 3);
        // The most-recent 3 values.
        assert_eq!(h[0].value, json!(7));
        assert_eq!(h[1].value, json!(8));
        assert_eq!(h[2].value, json!(9));
    }

    #[test]
    fn memory_store_is_empty() {
        let s = MemoryStore::new(10);
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
        s.append("a", &json!(1));
        assert!(!s.is_empty());
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn history_entry_serializable() {
        let e = HistoryEntry {
            cell_id: "a".into(),
            value: json!(42),
            timestamp: 1000,
        };
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains("\"cell_id\":\"a\""));
    }
}
