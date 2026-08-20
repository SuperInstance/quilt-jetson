//! # parser.rs
//!
//! YAML loader / serializer for the canonical Quilt sheet format.
//!
//! ## Role in the system
//!
//! Quilt sheets are stored as YAML. This file is the bridge between
//! "what humans write" and "what the engine consumes." It is the
//! **single source of truth** for the sheet format on the Jetson
//! tier. The TypeScript implementation in `quilt/packages/core/src/parser.ts`
//! and the Rust sync implementation in
//! `quilt-rust/packages/core/src/parser.rs` should produce
//! identical output for any valid sheet.
//!
//! ## Depends on
//!
//! - `serde_yaml` — YAML deserialization. We chose this over
//!   `serde_yml` because it has the most stable API surface and
//!   tracks serde releases closely.
//! - `crate::types` — `CellDef`, `CellKind`, `SheetDef`, etc.
//! - `crate::error` — `Error`, `Result`.
//!
//! ## Used by
//!
//! - `quilt-jetson` CLI binary — the `run` and `serve` subcommands
//!   load sheets from disk via this module.
//! - The web server's `/api/sheet` endpoint — loads and re-serializes
//!   sheets on demand.
//! - User code that wants to embed Quilt.
//!
//! ## Sheet format
//!
//! A sheet is a top-level object with metadata and a `cells` array.
//! Each cell has an `id`, a `kind`, and kind-specific fields:
//!
//! ```yaml
//! id: my-sheet
//! version: 1
//! title: My Sheet
//! description: |
//!   A short prose description of what this sheet does.
//! cells:
//!   - id: temperature
//!     kind: sensor
//!     source: i2c:/dev/i2c-1:0x68
//!   - id: too_hot
//!     kind: formula
//!     expr: =temperature > 80
//!   - id: alert
//!     kind: listener
//!     watch: [too_hot]
//!     action: notify
//! ```
//!
//! See `examples/` in the repo for real sheets.

use std::path::Path;

use crate::error::{Error, Result};
use crate::types::{CellDef, CellKind, RouteTarget, RouterRule, SheetDef};

// =============================================================================
// Public API
// =============================================================================

/// Parse a sheet from a YAML string.
///
/// Validates the sheet: checks for duplicate ids, kind-specific
/// required fields, and (basic) well-formedness.
pub fn parse_sheet(source: &str) -> Result<SheetDef> {
    let sheet: SheetDef = serde_yaml::from_str(source)?;
    validate_sheet(&sheet)?;
    Ok(sheet)
}

/// Parse a sheet from a YAML file. Convenience wrapper around
/// `parse_sheet`.
pub fn parse_sheet_file(path: impl AsRef<Path>) -> Result<SheetDef> {
    let source = std::fs::read_to_string(path.as_ref())?;
    parse_sheet(&source)
}

/// Serialize a sheet to a YAML string. The output is round-trippable:
/// parsing the output and re-serializing produces an equivalent sheet
/// (modulo formatting choices).
pub fn serialize_sheet(sheet: &SheetDef) -> Result<String> {
    serde_yaml::to_string(sheet).map_err(|e| Error::Yaml(e.to_string()))
}

/// Write a sheet to a YAML file.
pub fn serialize_sheet_file(sheet: &SheetDef, path: impl AsRef<Path>) -> Result<()> {
    let yaml = serialize_sheet(sheet)?;
    std::fs::write(path.as_ref(), yaml)?;
    Ok(())
}

