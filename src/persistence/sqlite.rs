//! # persistence/sqlite.rs
//!
//! Async SQLite store for cell history.
//!
//! ## Role in the system
//!
//! `SqliteStore` is the persistent backing store for cell values on
//! the Jetson. Every successful evaluation appends a row; the web
//! UI reads them back as time-series data.
//!
//! The store is opened with `sqlx::SqlitePool`. The schema is
//! created on first run. The store is `Send + Sync` and can be
//! shared across the engine and the web server.
//!
//! ## Schema
//!
//! ```sql
//! CREATE TABLE IF NOT EXISTS cell_history (
//!     id INTEGER PRIMARY KEY AUTOINCREMENT,
//!     cell_id TEXT NOT NULL,
//!     value TEXT NOT NULL,            -- JSON-encoded
//!     status TEXT NOT NULL,
//!     computed_at INTEGER NOT NULL,   -- millis since epoch
//!     effects TEXT                    -- JSON-encoded array
//! );
//! CREATE INDEX IF NOT EXISTS idx_cell_history_cell_id
//!     ON cell_history(cell_id, computed_at DESC);
//! ```
//!
//! ## Depends on
//!
//! - `sqlx` — async SQLite.
//!
//! ## Used by
//!
//! - `crate::engine` — `store.append` after every successful
//!   evaluation.
//! - `crate::web` — history view.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqlitePoolOptions, SqliteConnectOptions};
use sqlx::{Pool, Row, Sqlite};
use tokio::sync::Mutex;

use crate::types::CellValue;

/// A single row in the cell history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CellHistoryRow {
    /// The cell id.
    pub cell_id: String,
    /// The cell value (JSON-encoded).
    pub value: serde_json::Value,
    /// The status (`"ready"`, `"error"`, etc).
    pub status: String,
    /// When the value was computed (millis since epoch).
    pub computed_at: u64,
}

/// Async SQLite store. Cheap to clone (internally `Arc`).
#[derive(Clone)]
pub struct SqliteStore {
    pool: Pool<Sqlite>,
    write_lock: Arc<Mutex<()>>,
}

impl std::fmt::Debug for SqliteStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteStore").finish()
    }
}

impl SqliteStore {
    /// Open or create a SQLite database at the given path. Runs
    /// migrations on first call.
    pub async fn open(path: impl AsRef<std::path::Path>) -> crate::error::Result<Self> {
        let opts = SqliteConnectOptions::new()
            .filename(path.as_ref())
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await?;
        let store = Self {
            pool,
            write_lock: Arc::new(Mutex::new(())),
        };
        store.migrate().await?;
        Ok(store)
    }

    /// Open an in-memory SQLite database. Useful for tests.
    pub async fn in_memory() -> crate::error::Result<Self> {
        let opts = SqliteConnectOptions::new()
            .filename(":memory:")
            .create_if_missing(true);
        // Note: in-memory SQLite is per-connection. To get a shared
        // in-memory DB we need to use a single connection. For our
        // tests, a single pool with one connection is sufficient.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await?;
        let store = Self {
            pool,
            write_lock: Arc::new(Mutex::new(())),
        };
        store.migrate().await?;
        Ok(store)
    }

