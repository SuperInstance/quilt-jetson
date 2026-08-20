//! # ros2
//!
//! ROS2 bridge for the Jetson tier.
//!
//! ## Role in the system
//!
//! This module is the bridge between Quilt cells and the ROS2
//! ecosystem. ROS2 is the dominant robotics middleware; most
//! humanoid and mobile robots run ROS2 natively. The Jetson is
//! typically the on-board computer that hosts the perception and
//! planning nodes, so making it easy to wire Quilt sheets into a
//! ROS2 graph is a high-leverage feature.
//!
//! Two directions:
//!
//! - **Subscribe** — a ROS2 topic becomes a Quilt `sensor` cell.
//!   When a message arrives, the bridge calls `engine.push`.
//! - **Publish** — a Quilt `io` cell with `direction: out` becomes
//!   a ROS2 publisher. The bridge reads the cell's value and
//!   publishes it as a ROS2 message.
//!
//! ## Implementation
//!
//! The bridge is built on `rclrs`, a pure-Rust ROS2 client. ROS2
//! itself is a large install (the `ros-humble-desktop` package on
//! Ubuntu is 1+ GB). To keep the default build self-contained, the
//! `rclrs` dependency is **feature-gated** behind `--features ros2`.
//!
//! Without the feature, this module provides:
//!
//! - Type definitions (`Ros2Message`, `TopicInfo`).
//! - A `Ros2BridgeStub` that logs what it would do but doesn't
//!   connect to ROS2.
//! - The same public API — `subscribe`, `publish`, `start`, `stop`
//!   — so user code doesn't have to change when the feature is
//!   enabled.
//!
//! With the feature, the stub is replaced by the real
//! `rclrs`-backed implementation in `bridge.rs`.
//!
//! ## Depends on
//!
//! - `rclrs` (optional, behind `ros2` feature).
//! - `tokio` — for the subscription loop.
//!
//! ## Used by
//!
//! - The CLI binary's `serve` subcommand, when configured with a
//!   sheet that has `sensor` cells with `source: ros2:...`.

pub mod subscriber;
pub mod publisher;

pub use publisher::Ros2Publisher;
pub use subscriber::Ros2Subscriber;

/// A ROS2 message — abstracted as a JSON value. The bridge
/// serializes/deserializes to/from the appropriate ROS2 message
/// type based on the topic's declared type.
pub type Ros2Message = serde_json::Value;

/// Information about a ROS2 topic.
#[derive(Debug, Clone)]
pub struct TopicInfo {
    /// The topic name (e.g. `/cmd_vel`).
    pub name: String,
    /// The message type (e.g. `geometry_msgs/msg/Twist`).
    pub message_type: String,
    /// The QoS profile name (e.g. `"default"`, `"sensor_data"`).
    pub qos: String,
}

impl TopicInfo {
    /// Build a topic info from a `source` string in the form
    /// `ros2:/topic:name:qos`.
    pub fn from_source(source: &str) -> Option<Self> {
        // Format: ros2:/topic:MessageType[:qos]
        let stripped = source.strip_prefix("ros2:")?;
        let mut parts = stripped.splitn(3, ':');
        let name = parts.next()?.to_string();
        let message_type = parts.next()?.to_string();
        let qos = parts.next().unwrap_or("default").to_string();
        Some(Self {
            name,
            message_type,
            qos,
        })
    }
}

/// The ROS2 bridge. Holds the active subscribers and publishers
/// keyed by topic name. In the stub implementation, this is just a
/// registry; with the `ros2` feature, it holds an `rclrs::Context`.
pub struct Ros2Bridge {
    /// The bridge mode — stub (no ROS2) or live.
    mode: BridgeMode,
}

/// The bridge mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeMode {
    /// Stub mode — no actual ROS2 connection.
    Stub,
    /// Live mode — connected to a real ROS2 daemon.
    /// Only available with the `ros2` feature.
    Live,
}

impl Ros2Bridge {
    /// Create a new stub bridge. Useful for development on a
    /// machine without ROS2 installed.
    pub fn stub() -> Self {
        Self { mode: BridgeMode::Stub }
    }

    /// The bridge mode.
    pub fn mode(&self) -> BridgeMode {
        self.mode
    }

