# Architecture — How `quilt-jetson` fits in the federation

> Status: v0.1.0
> Pair with: [`federation.md`](federation.md), [`ros2-bridge.md`](ros2-bridge.md), [`vision-cells.md`](vision-cells.md).

The Quilt ecosystem is a federation of runtimes on different
hardware tiers, all reading the same YAML sheet format and all
talking to each other over the `quilt://` URI scheme. The three
production tiers are:

```
┌──────────────────────────────────────────────────────────────┐
│                  Quilt federation tiers                       │
│                                                               │
│   ┌────────────┐    ┌────────────────┐    ┌────────────┐     │
│   │  ESP32     │    │  Jetson        │    │  Cloud     │     │
│   │  (edge)    │    │  (edge-mid)    │    │  (server)  │     │
│   │            │    │                │    │            │     │
│   │ no_std     │    │ full Linux     │    │ Workers /  │     │
│   │ tight loop │    │ tokio + CUDA   │    │ Codespace  │     │
│   │ sensors    │    │ ROS2 + vision  │    │ always-on  │     │
│   │ motors     │    │ SQLite + HTTP  │    │  audit     │     │
│   └─────┬──────┘    └────────┬───────┘    └─────┬──────┘     │
│         │                    │                   │           │
│         └────────────────────┴───────────────────┘           │
│                              │                                │
│                  quilt:// URI scheme + WebSocket federation   │
│                              │                                │
└──────────────────────────────┼────────────────────────────────┘
                               │
                  ┌────────────┴────────────┐
                  │  Shared sheet format    │
                  │  (YAML, 8 cell kinds)   │
                  └─────────────────────────┘
```

## Where `quilt-jetson` sits

`quilt-jetson` is the **mid-tier**. It has more capability than the
ESP32 (full Linux, tokio, GPU, persistent storage) but is closer to
the data than the cloud (low-latency sensor access, no public-IP
round-trip, no per-call billing).

| Capability                  | ESP32 | **Jetson** | Codespace | Cloudflare |
|-----------------------------|-------|------------|-----------|------------|
| Async runtime               | ✗     | ✓          | ✓         | ✓          |
| ML inference (CUDA)         | ✗     | ✓          | ✗         | ✗         |
| ROS2                        | ✗     | ✓          | ✗         | ✗         |
| Local persistence           | ✗     | ✓ (SQLite) | ✗         | KV         |
| Always-on                   | ✗     | depends    | ✗         | ✓          |
| Cold-start                  | ms    | s          | min       | ms         |
| Federation                  | ✓     | ✓          | ✓         | ✓          |
| Serves UI                   | ✗     | ✓ (:8080)  | ✓ (:8080) | n/a        |
| `no_std`                    | ✓     | ✗          | ✗         | ✗         |

## The federation story, end to end

A typical deployment looks like this:

```
           ┌───────────────────────────────┐
           │   Cloud (Codespace)            │
           │   - audit / replay             │
           │   - higher-level reasoning     │
           │   - long-term storage          │
           └──────────┬────────────────────┘
                      │  subscribes to cells,
                      │  pushes back tunables
                      │
   ┌──────────────────┼──────────────────┐
   │                  │                  │
   ▼                  ▼                  ▼
┌──────────────┐ ┌──────────────┐ ┌──────────────┐
│  Jetson #1   │ │  Jetson #2   │ │  Jetson #3   │
│  (perception)│ │  (planning)  │ │  (federation)│
│              │ │              │ │              │
│  - YOLOv8    │ │  - PID       │ │  - bridge    │
│  - IMU       │ │  - LLM       │ │  - logging   │
│  - LIDAR     │ │  - ROS2 nav  │ │  - SQLite    │
└──────┬───────┘ └──────┬───────┘ └──────┬───────┘
       │                │                │
       └────────────────┼────────────────┘
                        │  WebSocket federation,
                        │  also direct ROS2 messages
                        │
       ┌────────────────┼────────────────┐
       ▼                ▼                ▼
   ┌───────┐        ┌───────┐        ┌───────┐
   │ ESP32 │        │ ESP32 │        │ ESP32 │
   │ (boat)│        │ (boat)│        │(drone)│
   └───────┘        └───────┘        └───────┘
```

Every tier runs the same reactive engine and the same cell kinds.
A `value` cell on an ESP32 looks the same to a Jetson as a `value`
cell on a Codespace — it's just a URL. The federation layer is
**uniform**: `quilt://jetson-lab/perception#vision.obstacles` is
the same shape as `quilt://esp32-fleet/boat#sensor.heading`.