/// Validate a sheet without parsing. Used by `parse_sheet` (so the
/// result is always valid) and exposed publicly for callers that
/// already have a `SheetDef` in hand.
pub fn validate_sheet(sheet: &SheetDef) -> Result<()> {
    // Check for duplicate ids.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for cell in &sheet.cells {
        if !seen.insert(cell.id.clone()) {
            return Err(Error::InvalidCellDef {
                id: cell.id.clone(),
                message: "duplicate cell id".to_string(),
            });
        }
    }

    // Check kind-specific required fields.
    for cell in &sheet.cells {
        validate_cell(cell)?;
    }

    // Check that listener.watch references exist.
    for cell in &sheet.cells {
        if cell.kind == CellKind::Listener {
            for w in &cell.watch {
                if !sheet.cells.iter().any(|c| &c.id == w) {
                    return Err(Error::InvalidCellDef {
                        id: cell.id.clone(),
                        message: format!("listener watch references undefined cell: {}", w),
                    });
                }
            }
        }
    }

    // Check that router rules reference existing cells.
    for cell in &sheet.cells {
        if cell.kind == CellKind::Router {
            for rule in &cell.rules {
                let target = match &rule.route {
                    RouteTarget::CellId(id) => id.clone(),
                    RouteTarget::Cell { cell: c, .. } => c.clone(),
                    RouteTarget::Value { .. } => continue,
                    RouteTarget::Model { .. } => continue,
                };
                if !sheet.cells.iter().any(|c| c.id == target) {
                    return Err(Error::InvalidCellDef {
                        id: cell.id.clone(),
                        message: format!("router rule target undefined: {}", target),
                    });
                }
            }
        }
    }

    Ok(())
}

// =============================================================================
// Cell-level validation
// =============================================================================

fn validate_cell(cell: &CellDef) -> Result<()> {
    let id = cell.id.trim();
    if id.is_empty() {
        return Err(Error::InvalidCellDef {
            id: cell.id.clone(),
            message: "cell id cannot be empty".to_string(),
        });
    }

    match cell.kind {
        CellKind::Value => {
            if cell.value.is_none() {
                return Err(Error::InvalidCellDef {
                    id: cell.id.clone(),
                    message: "value cell requires `value` field".to_string(),
                });
            }
        }
        CellKind::Formula => {
            if cell.expr.is_none() {
                return Err(Error::InvalidCellDef {
                    id: cell.id.clone(),
                    message: "formula cell requires `expr` field".to_string(),
                });
            }
        }
        CellKind::Api => {
            if cell.endpoint.is_none() {
                return Err(Error::InvalidCellDef {
                    id: cell.id.clone(),
                    message: "api cell requires `endpoint` field".to_string(),
                });
            }
        }
        CellKind::Program => {
            if cell.code.is_none() {
                return Err(Error::InvalidCellDef {
                    id: cell.id.clone(),
                    message: "program cell requires `code` field".to_string(),
                });
            }
        }
        CellKind::Sensor => {
            // source is optional; a "simulated" sensor has no source.
        }
        CellKind::Io => {
            if cell.port.is_none() {
                return Err(Error::InvalidCellDef {
                    id: cell.id.clone(),
                    message: "io cell requires `port` field".to_string(),
                });
            }
        }
        CellKind::Listener => {
            if cell.watch.is_empty() {
                return Err(Error::InvalidCellDef {
                    id: cell.id.clone(),
                    message: "listener cell requires non-empty `watch`".to_string(),
                });
            }
        }
        CellKind::Router => {
            if cell.rules.is_empty() {
                return Err(Error::InvalidCellDef {
                    id: cell.id.clone(),
                    message: "router cell requires non-empty `rules`".to_string(),
                });
            }
        }
    }
    Ok(())
}

// =============================================================================
// Helpers used in tests
// =============================================================================

/// Build a `CellDef` programmatically. Used by integration tests to
/// avoid writing YAML by hand.
pub fn cell(id: &str, kind: CellKind, build: impl FnOnce(&mut CellDef)) -> CellDef {
    let mut def = CellDef::default();
    def.id = id.to_string();
    def.kind = kind;
    build(&mut def);
    def
}

