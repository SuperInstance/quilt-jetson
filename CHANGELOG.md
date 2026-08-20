# Changelog

All notable changes to `quilt-jetson` are documented here. This
project follows [Semantic Versioning](https://semver.org/).

## [0.1.0] — 2026-08-19

### Added

- **Async `QuiltEngine`** — the reactive cell runtime, port of
  `quilt-rust`'s engine to a tokio-native async design.
- **All 8 standard cell kinds** — `value`, `formula`, `api`,
  `program`, `sensor`, `io`, `listener`, `router`.
- **`vision` cell evaluator** — `tensorrt://` and `onnx://` URL
  schemes on `api` cells, routed to a GPU-accelerated vision
  pipeline. Supports `detect`, `classify`, and `segment` tasks.
- **`ai` cell evaluator** — OpenAI-compatible chat completions.
- **ROS2 bridge (feature-gated)** — `rclrs` (pure-Rust ROS2
  client) integration behind `--features ros2`. Stub mode by
  default.
- **Local SQLite persistence** — every successful cell evaluation
  is recorded; the web UI reads it back as history.
- **Web UI on `:8080`** — axum server with the live cell browser,
  history view, and YAML editor.
- **Federation client** — `quilt://` URI scheme; subscribe to
  remote cells over HTTP + WebSocket.
- **100+ unit tests** across 11 modules.
- **4 integration tests** in `tests/` covering the full pipeline.
- **4 example YAML sheets** — `ros2-publisher.yaml`,
  `vision-detect.yaml`, `sensor-fusion.yaml`,
  `federation-subscribe.yaml`.
- **CI workflow** — `cargo build`, `cargo test`, `cargo clippy`,
  `cargo fmt`, cross-compile smoke test, examples validation.
- **Release workflow** — cross-compile to aarch64 on tag, build
  Debian package, upload to GitHub releases.

### Notes

- This is the v0.1.0 release. The crate is feature-complete for
  the MVP but expect rough edges around the federation client
  and the ROS2 bridge; both have working stubs and the real
  implementations are opt-in.
- The vision evaluator returns a synthetic result unless the
  `vision-onnx` feature is enabled. This is intentional — it
  lets you develop the rest of the sheet without a GPU.
- The `rclrs` and `ort` dependencies are large and platform-
  specific. They're feature-gated so the default `cargo build`
  is fast and portable.

[0.1.0]: https://github.com/SuperInstance/quilt-jetson/releases/tag/v0.1.0
