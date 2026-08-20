//! # web
//!
//! The Quilt Jetson web server.
//!
//! ## Role in the system
//!
//! Serves the local cell UI on port 8080 by default. The UI shows:
//!
//! - A list of sheets
//! - A list of cells in the current sheet
//! - Live values (via WebSocket subscription)
//! - A history view for each cell (from SQLite)
//! - A built-in YAML editor
//!
//! Endpoints:
//!
//! - `GET /` — the static web UI
//! - `GET /api/sheet` — the current sheet (JSON)
//! - `GET /api/cell/:id` — the current value of a cell
//! - `POST /api/cell/:id` — set a cell's value
//! - `GET /api/cell/:id/history` — the cell's recent history
//! - `GET /ws` — the WebSocket for live events
//! - `GET /api/meta` — engine metadata (id, cell count, etc)
//!
//! ## Depends on
//!
//! - `axum` — web framework.
//! - `tokio-tungstenite` — WebSocket.
//! - `crate::engine` — the engine to expose.
//! - `crate::persistence` — for the history endpoint.
//!
//! ## Used by
//!
//! - The CLI binary's `serve` subcommand.

pub mod routes;

pub use routes::{make_router, AppState, WebServer};

use std::net::SocketAddr;
use std::sync::Arc;

use tracing::info;

/// Configuration for the web server.
#[derive(Debug, Clone)]
pub struct WebConfig {
    /// The address to bind to. Default `0.0.0.0:8080`.
    pub bind: SocketAddr,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:8080".parse().expect("0.0.0.0:8080 is a valid address"),
        }
    }
}

/// Start the web server. Returns once the server stops (or fails).
pub async fn serve(engine: Arc<crate::QuiltEngine>, config: WebConfig) -> crate::error::Result<()> {
    let app = make_router(Arc::new(AppState::new(engine)));
    info!("Quilt Jetson web server listening on http://{}", config.bind);
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn default_bind() {
        let c = WebConfig::default();
        assert_eq!(c.bind.port(), 8080);
        assert_eq!(c.bind.ip(), Ipv4Addr::new(0, 0, 0, 0));
    }

    #[test]
    fn custom_bind() {
        let c = WebConfig {
            bind: "127.0.0.1:9000".parse().unwrap(),
        };
        assert_eq!(c.bind.port(), 9000);
    }
}
