# ROS2 Bridge

> Status: v0.1.0 — stub mode by default, real ROS2 with `--features ros2`.
> Pair with: [`architecture.md`](architecture.md).

The `quilt-jetson` ROS2 bridge turns ROS2 topics into Quilt cells.
A `sensor` cell with `source: ros2:/topic:MsgType` becomes a
ROS2 subscriber; an `io` cell with `port: ros2:/topic:MsgType`
becomes a ROS2 publisher. The same `quilt-jetson` engine that
serves your local cells also talks to the rest of your ROS2
graph.

## Why a ROS2 bridge?

ROS2 is the dominant robotics middleware. Most humanoid and
mobile robots run ROS2 natively; the perception and planning
nodes typically live on a Jetson-class on-board computer. By
making Quilt cells first-class participants in the ROS2 graph,
you can:

- Use ROS2 tooling (`rviz2`, `ros2 topic echo`, `ros2 bag`) to
  inspect what your Quilt sheet is doing.
- Compose Quilt logic with off-the-shelf ROS2 nodes
  (`nav2`, `slam_toolbox`, `depthai-ros`).
- Reuse existing ROS2 message types without writing a bridge
  per topic.

## The two directions

### Subscribe (ROS2 → Quilt)

A `sensor` cell with a `source` field that starts with `ros2:`
becomes a subscriber:

```yaml
- id: cmd_vel.in
  kind: sensor
  source: ros2:/cmd_vel:geometry_msgs/msg/Twist
  rate: 50
```

The bridge parses the `source` field:

- `ros2:` — the marker.
- `/cmd_vel` — the topic name.
- `geometry_msgs/msg/Twist` — the message type.
- (Optional) `qos_profile` — defaults to `"default"`.

When a message arrives, the bridge serializes it to JSON and
calls `engine.push(sensor_id, value)`. Downstream cells
(formulas, listeners) re-evaluate.

### Publish (Quilt → ROS2)

An `io` cell with `port: ros2:...` and `direction: out` becomes
a publisher:

```yaml
- id: cmd_vel.out
  kind: io
  port: ros2:/cmd_vel_smoothed:geometry_msgs/msg/Twist
  direction: out
```

The bridge watches the cell's value. When it changes, the
bridge deserializes the JSON to a ROS2 message and publishes
it on the topic.

## Feature-gating

The `rclrs` (pure-Rust ROS2 client) dependency is large and
requires ROS2 to be installed on the host. To keep the default
build self-contained, the bridge is **feature-gated**:

```bash
# Default build — stub mode (no ROS2, just logs what would happen)
cargo build --release

# With ROS2 support
cargo build --release --features ros2
```

In stub mode:

- The bridge logs what it would subscribe to / publish.
- Sensors and io cells work as `value` cells with the `default`
  field as their initial value.
- The rest of the sheet runs as normal.

This lets you develop and test sheets on a laptop without
ROS2, then deploy to a Jetson with the `--features ros2` build.

## Installing ROS2 on the Jetson

For the `ros2` feature to link, you need ROS2 installed (we
target ROS2 Humble on Ubuntu 22.04). The official install:

```bash
# ROS2 Humble (Ubuntu 22.04 / Jetson JetPack 5.x)
sudo apt install software-properties-common
sudo add-apt-repository universe
sudo apt update && sudo apt install curl -y
sudo curl -sSL https://raw.githubusercontent.com/ros/rosdistro/master/ros.key -o /usr/share/keyrings/ros-archive-keyring.gpg
echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/ros-archive-keyring.gpg] http://packages.ros.org/ros2/ubuntu $(. /etc/os-release && echo $UBUNTU_CODENAME) main" | sudo tee /etc/apt/sources.list.d/ros2.list > /dev/null
sudo apt update
sudo apt install ros-humble-desktop ros-humble-rclrs-msgs  # plus any message packages you need
```

After installation, source the setup script before building:

```bash
source /opt/ros/humble/setup.bash
cargo build --features ros2
```

## Example: boat autopilot

A three-tier autopilot that uses ROS2 to talk to the actuators:

```yaml
id: boat-autopilot
title: ROS2-based boat autopilot
version: 0.1.0

cells:
  # Inbound — the remote controller's twist messages.
  - id: cmd_vel.in
    kind: sensor
    source: ros2:/cmd_vel:geometry_msgs/msg/Twist
    rate: 50

  # Compute the rudder angle from the desired heading.
  - id: rudder.command
    kind: formula
    expr: |
      cmd_vel.in.angular.z * 0.5  # proportional
    unit: degrees

  # Outbound — publish the rudder command.
  - id: rudder.io
    kind: io
    port: ros2:/actuators/rudder:std_msgs/msg/Float32
    direction: out

  # The helm status — also published back.
  - id: helm.status
    kind: io
    port: ros2:/status/helm:std_msgs/msg/String
    direction: out
```

This sheet is the same one that runs on the ESP32 in
`quilt-esp32`; the difference is the `sensor` and `io` cells
now talk to ROS2 instead of the bare metal.

## QoS profiles

The third part of the `source` field is the QoS profile:

```yaml
source: ros2:/imu/data:sensor_msgs/msg/Imu:sensor_data
```

The default QoS profile is `"default"`. Other options include
`"sensor_data"`, `"parameters"`, `"services_default"`, and any
custom profile you've registered.

## Limitations

The bridge, in v0.1.0, supports only JSON-serializable message
types. ROS2 messages with custom binary fields (e.g. PointCloud2
binaries) need a custom serializer; this is on the roadmap.

For binary types, use a Quilt `program` cell to deserialize the
message into JSON in the script body. The runtime handle
exposes the raw bytes via `runtime.get("topic.raw")` if the
bridge provides them.
