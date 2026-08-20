# Vision Cells

> Status: v0.1.0 — stub mode by default, real ONNX/TensorRT with
> `--features vision-onnx`.

A vision cell is an `api` cell whose endpoint starts with
`tensorrt://` or `onnx://`. The engine routes those URLs to the
vision cell evaluator, which runs the model on the Jetson's GPU
and returns the result as JSON.

## URL schemes

```yaml
endpoint: tensorrt:///opt/models/yolov8n.engine
endpoint: onnx:///opt/models/resnet50.onnx
```

The path after the scheme is the path to the model file on disk.

## Tasks

The vision evaluator supports three tasks:

- `detect` — bounding boxes + class labels.
- `classify` — single class label + confidence.
- `segment` — per-pixel class masks.

The default task is `detect`. Override via the cell's `headers`:

```yaml
- id: my.classifier
  kind: api
  endpoint: onnx:///opt/models/resnet50.onnx
  headers:
    task: classify
```

## Headers

The vision cell recognizes these headers:

| Header            | Default  | Description                                     |
|-------------------|----------|-------------------------------------------------|
| `task`            | `detect` | One of `detect`, `classify`, `segment`.         |
| `conf_threshold`  | `0.5`    | Minimum confidence to include a detection.      |
| `iou_threshold`   | `0.45`   | IoU threshold for NMS.                          |
| `classes`         | (all)    | Comma-separated class ids to keep.              |

## Response shape

The vision cell returns a JSON object with this shape:

```json
{
  "model": "/opt/models/yolov8n.engine",
  "task": "detect",
  "backend": "tensorrt",
  "detections": [
    {
      "bbox": { "x": 100, "y": 200, "width": 50, "height": 80 },
      "class_id": 0,
      "class_name": "person",
      "confidence": 0.92
    }
  ],
  "classification": null,
  "segmentation": null
}
```

A `classify` task returns `classification` instead of `detections`.
A `segment` task returns `segmentation`.

## Effects

A successful vision cell evaluation produces an `Effect::Vision`
in its `effects` list:

```json
{
  "kind": "vision",
  "model": "yolov8n",
  "task": "detect",
  "latency_ms": 12
}
```

This is useful for monitoring — you can attach a listener to
the vision cell and log the latencies, or push them to a
time-series database.

## Stub mode

Without the `vision-onnx` feature, the vision evaluator returns
a synthetic but representative result. This lets you develop
the rest of the sheet — formulas that count detections,
listeners that fire on certain classes, the web UI's history
view — without a GPU.

The stub result for a `detect` task is one synthetic bounding
box at `(100, 100, 200, 200)` with class name `"person"` and
confidence `0.92`. The stub result for a `classify` task is
class id `207` (golden retriever) with confidence `0.87`. The
stub result for a `segment` task is a `640×480` zero mask.

## With the real backend

Build with `--features vision-onnx`:

```bash
cargo build --release --features vision-onnx
```

This pulls in `ort` (ONNX Runtime bindings). The vision
evaluator switches to a real ONNX Runtime session. The same
URL scheme (`onnx://`) now does real inference.

For TensorRT support, the underlying ONNX Runtime uses the
TensorRT execution provider when available. The Jetson's
JetPack ships with TensorRT 8.x; `ort` will use it
automatically.

## A real example

```yaml
id: security-camera
title: Security camera with person detection
version: 0.1.0

cells:
  - id: camera.frame
    kind: sensor
    source: camera:/dev/video0
    rate: 30

  - id: vision.persons
    kind: api
    endpoint: onnx:///opt/models/yolov8n.onnx
    headers:
      task: detect
      conf_threshold: "0.6"
      classes: "0"  # person only

  - id: vision.person_count
    kind: formula
    expr: =vision.persons.detections.len()

  - id: alert.person
    kind: listener
    watch: [vision.person_count]
    condition: "vision.person_count > 0"
    action: log.person

  - id: log.person
    kind: program
    code: |
      const c = await runtime.get("vision.person_count");
      return {
        severity: "info",
        message: `${c.data} person(s) detected`,
        timestamp: Date.now(),
      };
```

This sheet:

1. Reads a frame from the camera every 33ms.
2. Runs YOLOv8n on it.
3. Counts the number of detected persons.
4. Logs an event when at least one is detected.

With the real backend, this is a complete security camera
pipeline. With the stub backend, it still runs end-to-end —
the stub returns one detection per call, so the count is
always 1 and the listener always fires.

## Performance

On a Jetson Orin Nano, YOLOv8n at 640×640 runs at about
30 FPS in FP16 mode via TensorRT. The vision cell's
`latency_ms` effect reports the actual time taken.

For higher throughput, batch the inputs: write a `program`
cell that collects N frames and dispatches them all in one
ONNX Runtime call. The `ort` API supports this natively.
