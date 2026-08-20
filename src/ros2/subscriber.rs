//! # ros2/subscriber.rs
//!
//! ROS2 subscriber wrapper.
//!
//! ## Role in the system
//!
//! A subscriber turns a ROS2 topic into a stream of Quilt cell
//! values. When a message arrives, the subscriber's callback is
//! invoked; the callback typically calls
//! `engine.push(cell_id, value)` to feed the value into the cell
//! graph.
//!
//! In stub mode, the subscriber is a no-op that simply registers
//! the topic — useful for testing sheets on a machine without
//! ROS2.
//!
//! ## Depends on
//!
//! - `rclrs` (optional, behind `ros2` feature).
//! - `crate::ros2::TopicInfo`.
//!
//! ## Used by
//!
//! - The CLI binary's sheet-loader, which iterates over sensor
//!   cells with `source: ros2:...` and wires them up.

use crate::ros2::TopicInfo;

/// A ROS2 subscriber. The `callback` is invoked with the message
/// (as a `serde_json::Value`) every time a message arrives.
pub struct Ros2Subscriber {
    /// The topic.
    pub topic: TopicInfo,
    /// The subscriber id (returned by the bridge).
    pub id: String,
    /// Whether the subscriber is in stub mode.
    pub stub: bool,
}

impl Ros2Subscriber {
    /// Create a new subscriber for the given topic with the
    /// provided id.
    pub fn new(topic: TopicInfo, id: String, stub: bool) -> Self {
        Self { topic, id, stub }
    }

    /// The topic name.
    pub fn name(&self) -> &str {
        &self.topic.name
    }

    /// The message type.
    pub fn message_type(&self) -> &str {
        &self.topic.message_type
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscriber_fields() {
        let topic = TopicInfo {
            name: "/cmd_vel".into(),
            message_type: "geometry_msgs/msg/Twist".into(),
            qos: "default".into(),
        };
        let s = Ros2Subscriber::new(topic, "id-1".into(), true);
        assert_eq!(s.name(), "/cmd_vel");
        assert_eq!(s.message_type(), "geometry_msgs/msg/Twist");
        assert!(s.stub);
    }
}
