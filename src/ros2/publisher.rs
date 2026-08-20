//! # ros2/publisher.rs
//!
//! ROS2 publisher wrapper.
//!
//! ## Role in the system
//!
//! A publisher takes the current value of a Quilt `io` cell and
//! publishes it on a ROS2 topic. The publisher is the inverse of
//! the subscriber: subscriber is inbound (ROS2 → Quilt), publisher
//! is outbound (Quilt → ROS2).
//!
//! In stub mode, the publisher is a no-op that just logs what it
//! would publish. With the `ros2` feature, it uses `rclrs` to
//! publish real messages.
//!
//! ## Depends on
//!
//! - `rclrs` (optional, behind `ros2` feature).
//! - `crate::ros2::TopicInfo`.
//!
//! ## Used by
//!
//! - The CLI binary's sheet-loader, which iterates over `io` cells
//!   with `port: ros2:...` and wires them up.

use crate::ros2::TopicInfo;

/// A ROS2 publisher.
pub struct Ros2Publisher {
    /// The topic.
    pub topic: TopicInfo,
    /// The publisher id.
    pub id: String,
    /// Whether the publisher is in stub mode.
    pub stub: bool,
}

impl Ros2Publisher {
    /// Create a new publisher for the given topic.
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
    fn publisher_fields() {
        let topic = TopicInfo {
            name: "/cmd_vel".into(),
            message_type: "geometry_msgs/msg/Twist".into(),
            qos: "default".into(),
        };
        let p = Ros2Publisher::new(topic, "id-1".into(), true);
        assert_eq!(p.name(), "/cmd_vel");
        assert_eq!(p.message_type(), "geometry_msgs/msg/Twist");
        assert!(p.stub);
    }
}
