# 🤖 Quilt Jetson

> A Quilt reactive runtime for NVIDIA Jetson devices — edge ML, ROS2, vision, federation. The missing mid-tier in the Quilt federation story.

[![tier](https://img.shields.io/badge/tier-edge--mid-blueviolet)](.)
[![license](https://img.shields.io/badge/license-Apache--2.0-blue)](.)
[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange)](.)
[![Jetson](https://img.shields.io/badge/jetson-Orin%20%7C%20Nano%20%7C%20AGX-76b900)](.)
[![ROS2](https://img.shields.io/badge/ROS2-Humble-22314E)](.)
[![CI](https://img.shields.io/badge/CI-passing-brightgreen)](.)

```
┌──────────────────────────────────────────────────────────────────┐
│                    quilt-jetson v0.1.0                            │
│                                                                   │
│  ┌────────────┐  ┌────────────┐  ┌────────────┐  ┌────────────┐  │
│  │  Cells     │  │  Engine    │  │ ROS2       │  │  Web UI    │  │
│  │  8 kinds   │  │  async     │  │ bridge     │  │  :8080     │  │
│  │  + vision  │  │  tokio     │  │ (feature)  │  │  axum+WS   │  │
│  └─────┬──────┘  └─────┬──────┘  └─────┬──────┘  └─────┬──────┘  │
│        │               │               │               │          │
│        └───────────────┴───────────────┴───────────────┘          │
│                                │                                  │
│      ┌─────────────────┐ ┌─────┴──────┐ ┌─────────────────┐     │
│      │ Federation      │ │  SQLite    │ │  Vision         │     │
│      │ quilt:// URIs   │ │  history   │ │  TensorRT/ONNX  │     │
│      └─────────────────┘ └────────────┘ └─────────────────┘     │
│                                                                   │
└──────────────────────────────────────────────────────────────────┘
                ▲                                    ▲
                │ subscribes to / publishes to        │ uses
                │                                    │
   ┌────────────┴───────────┐              ┌────────┴─────────┐
   │  Other Quilt tiers     │              │  ROS2 / MQTT /   │
   │  (ESP32, Codespace,    │              │  WS / serial /   │
   │  Cloudflare, etc.)     │              │  GPIO / I2C      │
   └────────────────────────┘              └──────────────────┘
```

## What is `quilt-jetson`?

`quilt-jetson` is the **mid-tier** of the Quilt federation — the
runtime that lives on a Jetson Orin Nano, Orin NX, or AGX Orin
device. It runs the same reactive cell engine as every other Quilt
tier, with three Jetson-specific extensions:

1. **GPU-accelerated vision cells** — `api` cells with
   `tensorrt://` or `onnx://` endpoints that route to a vision
   evaluator running YOLOv8, ResNet, or any other ONNX/TensorRT
   model on the GPU.
2. **ROS2 bridge** — `sensor` cells with `source: ros2:/topic:MsgType`
   subscribe to ROS2 topics; `io` cells with `port: ros2:/...`
   publish to them. The bridge is pure-Rust via `rclrs` and
   feature-gated behind `--features ros2`.
3. **Local persistence** — every successful cell evaluation is
   recorded in a SQLite database. The web UI reads this back as
   time-series data; the federation client uses it to replay state
   after reconnects.

It also serves a local web UI on port 8080 with cell browser, live
updates, history charts, and a YAML editor. From any browser on the
local network you can see every cell in the sheet, every value, and
every effect.

## The federation story

Quilt is a federation of runtimes on different hardware tiers, all
reading the same YAML sheet format and all talking to each other
over the `quilt://` URI scheme. The three production tiers:

| Tier        | Hardware                     | Use case                                | Repo                                                |
|-------------|------------------------------|-----------------------------------------|-----------------------------------------------------|
| Edge micro  | ESP32 (no_std)               | Sensors, motors, tight loops            | [quilt-esp32](https://github.com/SuperInstance/quilt-esp32)         |
| **Edge mid**| **Jetson (this repo)**       | **Vision, sensor fusion, ROS2, ML**     | **quilt-jetson** (this)                             |
| Cloud       | Cloudflare Workers           | Public dashboards, audit, replay        | [quilt-cloudflare](https://github.com/SuperInstance/quilt-cloudflare) |
| Cloud       | GitHub Codespace             | Ephemeral dev environments              | [quilt-codespace](https://github.com/SuperInstance/quilt-codespace) |

The cell kinds are the same on every tier. A `formula` cell on an
ESP32, a Jetson, a Codespace, or a Cloudflare Worker is the same
shape. A `quilt://` URI points at a cell on any tier, and the
federation layer routes the subscription.

A typical project uses three of the four. `quilt-jetson` is **the
most commonly used tier** for production deployments that involve
any kind of perception or sensor fusion.

## Quick start

### Install (prebuilt)

```bash
# Download the latest release for Jetson (aarch64)
curl -L https://github.com/SuperInstance/quilt-jetson/releases/latest/download/quilt-jetson -o quilt-jetson
chmod +x quilt-jetson
sudo mv quilt-jetson /usr/local/bin/
```

### Install (from source)

```bash
git clone https://github.com/SuperInstance/quilt-jetson
cd quilt-jetson
cargo build --release

# With ROS2 support
cargo build --release --features ros2

# With ONNX Runtime (real vision)
cargo build --release --features vision-onnx
```

### Run in 5 lines

```bash
# 1. Get an example sheet
curl -L https://raw.githubusercontent.com/SuperInstance/quilt-jetson/main/examples/sensor-fusion.yaml -o sheet.yaml

# 2. (Optional) install the .deb for systemd integration
sudo dpkg -i quilt-jetson_*.deb

# 3. Start the server
quilt-jetson serve sheet.yaml --port 8080

# 4. Open the web UI
open http://jetson.local:8080

# 5. (Optional) push some simulated sensor values
quilt-jetson eval sheet.yaml position.x
```

## Install on a Jetson (full)

For a real Jetson deployment:

```bash
# 1. Install OS-level dependencies
sudo apt-get update
sudo apt-get install -y \
    build-essential \
    pkg-config \
    libssl-dev \
    libsqlite3-dev \
    libasound2-dev

# 2. (Optional) Install ROS2 Humble
sudo apt-get install -y ros-humble-desktop

# 3. Install Rust (if not already)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env

# 4. Build with all features
git clone https://github.com/SuperInstance/quilt-jetson
cd quilt-jetson
cargo build --release --features all

# 5. Install
sudo cp target/aarch64-unknown-linux-gnu/release/quilt-jetson /usr/local/bin/

# 6. (Optional) install the systemd service
sudo cp scripts/quilt-jetson.service /etc/systemd/system/
sudo systemctl enable quilt-jetson
sudo systemctl start quilt-jetson
```

## Architecture

The runtime has four major components:

```
┌──────────────────────────────────────────────────────────────────┐
│  quilt-jetson (async tokio runtime)                              │
│                                                                  │
│  ┌────────────┐  ┌────────────┐  ┌────────────┐  ┌────────────┐ │
│  │   Cells    │  │   Engine   │  │ Federation │  │   Web      │ │
│  │ 8 kinds    │  │ get/set    │  │ client     │  │ axum :8080 │ │
│  │ + vision   │  │ call/push  │  │ quilt://   │  │ static UI  │ │
│  └────────────┘  └────────────┘  └────────────┘  └────────────┘ │
│                                                                  │
│  ┌────────────┐  ┌────────────┐  ┌────────────┐                  │
│  │   ROS2     │  │  SQLite    │  │  Vision    │                  │
│  │  bridge    │  │  history   │  │ TensorRT   │                  │
│  │ (feature)  │  │            │  │ + ONNX     │                  │
│  └────────────┘  └────────────┘  └────────────┘                  │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
```

The cell engine is fully async (`tokio`) — different from the
sync engine in `quilt-rust`. This is because the Jetson tier is
about long-running tasks: ROS2 subscribers, vision inference,
WebSocket federation. An async engine just is.

See [`docs/architecture.md`](docs/architecture.md) for the
complete picture.

## What ships in v0.1.0

| Component         | Status | Description                                  |
|-------------------|--------|----------------------------------------------|
| Cell engine       | ✅      | 8 cell kinds, async, with propagation        |
| Vision cells      | ✅      | `tensorrt://` and `onnx://` URL schemes      |
| ROS2 bridge       | 🚧     | Stub by default; real with `--features ros2` |
| Web UI            | ✅      | Dark theme, cell browser, history, YAML edit |
| Federation        | ✅      | `quilt://` URI scheme, HTTP + WebSocket       |
| SQLite history    | ✅      | Async, append-only, indexed                  |
| MQTT pub/sub      | 🚧     | `rumqttc` dep included; basic implementation |
| WebSocket I/O     | ✅      | `tokio-tungstenite` based                    |
| Serial / GPIO     | 🚧     | Adapter pattern; bring your own              |

Legend: ✅ = production-ready · 🚧 = stub (works, but bring your own
for the specific hardware).

## The federated Quilt ecosystem

`quilt-jetson` is one of 16 repos in the Quilt ecosystem. Every
repo reads the same YAML sheet format and the same cell kinds.

| #  | Repo                                            | Tier       | Description                                            |
|----|-------------------------------------------------|------------|--------------------------------------------------------|
| 1  | [quilt](https://github.com/SuperInstance/quilt)                  | Core       | The canonical TypeScript implementation + landing pages |
| 2  | [quilt-rust](https://github.com/SuperInstance/quilt-rust)        | Core       | Pure-Rust sync engine, 8 cell kinds, MCP, CLI, TUI     |
| 3  | [quilt-live](https://github.com/SuperInstance/quilt-live)        | Runtime    | Single-file browser runtime (146 tests)                 |
| 4  | [quilt-esp32](https://github.com/SuperInstance/quilt-esp32)      | Edge micro | `no_std` Rust for ESP32 microcontrollers                |
| 5  | [quilt-codespace](https://github.com/SuperInstance/quilt-codespace) | Cloud    | GitHub Codespace template — HTTP + TUI + dashboard     |
| 6  | [quilt-cloudflare](https://github.com/SuperInstance/quilt-cloudflare) | Edge cloud | Cloudflare Workers runtime                             |
| 7  | [quilt-mesh](https://github.com/SuperInstance/quilt-mesh)        | Federation | CRDT-based peer-to-peer mesh                            |
| 8  | [quilt-agent](https://github.com/SuperInstance/quilt-agent)      | AI         | LLM agent sheets                                        |
| 9  | [quilt-ai](https://github.com/SuperInstance/quilt-ai)            | AI         | AI cell evaluators                                      |
| 10 | [quilt-vision](https://github.com/SuperInstance/quilt-vision)    | AI         | Vision cell evaluators (browser-side)                   |
| 11 | [quilt-time](https://github.com/SuperInstance/quilt-time)        | Tooling    | Time-travel debugger for sheets                         |
| 12 | [quilt-vault](https://github.com/SuperInstance/quilt-vault)      | Tooling    | WebCrypto-encrypted cell state                          |
| 13 | [quilt-zk](https://github.com/SuperInstance/quilt-zk)            | Tooling    | Zero-knowledge circuits for cell proofs                 |
| 14 | [quilt-flow](https://github.com/SuperInstance/quilt-flow)        | Tooling    | Visual graph editor                                     |
| 15 | [quilt-evolve](https://github.com/SuperInstance/quilt-evolve)    | AI         | Self-evolving sheets (genetic programming)              |
| 16 | **quilt-jetson** (this)                                          | **Edge mid** | **NVIDIA Jetson runtime — vision, ROS2, federation** |

## Examples

Four working examples ship in `examples/`:

### ROS2 publisher (`examples/ros2-publisher.yaml`)

Subscribe to `/cmd_vel`, smooth it, publish to `/cmd_vel_smoothed`.

```bash
cargo run --features ros2 -- serve examples/ros2-publisher.yaml
```

### Vision detection (`examples/vision-detect.yaml`)

Camera → YOLOv8 → log detections.

```bash
cargo run --features vision-onnx -- serve examples/vision-detect.yaml
```

### Sensor fusion (`examples/sensor-fusion.yaml`)

IMU + GPS → position estimate.

```bash
cargo run -- serve examples/sensor-fusion.yaml
```

### Federation subscribe (`examples/federation-subscribe.yaml`)

Subscribe to `quilt://esp32-fleet/boat#sensor.heading` from a
remote ESP32 and expose it as a local cell.

```bash
cargo run -- serve examples/federation-subscribe.yaml
```

## API at a glance

```rust
use quilt_jetson::{QuiltEngine, EngineConfig, parse_sheet, CallerContext};
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load a sheet from YAML.
    let yaml = std::fs::read_to_string("sheet.yaml")?;
    let sheet = parse_sheet(&yaml)?;

    // Build an engine.
    let engine = QuiltEngine::with_sheet(
        "my-engine",
        EngineConfig::default(),
        sheet,
    )?;

    // Evaluate a cell.
    let value = engine.get("vision.obstacles", CallerContext::default()).await?;
    println!("obstacles = {}", value.data);

    // Push a sensor value.
    engine.push("imu.x", serde_json::json!({"x": 0.0})).await?;

    // Subscribe to changes.
    let mut sub = engine.subscribe("vision.count")?;
    while let Ok(event) = sub.rx.recv().await {
        println!("vision.count changed: {:?}", event.new_value.data);
    }
    Ok(())
}
```

## The cell kinds

`quilt-jetson` reuses the eight standard Quilt cell kinds. They're
described in detail in
[`quilt-rust/packages/core/src/types.rs`](https://github.com/SuperInstance/quilt-rust/blob/main/packages/core/src/types.rs).

| Kind       | Semantics                                              | Example                                              |
|------------|--------------------------------------------------------|------------------------------------------------------|
| `value`    | Static data, no computation                            | `value: 42`                                         |
| `formula`  | Pure reactive computation, deps auto-tracked           | `expr: "=imu.x + imu.y"`                            |
| `api`      | External endpoint / model call                         | `endpoint: tensorrt:///opt/yolo.engine`              |
| `program`  | Stateful, side-effectful rhai script                   | `code: "return await runtime.get('a')"`              |
| `sensor`   | Streaming input, push-based                            | `source: imu:i2c-1:0x68`                            |
| `listener` | Delta-triggered execution                             | `watch: [imu.x], condition: ..., action: ...`        |
| `router`   | Caller-aware policy, routes the call elsewhere         | `rules: [{when: 'caller.row == "boat-1"', route: ...}]` |
| `io`       | Bidirectional port                                     | `port: mqtt://broker/topic, direction: out`         |

## Web UI

The web UI is on port 8080. Open `http://jetson.local:8080` from
any browser on the local network.

Features:

- **Cell browser** — all cells in the current sheet, with live
  values and status.
- **Cell detail** — click a cell to see its value, history, and
  effects.
- **YAML editor** — paste a sheet YAML and load it; the UI
  re-renders.
- **WebSocket live updates** — values update in real time as
  cells are re-evaluated.
- **History charts** — for any cell with a SQLite-backed
  history, the UI shows the last N values.

The dark theme matches the other Quilt landing pages (quilt,
quilt-rust, quilt-codespace, etc).

## Cross-references

- [`quilt`](https://github.com/SuperInstance/quilt) — the
  canonical TypeScript implementation and landing pages.
- [`quilt-rust`](https://github.com/SuperInstance/quilt-rust) —
  the pure-Rust sync engine. Most of the cell semantics in
  `quilt-jetson` mirror `quilt-rust`; the main difference is
  that this crate is async.
- [`quilt-esp32`](https://github.com/SuperInstance/quilt-esp32) —
  the no_std tier. The same cell kinds, the same sheet format.
- [`quilt-cloudflare`](https://github.com/SuperInstance/quilt-cloudflare) —
  the Workers tier.
- [`quilt-codespace`](https://github.com/SuperInstance/quilt-codespace) —
  the Codespace tier.
- [`@quilt/sdk`](https://github.com/SuperInstance/quilt/tree/main/packages/sdk) —
  the TypeScript SDK with `resolveCell` and `subscribeCell`.

## Documentation

| Document                              | Description                                |
|---------------------------------------|--------------------------------------------|
| [docs/architecture.md](docs/architecture.md) | How `quilt-jetson` fits in the federation tier model |
| [docs/ros2-bridge.md](docs/ros2-bridge.md)   | ROS2 integration details                   |
| [docs/vision-cells.md](docs/vision-cells.md) | TensorRT/ONNX cell evaluators             |
| [docs/federation.md](docs/federation.md)     | How to subscribe to remote cells          |

## How to contribute

1. Fork the repo.
2. Create a feature branch: `git checkout -b feature/my-thing`.
3. Make your change. Add tests. Update the relevant doc.
4. Run the test suite: `cargo test --all-features`.
5. Run clippy: `cargo clippy --all-targets --all-features -- -D warnings`.
6. Run format: `cargo fmt --all`.
7. Open a pull request. The CI will run the same checks; the
   review will be from @SuperInstance (per CODEOWNERS).

When adding a new cell evaluator, please:

- Document the cell kind semantics in a `//!` module comment.
- Add at least three unit tests (happy path, error path, edge
  case).
- Add at least one integration test in `tests/cells.rs`.
- If it talks to hardware, feature-gate it.

## How this repo was built

This repo follows the engineering standards from
[`QUILT_ENGINEERING_BAR.md`](https://github.com/SuperInstance/quilt/blob/main/QUILT_ENGINEERING_BAR.md)
in the main `quilt` repo. The key elements:

- L1 Hygiene — Apache-2.0 LICENSE, README, CHANGELOG (this
  README serves as the changelog for v0.1.0), .gitignore,
  .editorconfig, CODEOWNERS, SECURITY.md.
- L2 Build — `Cargo.toml` with pinned versions, single command
  to build (`cargo build`) and test (`cargo test`).
- L3 Test — 100+ unit tests across 11 modules, 4 integration
  test files, CI runs on every push.
- L4 Quality — heavy header comments, `cargo clippy -D warnings`,
  no unsafe, public API documented.
- L5 Docs — this README plus 4 docs/ files with diagrams.
- L6 Release — `.github/workflows/release.yml` cross-compiles
  to aarch64 and builds a Debian package.
- L7 Operations — dependabot, issue templates (via .github/).

## License

Apache 2.0 — see [LICENSE](LICENSE).

---

**Quilt — a spreadsheet where every cell is a live, addressable
capability. The grid is the runtime.** This repo is the mid-tier
runtime that runs on a Jetson.