    async fn migrate(&self) -> crate::error::Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS cell_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                cell_id TEXT NOT NULL,
                value TEXT NOT NULL,
                status TEXT NOT NULL,
                computed_at INTEGER NOT NULL,
                effects TEXT
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_cell_history_cell_id
                ON cell_history(cell_id, computed_at DESC)
            "#,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Append a cell value to the history.
    pub async fn append(&self, cell_id: &str, value: &CellValue) -> crate::error::Result<()> {
        let value_str = serde_json::to_string(&value.data).unwrap_or_else(|_| "null".to_string());
        let effects_str = serde_json::to_string(&value.effects).unwrap_or_else(|_| "[]".to_string());
        let status_str = value.status.as_str();
        let computed_at = value.computed_at.unwrap_or(0);
        let _guard = self.write_lock.lock().await;
        sqlx::query(
            r#"
            INSERT INTO cell_history (cell_id, value, status, computed_at, effects)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
        )
        .bind(cell_id)
        .bind(value_str)
        .bind(status_str)
        .bind(computed_at as i64)
        .bind(effects_str)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Read the most recent N history rows for a cell. Most-recent
    /// first.
    pub async fn history(
        &self,
        cell_id: &str,
        limit: u32,
    ) -> crate::error::Result<Vec<CellHistoryRow>> {
        let rows = sqlx::query(
            r#"
            SELECT cell_id, value, status, computed_at
            FROM cell_history
            WHERE cell_id = ?1
            ORDER BY computed_at DESC
            LIMIT ?2
            "#,
        )
        .bind(cell_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let value_str: String = row.try_get("value")?;
            let value: serde_json::Value =
                serde_json::from_str(&value_str).unwrap_or(serde_json::Value::Null);
            out.push(CellHistoryRow {
                cell_id: row.try_get("cell_id")?,
                value,
                status: row.try_get("status")?,
                computed_at: row.try_get::<i64, _>("computed_at")? as u64,
            });
        }
        Ok(out)
    }

    /// Count the number of history rows for a cell.
    pub async fn count(&self, cell_id: &str) -> crate::error::Result<u64> {
        let row = sqlx::query("SELECT COUNT(*) as c FROM cell_history WHERE cell_id = ?1")
            .bind(cell_id)
            .fetch_one(&self.pool)
            .await?;
        let c: i64 = row.try_get("c")?;
        Ok(c as u64)
    }

    /// Delete all history rows for a cell.
    pub async fn clear(&self, cell_id: &str) -> crate::error::Result<()> {
        let _guard = self.write_lock.lock().await;
        sqlx::query("DELETE FROM cell_history WHERE cell_id = ?1")
            .bind(cell_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Get the underlying pool, for advanced use cases.
    pub fn pool(&self) -> &Pool<Sqlite> {
        &self.pool
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    #[tokio::test]
    async fn open_creates_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let _store = SqliteStore::open(&path).await.unwrap();
        assert!(path.exists());
    }

    #[tokio::test]
    async fn append_and_history() {
        let store = SqliteStore::in_memory().await.unwrap();
        let v1 = CellValue::ready(json!(1));
        let v2 = CellValue::ready(json!(2));
        store.append("a", &v1).await.unwrap();
        store.append("a", &v2).await.unwrap();
        let h = store.history("a", 10).await.unwrap();
        assert_eq!(h.len(), 2);
        // Most-recent first.
        assert_eq!(h[0].value, json!(2));
        assert_eq!(h[1].value, json!(1));
    }

    #[tokio::test]
    async fn history_limit() {
        let store = SqliteStore::in_memory().await.unwrap();
        for i in 0..10 {
            store
                .append("a", &CellValue::ready(json!(i)))
                .await
                .unwrap();
        }
        let h = store.history("a", 3).await.unwrap();
        assert_eq!(h.len(), 3);
        assert_eq!(h[0].value, json!(9));
    }

    #[tokio::test]
    async fn count_returns_zero_for_unknown_cell() {
        let store = SqliteStore::in_memory().await.unwrap();
        let c = store.count("unknown").await.unwrap();
        assert_eq!(c, 0);
    }

    #[tokio::test]
    async fn count_after_appends() {
        let store = SqliteStore::in_memory().await.unwrap();
        for _ in 0..5 {
            store
                .append("a", &CellValue::ready(json!(1)))
                .await
                .unwrap();
        }
        let c = store.count("a").await.unwrap();
        assert_eq!(c, 5);
    }

    #[tokio::test]
    async fn clear_removes_rows() {
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
    async fn append_error_value() {
        let store = SqliteStore::in_memory().await.unwrap();
        let v = CellValue::err("oops");
        store.append("a", &v).await.unwrap();
        let h = store.history("a", 10).await.unwrap();
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].status, "error");
    }

    #[tokio::test]
    async fn history_for_different_cells() {
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
    async fn history_row_serializable() {
        let r = CellHistoryRow {
            cell_id: "a".into(),
            value: json!(42),
            status: "ready".into(),
            computed_at: 1000,
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("\"cell_id\":\"a\""));
        assert!(s.contains("\"status\":\"ready\""));
    }
}