## Inside `quilt-jetson`

```
┌──────────────────────────────────────────────────────────────────┐
│  quilt-jetson (async tokio runtime)                              │
│                                                                  │
│  ┌────────────┐ ┌────────────┐ ┌────────────┐ ┌────────────┐    │
│  │   Cells    │ │   Engine   │ │ Federation │ │   Web      │    │
│  │ value      │ │ get/set    │ │ client     │ │ axum :8080 │    │
│  │ formula    │ │ call/push  │ │ quilt://   │ │ static UI  │    │
│  │ api (HTTP) │ │ subscribe  │ │ over WSS   │ │ WebSocket  │    │
│  │ program    │ │ propagate  │ └────────────┘ └────────────┘    │
│  │ sensor     │ │ trace      │                                  │
│  │ io         │ │ cache      │ ┌────────────┐ ┌────────────┐    │
│  │ listener   │ │            │ │   ROS2     │ │  SQLite    │    │
│  │ router     │ │            │ │  bridge    │ │  history   │    │
│  │ vision ⭐   │ │            │ │ (feature)  │ │            │    │
│  └────────────┘ └────────────┘ └────────────┘ └────────────┘    │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
```

## What `quilt-jetson` extends (vs. the base engine)

The base Quilt engine (in `quilt-rust` and `quilt-core`) supports
eight cell kinds. `quilt-jetson` reuses all eight and adds:

1. **Vision cell kinds** — same `api` cell, but with new URL schemes
   (`tensorrt://...`, `onnx://...`) that route to the vision
   evaluator. The evaluator uses TensorRT or ONNX Runtime to run
   detection, classification, and segmentation models on the
   Jetson's GPU.

2. **ROS2 bridge** — the `ros2_bridge` module turns `sensor` cells
   with `source: ros2:/topic:MsgType` into ROS2 subscribers, and
   `io` cells with `port: ros2:/topic:MsgType` into ROS2 publishers.

3. **Hardware sensor support** — `sensor` cells with `source:
   imu:i2c-1:0x68`, `nmea:/dev/ttyTHS1:115200`,
   `camera:/dev/video0`, and similar patterns are routed to
   appropriate drivers by the runtime.

4. **Local persistence** — the `SqliteStore` records every
   successful evaluation to a local SQLite database. The web UI
   reads this back as time-series data.

5. **Web UI** — the `axum` server on `:8080` serves a static UI
   with cell browser, live updates, history charts, and a YAML
   editor.

## Federating with other tiers

The federation client (`crate::federation`) handles cross-tier
communication:

- **Subscribe to a remote cell**: a `sensor` cell with
  `source: "quilt://other/sheet#cell"` becomes a proxy. The
  client opens a WebSocket to the remote instance and pushes
  updates into the local cell.

- **Expose a local cell**: when the engine detects that a local
  cell has subscribers on a remote tier, it forwards the value
  over the federation channel.

- **Bidirectional cell chains**: a formula on a Jetson can depend
  on a sensor on an ESP32, and a listener on a Codespace can
  depend on the formula. The propagation runs through the
  federation layer.

## Why async?

`quilt-rust` is sync. We chose async here because the Jetson
tier is about long-running tasks: ROS2 subscribers that block on
the next message, vision inference that takes 10-50ms per frame,
WebSocket federation that streams updates. A sync engine would
need a tokio bridge for every effectful cell; an async engine
just is.

The cost is that some patterns (lock contention, cancellation)
need more care. The `QuiltEngine` uses `parking_lot::RwLock` for
the cell graph and `tokio::sync::broadcast` for events, which is
the same pattern `quilt-rust` uses, just with the right async
flavors.

## When to use which tier

- **ESP32**: a microcontroller doing tight sensor or motor loops.
  No allocation, no async. Battery-powered devices.
- **Jetson**: a Linux box with a GPU, running perception and
  control. Real-time-ish, local, no cloud round-trip. Robotics,
  drones, smart cameras, edge ML.
- **Codespace**: ephemeral dev environments. Throwaway, branchable,
  good for experimentation.
- **Cloudflare**: always-on, low-cost, low-latency-to-user. Public
  dashboards, audit trails, replay.

A typical project uses three of the four. `quilt-jetson` is
**the most commonly used tier** for production deployments that
involve any kind of sensor fusion or perception.
