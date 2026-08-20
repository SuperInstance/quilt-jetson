//! # cells/sensor.rs
//!
//! Sensor cell evaluator — push-based streaming input.
//!
//! ## Role in the system
//!
//! A `sensor` cell is a window onto an external data source. The
//! runtime does not pull from the source — instead, an adapter (IMU
//! reader, LIDAR driver, ROS2 subscriber, simulated generator) calls
//! `engine.push(sensor_id, value)` to feed values in.
//!
//! The `source` field of the `CellDef` is a logical identifier that
//! the adapter knows how to interpret. Common patterns:
//!
//! - `"simulated"` — for tests and demo sheets
//! - `"imu:i2c-1:0x68"` — IMU on I2C bus 1, address 0x68
//! - `"nmea:/dev/ttyTHS1:115200"` — NMEA 0183 on a serial port
//! - `"lidar:/dev/ttyUSB0"` — RPLIDAR or similar on a USB serial
//! - `"camera:/dev/video0"` — V4L2 camera
//! - `"ros2:/cmd_vel"` — ROS2 topic
//!
//! ## Depends on
//!
//! - `crate::types` — `Cell`, `CellValue`, `CallerContext`.
//!
//! ## Used by
//!
//! - `crate::engine` — dispatches to this on `get` for `sensor` cells.

use crate::types::{now_millis, Cell, CellStatus, CellValue, CallerContext, Effect};

/// Evaluate a sensor cell. Returns the most recently pushed value.
/// If no value has been pushed yet, returns the `default` from the
/// `CellDef` (or `idle`/`Null` if no default was set).
pub fn evaluate_sensor(cell: &Cell, _ctx: &CallerContext) -> CellValue {
    cell.value.clone()
}

/// Build a `CellValue` for a freshly pushed sensor reading.
pub fn make_sensor_value(data: serde_json::Value) -> CellValue {
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
    fn make_sensor_value_is_ready() {
        let v = make_sensor_value(json!({"x": 1.0}));
        assert!(v.is_ready());
        assert_eq!(v.data, json!({"x": 1.0}));
    }

    #[test]
    fn evaluate_returns_pushed_value() {
        let mut cell = Cell::new(CellDef {
            id: "s".into(),
            kind: CellKind::Sensor,
            ..Default::default()
        });
        cell.value = make_sensor_value(json!(42));
        let v = evaluate_sensor(&cell, &CallerContext::default());
        assert_eq!(v.data, json!(42));
    }

    #[test]
    fn evaluate_returns_default_if_not_pushed() {
        let cell = Cell::new(CellDef {
            id: "s".into(),
            kind: CellKind::Sensor,
            default: Some(json!({"x": 0.0, "y": 0.0})),
            ..Default::default()
        });
        let v = evaluate_sensor(&cell, &CallerContext::default());
        assert_eq!(v.data, json!({"x": 0.0, "y": 0.0}));
    }

    #[test]
    fn evaluate_returns_idle_if_no_default() {
        let cell = Cell::new(CellDef {
            id: "s".into(),
            kind: CellKind::Sensor,
            ..Default::default()
        });
        let v = evaluate_sensor(&cell, &CallerContext::default());
        assert_eq!(v.status, CellStatus::Idle);
    }

    #[test]
    fn source_field_is_advisory() {
        // The runtime does not interpret `source` — adapters do.
        let cell = Cell::new(CellDef {
            id: "s".into(),
            kind: CellKind::Sensor,
            source: Some("ros2:/cmd_vel".into()),
            ..Default::default()
        });
        assert_eq!(cell.def.source.as_deref(), Some("ros2:/cmd_vel"));
    }
}
