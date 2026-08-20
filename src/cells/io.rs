//! # cells/io.rs
//!
//! I/O cell evaluator — bidirectional port.
//!
//! ## Role in the system
//!
//! An `io` cell represents a bidirectional communication channel.
//! The `port` field names the channel; the `direction` field says
//! whether this cell is reading (`in`), writing (`out`), or both.
//!
//! On Jetson, common `port` patterns:
//!
//! - `"ws://host:port/path"` — a WebSocket
//! - `"mqtt://broker:1883/topic"` — an MQTT pub/sub channel
//! - `"serial:/dev/ttyTHS1:115200"` — a serial port
//! - `"gpio:18"` — a GPIO pin (output only typically)
//! - `"can:can0:0x18FF1234"` — a CAN bus frame
//!
//! ## Depends on
//!
//! - `crate::types` — `Cell`, `CellValue`, `CallerContext`.
//!
//! ## Used by
//!
//! - `crate::engine` — dispatches to this on `get` for `io` cells.

use crate::types::{now_millis, Cell, CellStatus, CellValue, CallerContext, Effect};

/// Evaluate an I/O cell. Returns the most recently pushed value.
pub fn evaluate_io(cell: &Cell, _ctx: &CallerContext) -> CellValue {
    cell.value.clone()
}

/// Build a `CellValue` for a freshly pushed I/O event.
pub fn make_io_value(data: serde_json::Value) -> CellValue {
    CellValue {
        data,
        status: CellStatus::Ready,
        computed_at: Some(now_millis()),
        error: None,
        effects: vec![Effect::Compute { ms: 0 }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CellDef, CellKind};
    use serde_json::json;

    #[test]
    fn make_io_value_is_ready() {
        let v = make_io_value(json!({"msg": "hi"}));
        assert!(v.is_ready());
        assert_eq!(v.data, json!({"msg": "hi"}));
    }

    #[test]
    fn evaluate_returns_pushed_value() {
        let mut cell = Cell::new(CellDef {
            id: "io".into(),
            kind: CellKind::Io,
            port: Some("ws://localhost:9000".into()),
            direction: Some(crate::types::Direction::Bidirectional),
            ..Default::default()
        });
        cell.value = make_io_value(json!({"event": "tick"}));
        let v = evaluate_io(&cell, &CallerContext::default());
        assert_eq!(v.data, json!({"event": "tick"}));
    }

    #[test]
    fn evaluate_returns_idle_if_no_push() {
        let cell = Cell::new(CellDef {
            id: "io".into(),
            kind: CellKind::Io,
            port: Some("mqtt://broker/topic".into()),
            direction: Some(crate::types::Direction::In),
            ..Default::default()
        });
        let v = evaluate_io(&cell, &CallerContext::default());
        assert_eq!(v.status, CellStatus::Idle);
    }

    #[test]
    fn port_field_preserved() {
        let cell = Cell::new(CellDef {
            id: "io".into(),
            kind: CellKind::Io,
            port: Some("mqtt://broker:1883/topic".into()),
            direction: Some(crate::types::Direction::Out),
            ..Default::default()
        });
        assert_eq!(cell.def.port.as_deref(), Some("mqtt://broker:1883/topic"));
        assert_eq!(cell.def.direction, Some(crate::types::Direction::Out));
    }
}