/// Build a `RouterRule` programmatically. Used by tests.
pub fn rule_to(target: &str, when: &str) -> RouterRule {
    RouterRule {
        when: when.to_string(),
        route: RouteTarget::CellId(target.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_minimal_value_cell() {
        let yaml = r#"
id: minimal
version: "1"
cells:
  - id: greeting
    kind: value
    value: hello
"#;
        let sheet = parse_sheet(yaml).unwrap();
        assert_eq!(sheet.cells.len(), 1);
        assert_eq!(sheet.cells[0].id, "greeting");
        assert_eq!(sheet.cells[0].kind, CellKind::Value);
    }

    #[test]
    fn parse_formula_cell() {
        let yaml = r#"
id: with-formula
version: "1"
cells:
  - id: a
    kind: value
    value: 10
  - id: b
    kind: value
    value: 20
  - id: sum
    kind: formula
    expr: =a + b
"#;
        let sheet = parse_sheet(yaml).unwrap();
        assert_eq!(sheet.cells.len(), 3);
    }

    #[test]
    fn rejects_duplicate_ids() {
        let yaml = r#"
id: dup
version: "1"
cells:
  - id: a
    kind: value
    value: 1
  - id: a
    kind: value
    value: 2
"#;
        let result = parse_sheet(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_value_cell_without_value() {
        let yaml = r#"
id: bad
version: "1"
cells:
  - id: x
    kind: value
"#;
        let result = parse_sheet(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_formula_cell_without_expr() {
        let yaml = r#"
id: bad
version: "1"
cells:
  - id: x
    kind: formula
"#;
        let result = parse_sheet(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_listener_with_undefined_watch() {
        let yaml = r#"
id: bad
version: "1"
cells:
  - id: l
    kind: listener
    watch: [nonexistent]
"#;
        let result = parse_sheet(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn parse_listener_with_defined_watch() {
        let yaml = r#"
id: ok
version: "1"
cells:
  - id: trigger
    kind: value
    value: false
  - id: l
    kind: listener
    watch: [trigger]
"#;
        let sheet = parse_sheet(yaml).unwrap();
        assert_eq!(sheet.cells.len(), 2);
    }

    #[test]
    fn parse_sensor_with_default() {
        let yaml = r#"
id: s
version: "1"
cells:
  - id: imu
    kind: sensor
    source: i2c:/dev/i2c-1:0x68
    default: {x: 0, y: 0, z: 9.8}
"#;
        let sheet = parse_sheet(yaml).unwrap();
        assert_eq!(sheet.cells.len(), 1);
    }

    #[test]
    fn parse_api_with_tensorrt_endpoint() {
        let yaml = r#"
id: detect
version: "1"
cells:
  - id: camera
    kind: sensor
    source: camera:/dev/video0
  - id: obstacles
    kind: api
    endpoint: tensorrt:///opt/models/yolov8n.engine
"#;
        let sheet = parse_sheet(yaml).unwrap();
        assert_eq!(sheet.cells.len(), 2);
    }

    #[test]
    fn round_trip() {
        let yaml = r#"
id: round-trip
version: "1"
title: Round Trip
cells:
  - id: n
    kind: value
    value: 42
"#;
        let sheet = parse_sheet(yaml).unwrap();
        let yaml2 = serialize_sheet(&sheet).unwrap();
        let sheet2 = parse_sheet(&yaml2).unwrap();
        assert_eq!(sheet.cells.len(), sheet2.cells.len());
        assert_eq!(sheet.cells[0].id, sheet2.cells[0].id);
    }

    #[test]
    fn cell_builder() {
        let c = cell("x", CellKind::Value, |d| {
            d.value = Some(json!(1));
        });
        assert_eq!(c.id, "x");
        assert_eq!(c.kind, CellKind::Value);
    }

    #[test]
    fn validate_sheet_idempotent() {
        let yaml = r#"
id: x
cells:
  - id: a
    kind: value
    value: 1
"#;
        let sheet = parse_sheet(yaml).unwrap();
        // Calling validate twice should still succeed.
        validate_sheet(&sheet).unwrap();
        validate_sheet(&sheet).unwrap();
    }

    #[test]
    fn parse_sheet_with_axes() {
        let yaml = r#"
id: grid
axes:
  rows: { name: boat }
  cols: { name: capability }
cells:
  - id: a
    kind: value
    value: 1
"#;
        let sheet = parse_sheet(yaml).unwrap();
        let axes = sheet.axes.as_ref().unwrap();
        assert_eq!(axes.rows.as_ref().unwrap().name, "boat");
        assert_eq!(axes.cols.as_ref().unwrap().name, "capability");
    }

    #[test]
    fn parse_invalid_yaml_errors() {
        let result = parse_sheet(":\n - : :");
        assert!(result.is_err());
    }
}
