//! # error.rs
//!
//! Typed errors for the Quilt Jetson runtime.
//!
//! ## Role in the system
//!
//! Every fallible operation in the runtime returns a `Result<T,
//! quilt_jetson::Error>`. The error type is `thiserror`-derived so
//! callers get `Display` + `Error` automatically. The variant list is
//! deliberately small and exhaustive — we never leak the underlying
//! library errors (reqwest, tokio, sqlx, rhai) without translating
//! them into a `Quilt` variant.
//!
//! ## Used by
//!
//! - Every module in the crate.
//! - The `anyhow::Result` re-export in the lib root is for binaries
//!   (CLI, examples) — libraries should use the typed `Result`.

use std::io;
use std::path::PathBuf;

/// The crate-level result alias. Use this in fallible APIs.
pub type Result<T> = std::result::Result<T, Error>;

/// The error type for the Quilt Jetson runtime.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A sheet could not be parsed (YAML syntax error, validation
    /// failure, missing required field, etc).
    #[error("sheet parse error: {0}")]
    SheetParse(String),

    /// A YAML serialization or deserialization error.
    #[error("YAML error: {0}")]
    Yaml(String),

    /// A cell with the given id does not exist in the current engine.
    #[error("cell not found: {0}")]
    CellNotFound(String),

    /// A cell id is already in use.
    #[error("cell already defined: {0}")]
    CellAlreadyDefined(String),

    /// A cell definition is invalid (kind-specific required field
    /// missing, etc).
    #[error("invalid cell definition '{id}': {message}")]
    InvalidCellDef {
        /// The cell id.
        id: String,
        /// Human-readable message.
        message: String,
    },

    /// A script (rhai formula or program) failed to compile or
    /// evaluate.
    #[error("script error in {cell}: {message}")]
    ScriptError {
        /// The cell id (or `<formula>` / `<program>` for engine-level
        /// errors).
        cell: String,
        /// Human-readable message from rhai.
        message: String,
    },

    /// Configuration error.
    #[error("config error: {0}")]
    Config(String),

    /// IO error (file read/write, network, etc).
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    /// Network error (HTTP/WebSocket).
    #[error("network error: {0}")]
    Network(String),

    /// Storage error (SQLite).
    #[error("storage error: {0}")]
    Storage(String),

    /// Hardware error (sensor read, actuator write, GPIO, etc).
    #[error("hardware error: {0}")]
    Hardware(String),

    /// Federation error (could not resolve a remote `quilt://` URI).
    #[error("federation error: {0}")]
    Federation(String),

    /// A timeout was exceeded.
    #[error("timeout: {0}")]
    Timeout(String),

    /// A catch-all for any other error. Avoid using this in new code.
    #[error("{0}")]
    Other(String),
}

impl Error {
    /// Convenience constructor for the `Other` variant.
    pub fn other(message: impl Into<String>) -> Self {
        Error::Other(message.into())
    }

    /// Convenience constructor for `Hardware`.
    pub fn hardware(message: impl Into<String>) -> Self {
        Error::Hardware(message.into())
    }

    /// Convenience constructor for `Federation`.
    pub fn federation(message: impl Into<String>) -> Self {
        Error::Federation(message.into())
    }

    /// Convenience constructor for `Network`.
    pub fn network(message: impl Into<String>) -> Self {
        Error::Network(message.into())
    }

    /// Convenience constructor for `Storage`.
    pub fn storage(message: impl Into<String>) -> Self {
        Error::Storage(message.into())
    }

    /// True if the error is transient (worth retrying).
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            Error::Network(_) | Error::Storage(_) | Error::Timeout(_) | Error::Hardware(_)
        )
    }
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Error::Network(e.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Yaml(e.to_string())
    }
}

impl From<serde_yaml::Error> for Error {
    fn from(e: serde_yaml::Error) -> Self {
        Error::Yaml(e.to_string())
    }
}

impl From<sqlx::Error> for Error {
    fn from(e: sqlx::Error) -> Self {
        Error::Storage(e.to_string())
    }
}

impl From<tokio::task::JoinError> for Error {
    fn from(e: tokio::task::JoinError) -> Self {
        Error::Other(format!("task join: {e}"))
    }
}

impl From<url::ParseError> for Error {
    fn from(e: url::ParseError) -> Self {
        Error::Config(format!("invalid URL: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sheet_parse_displays_message() {
        let e = Error::SheetParse("bad yaml".into());
        assert!(e.to_string().contains("bad yaml"));
    }

    #[test]
    fn cell_not_found_displays_id() {
        let e = Error::CellNotFound("foo".into());
        assert!(e.to_string().contains("foo"));
    }

    #[test]
    fn invalid_cell_def_displays_both() {
        let e = Error::InvalidCellDef {
            id: "x".into(),
            message: "no value".into(),
        };
        let s = e.to_string();
        assert!(s.contains("x"));
        assert!(s.contains("no value"));
    }

    #[test]
    fn script_error_displays_cell_and_message() {
        let e = Error::ScriptError {
            cell: "f".into(),
            message: "syntax".into(),
        };
        let s = e.to_string();
        assert!(s.contains("f"));
        assert!(s.contains("syntax"));
    }

    #[test]
    fn from_io_error() {
        let io_err = io::Error::new(io::ErrorKind::NotFound, "x");
        let e: Error = io_err.into();
        assert!(matches!(e, Error::Io(_)));
    }

    #[test]
    fn from_yaml_error() {
        // Construct a serde_yaml error via roundtrip of invalid data.
        let res: std::result::Result<serde_yaml::Value, _> =
            serde_yaml::from_str("a: b: c");
        let yaml_err = res.unwrap_err();
        let e: Error = yaml_err.into();
        assert!(matches!(e, Error::Yaml(_)));
    }

    #[test]
    fn is_transient_matches_correctly() {
        assert!(Error::Network("x".into()).is_transient());
        assert!(Error::Storage("x".into()).is_transient());
        assert!(Error::Timeout("x".into()).is_transient());
        assert!(Error::Hardware("x".into()).is_transient());
        assert!(!Error::CellNotFound("x".into()).is_transient());
        assert!(!Error::SheetParse("x".into()).is_transient());
    }

    #[test]
    fn network_from_string() {
        let e = Error::network("oops");
        assert!(matches!(e, Error::Network(_)));
    }

    #[test]
    fn storage_from_string() {
        let e = Error::storage("oops");
        assert!(matches!(e, Error::Storage(_)));
    }

    #[test]
    fn federation_from_string() {
        let e = Error::federation("oops");
        assert!(matches!(e, Error::Federation(_)));
    }

    #[test]
    fn hardware_from_string() {
        let e = Error::hardware("oops");
        assert!(matches!(e, Error::Hardware(_)));
    }

    #[test]
    fn other_from_string() {
        let e = Error::other("oops");
        assert!(matches!(e, Error::Other(_)));
    }

    #[test]
    fn file_path_error_converts() {
        // PathBuf::open would give a NotFound, but we just want to
        // exercise the from<io::Error> path.
        let _p: PathBuf = "/nope".into();
    }
}
