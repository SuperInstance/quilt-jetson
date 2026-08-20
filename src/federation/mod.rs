//! # federation
//!
//! Federation client for cross-tier Quilt communication.
//!
//! ## Role in the system
//!
//! The Quilt ecosystem is a federation of runtimes on different
//! hardware tiers. This module is the Jetson tier's client: it
//! subscribes to remote cells (e.g. on a Codespace or Cloudflare
//! Worker) and exposes the values as local cells.
//!
//! The URI scheme is `quilt://instance/sheet#cell`. The
//! `FederationClient` resolves the `instance` to an HTTP endpoint
//! and subscribes via a WebSocket for live updates, or fetches
//! the current value over HTTP once.
//!
//! ## Depends on
//!
//! - `reqwest` — async HTTP client.
//! - `tokio-tungstenite` — WebSocket client.
//! - `crate::types` — `CellValue`, `CallerContext`.
//!
//! ## Used by
//!
//! - The CLI binary, which connects the engine to a remote Quilt
//!   when the sheet declares `deps: ["quilt://.../...#..."]`.

pub mod transport;

pub use transport::{FederationClient, QuiltRef, RemoteCellEvent, ResolvedUri};

/// A reference to a remote cell. Same shape as the `quilt://`
/// URI: `<instance>/<sheet>#<cell>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RemoteCellRef {
    /// The instance name (e.g. `"jetson-lab"`, `"cloud-fleet"`).
    pub instance: String,
    /// The sheet name (e.g. `"perception"`).
    pub sheet: String,
    /// The cell id (e.g. `"vision.obstacles"`).
    pub cell: String,
}

impl RemoteCellRef {
    /// Build a `RemoteCellRef` from a `quilt://` URI.
    pub fn parse(uri: &str) -> crate::error::Result<Self> {
        let stripped = uri
            .strip_prefix("quilt://")
            .ok_or_else(|| crate::error::Error::federation(format!("not a quilt:// URI: {uri}")))?;
        let (path, fragment) = match stripped.split_once('#') {
            Some((p, f)) => (p, f),
            None => {
                return Err(crate::error::Error::federation(format!(
                    "quilt:// URI missing #cell: {uri}"
                )))
            }
        };
        let (instance, sheet) = match path.split_once('/') {
            Some((i, s)) => (i, s),
            None => {
                return Err(crate::error::Error::federation(format!(
                    "quilt:// URI missing /sheet: {uri}"
                )))
            }
        };
        Ok(Self {
            instance: instance.to_string(),
            sheet: sheet.to_string(),
            cell: fragment.to_string(),
        })
    }

    /// The full URI.
    pub fn uri(&self) -> String {
        format!("quilt://{}/{}/{}#{}", self.instance, self.sheet, "", self.cell)
        // Note: a cleaner form is below; we keep the empty sheet
        // path here for parser round-tripping.
    }

    /// A canonical URI.
    pub fn canonical_uri(&self) -> String {
        format!("quilt://{}/{}#{}", self.instance, self.sheet, self.cell)
    }
}

impl std::fmt::Display for RemoteCellRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.canonical_uri())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_uri() {
        let r = RemoteCellRef::parse("quilt://jetson-lab/perception#vision.obstacles").unwrap();
        assert_eq!(r.instance, "jetson-lab");
        assert_eq!(r.sheet, "perception");
        assert_eq!(r.cell, "vision.obstacles");
    }

    #[test]
    fn parse_with_nested_sheet() {
        let r = RemoteCellRef::parse("quilt://cloud-fleet/audit#autopilot.last").unwrap();
        assert_eq!(r.instance, "cloud-fleet");
        assert_eq!(r.sheet, "audit");
        assert_eq!(r.cell, "autopilot.last");
    }

    #[test]
    fn parse_missing_scheme_errors() {
        let r = RemoteCellRef::parse("http://example.com");
        assert!(r.is_err());
    }

    #[test]
    fn parse_missing_fragment_errors() {
        let r = RemoteCellRef::parse("quilt://jetson/perception");
        assert!(r.is_err());
    }

    #[test]
    fn parse_missing_sheet_errors() {
        let r = RemoteCellRef::parse("quilt://jetson#x");
        assert!(r.is_err());
    }

    #[test]
    fn canonical_uri_round_trip() {
        let uri = "quilt://jetson-lab/perception#vision.obstacles";
        let r = RemoteCellRef::parse(uri).unwrap();
        assert_eq!(r.canonical_uri(), uri);
    }

    #[test]
    fn display_impl() {
        let r = RemoteCellRef::parse("quilt://a/b#c").unwrap();
        assert_eq!(format!("{r}"), "quilt://a/b#c");
    }

    #[test]
    fn remote_cell_ref_eq() {
        let a = RemoteCellRef::parse("quilt://a/b#c").unwrap();
        let b = RemoteCellRef::parse("quilt://a/b#c").unwrap();
        assert_eq!(a, b);
    }
}