    /// True if the bridge is connected to a real ROS2 daemon.
    pub fn is_live(&self) -> bool {
        self.mode == BridgeMode::Live
    }

    /// Start a subscriber on the given topic. Returns the
    /// subscriber id.
    pub async fn subscribe(
        &self,
        topic: TopicInfo,
        on_message: tokio::sync::mpsc::UnboundedSender<Ros2Message>,
    ) -> crate::error::Result<String> {
        match self.mode {
            BridgeMode::Stub => {
                let id = format!("stub-sub-{}", crate::types::now_millis());
                // Spawn a no-op task that logs the subscription.
                tokio::spawn(async move {
                    tracing::info!(
                        "[stub-ros2] would subscribe to {} ({}). messages go to {:?}",
                        topic.name,
                        topic.message_type,
                        on_message
                    );
                });
                Ok(id)
            }
            BridgeMode::Live => {
                // With the ros2 feature enabled, the real
                // implementation lives in `bridge.rs` and uses
                // rclrs. Without the feature, the live path is
                // unreachable.
                #[cfg(feature = "ros2")]
                {
                    crate::ros2::bridge::subscribe_live(topic, on_message).await
                }
                #[cfg(not(feature = "ros2"))]
                {
                    Err(crate::error::Error::Config(
                        "live ROS2 bridge requires `--features ros2`".into(),
                    ))
                }
            }
        }
    }

    /// Publish a message on a topic. Returns the publisher id.
    pub async fn publish(
        &self,
        topic: TopicInfo,
        message: Ros2Message,
    ) -> crate::error::Result<String> {
        match self.mode {
            BridgeMode::Stub => {
                let id = format!("stub-pub-{}", crate::types::now_millis());
                tracing::debug!("[stub-ros2] would publish on {}: {}", topic.name, message);
                Ok(id)
            }
            BridgeMode::Live => {
                #[cfg(feature = "ros2")]
                {
                    crate::ros2::bridge::publish_live(topic, message).await
                }
                #[cfg(not(feature = "ros2"))]
                {
                    let _ = message;
                    Err(crate::error::Error::Config(
                        "live ROS2 bridge requires `--features ros2`".into(),
                    ))
                }
            }
        }
    }
}

#[cfg(feature = "ros2")]
pub mod bridge;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_info_from_source_basic() {
        let info = TopicInfo::from_source("ros2:/cmd_vel:geometry_msgs/msg/Twist").unwrap();
        assert_eq!(info.name, "/cmd_vel");
        assert_eq!(info.message_type, "geometry_msgs/msg/Twist");
        assert_eq!(info.qos, "default");
    }

    #[test]
    fn topic_info_with_qos() {
        let info = TopicInfo::from_source(
            "ros2:/imu/data:sensor_msgs/msg/Imu:sensor_data",
        )
        .unwrap();
        assert_eq!(info.name, "/imu/data");
        assert_eq!(info.message_type, "sensor_msgs/msg/Imu");
        assert_eq!(info.qos, "sensor_data");
    }

    #[test]
    fn topic_info_from_source_rejects_non_ros2() {
        assert!(TopicInfo::from_source("simulated").is_none());
        assert!(TopicInfo::from_source("i2c:/dev/i2c-1:0x68").is_none());
    }

    #[test]
    fn stub_bridge_mode() {
        let b = Ros2Bridge::stub();
        assert_eq!(b.mode(), BridgeMode::Stub);
        assert!(!b.is_live());
    }

    #[tokio::test]
    async fn stub_subscribe_returns_id() {
        let b = Ros2Bridge::stub();
        let info = TopicInfo::from_source("ros2:/cmd_vel:geometry_msgs/msg/Twist").unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let id = b.subscribe(info, tx).await.unwrap();
        assert!(!id.is_empty());
    }

    #[tokio::test]
    async fn stub_publish_returns_id() {
        let b = Ros2Bridge::stub();
        let info = TopicInfo::from_source("ros2:/cmd_vel:geometry_msgs/msg/Twist").unwrap();
        let id = b
            .publish(info, serde_json::json!({"linear": {"x": 1.0}}))
            .await
            .unwrap();
        assert!(!id.is_empty());
    }
}
