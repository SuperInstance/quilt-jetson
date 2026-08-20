//! # cells/vision.rs
//!
//! Vision cell evaluator — GPU-accelerated perception.
//!
//! ## Role in the system
//!
//! On Jetson, vision cells use TensorRT (NVIDIA's high-performance
//! inference engine) or ONNX Runtime to run models for detection,
//! classification, and segmentation. The cell API is the same as a
//! regular `api` cell — `tensorrt:///path/to/engine` or
//! `onnx:///path/to/model.onnx` — but the engine routes those
//! endpoints here.
//!
//! Three vision tasks are supported:
//!
//! - `detect` — bounding boxes + class labels (YOLOv5/YOLOv8 family)
//! - `classify` — single class label + confidence
//! - `segment` — per-pixel masks (YOLOv8-seg, U-Net, etc.)
//!
//! ## Implementation status
//!
//! This module is the **stub implementation** used when the
//! `vision-onnx` feature is OFF (the default). It accepts
//! `tensorrt://` and `onnx://` URLs, decodes the request, and
//! returns a synthetic result that documents the shape of the real
//! response. When the feature is ON, `vision-onnx.rs` (a future
//! module) takes over and runs the actual inference.
//!
//! ## Depends on
//!
//! - `image` — image decoding from JPEG/PNG.
//! - `nalgebra` — math for bounding-box IoU, NMS, transforms.
//! - `crate::types` — `Cell`, `CellValue`, `CallerContext`, `Effect`.
//!
//! ## Used by
//!
//! - `crate::engine` — routes `tensorrt://` and `onnx://` `api` cell
//!   endpoints to this evaluator.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::types::{now_millis, Cell, CellStatus, CellValue, Effect};

/// A vision task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisionTask {
    /// Object detection — bounding boxes + class labels.
    Detect,
    /// Image classification — single label + confidence.
    Classify,
    /// Semantic segmentation — per-pixel class masks.
    Segment,
}

impl VisionTask {
    /// Parse a task from a string. Inverse of `as_str`.
    pub fn as_str(self) -> &'static str {
        match self {
            VisionTask::Detect => "detect",
            VisionTask::Classify => "classify",
            VisionTask::Segment => "segment",
        }
    }
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "detect" => Some(VisionTask::Detect),
            "classify" => Some(VisionTask::Classify),
            "segment" => Some(VisionTask::Segment),
            _ => None,
        }
    }
}

/// The vision request shape. Resolved from the cell's `endpoint`,
/// the caller context, and (optionally) the cell's input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisionRequest {
    /// The model path (resolved from the endpoint URL).
    pub model: String,
    /// The task to run.
    pub task: VisionTask,
    /// Optional input image (base64-encoded JPEG/PNG).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Optional input image path or device (`/dev/video0`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Optional confidence threshold (0-1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conf_threshold: Option<f64>,
    /// Optional IoU threshold for NMS (0-1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iou_threshold: Option<f64>,
    /// Optional class filter — only return detections of these class
    /// ids.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub classes: Option<Vec<u32>>,
}

/// The vision response shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisionResponse {
    /// The model identifier.
    pub model: String,
    /// The task that was run.
    pub task: String,
    /// Detections (for `detect`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detections: Vec<Detection>,
    /// Classification (for `classify`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub classification: Option<Classification>,
    /// Segmentation (for `segment`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub segmentation: Option<Segmentation>,
    /// Inference latency, in milliseconds.
    pub latency_ms: u64,
}

/// A single object detection — bounding box + class + confidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Detection {
    /// Bounding box, in pixel coordinates of the source image.
    pub bbox: BoundingBox,
    /// Class id (model-specific).
    pub class_id: u32,
    /// Class name (if known).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class_name: Option<String>,
    /// Confidence, in [0, 1].
    pub confidence: f64,
}

/// A bounding box. `x`, `y` is the top-left corner; `width` and
/// `height` are the dimensions in pixels.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundingBox {
    /// Left edge, in pixels.
    pub x: f64,
    /// Top edge, in pixels.
    pub y: f64,
    /// Width, in pixels.
    pub width: f64,
    /// Height, in pixels.
    pub height: f64,
}

/// A classification result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Classification {
    /// The predicted class id.
    pub class_id: u32,
    /// The class name (if known).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class_name: Option<String>,
    /// Confidence, in [0, 1].
    pub confidence: f64,
    /// Top-k predictions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub top_k: Vec<Classification>,
}

/// A segmentation result. The `mask` is a list of class-ids, one
/// per pixel, in row-major order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segmentation {
    /// Width of the mask, in pixels.
    pub width: u32,
    /// Height of the mask, in pixels.
    pub height: u32,
    /// Class id per pixel, row-major.
    pub mask: Vec<u32>,
    /// Class id → name mapping (if known).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub classes: std::collections::BTreeMap<u32, String>,
}

use std::collections::BTreeMap;

/// A vision backend. Different backends handle the actual model
/// loading and inference. The stub backend returns synthetic
/// results; the real backends (TensorRT, ONNX) do the work.
#[async_trait::async_trait]
pub trait VisionBackend: Send + Sync {
    /// The backend's name (e.g. `"tensorrt"`, `"onnx"`, `"stub"`).
    fn name(&self) -> &'static str;
    /// Run inference.
    async fn infer(&self, req: VisionRequest) -> Result<VisionResponse>;
}

/// The default stub backend. Returns a synthetic but
/// representative result so the rest of the system can be tested
/// end-to-end without GPU hardware.
pub struct StubBackend;

#[async_trait::async_trait]
impl VisionBackend for StubBackend {
    fn name(&self) -> &'static str {
        "stub"
    }

    async fn infer(&self, req: VisionRequest) -> Result<VisionResponse> {
        let started = now_millis();
        let response = match req.task {
            VisionTask::Detect => VisionResponse {
                model: req.model.clone(),
                task: "detect".into(),
                detections: vec![Detection {
                    bbox: BoundingBox {
                        x: 100.0,
                        y: 100.0,
                        width: 200.0,
                        height: 200.0,
                    },
                    class_id: 0,
                    class_name: Some("person".into()),
                    confidence: 0.92,
                }],
                classification: None,
                segmentation: None,
                latency_ms: now_millis().saturating_sub(started),
            },
            VisionTask::Classify => VisionResponse {
                model: req.model.clone(),
                task: "classify".into(),
                detections: vec![],
                classification: Some(Classification {
                    class_id: 207,
                    class_name: Some("golden_retriever".into()),
                    confidence: 0.87,
                    top_k: vec![],
                }),
                segmentation: None,
                latency_ms: now_millis().saturating_sub(started),
            },
            VisionTask::Segment => VisionResponse {
                model: req.model.clone(),
                task: "segment".into(),
                detections: vec![],
                classification: None,
                segmentation: Some(Segmentation {
                    width: 640,
                    height: 480,
                    mask: vec![0; 640 * 480],
                    classes: BTreeMap::new(),
                }),
                latency_ms: now_millis().saturating_sub(started),
            },
        };
        Ok(response)
    }
}

/// Evaluate a vision cell. The endpoint URL is parsed for the
/// backend (`tensorrt://` or `onnx://`) and the model path. The
/// task defaults to `detect` but can be overridden via the cell
/// def's `headers` map (key: `task`).
pub async fn evaluate_vision(cell: Cell, _ctx: crate::types::CallerContext) -> CellValue {
    let started = now_millis();
    let endpoint = match &cell.def.endpoint {
        Some(e) => e.clone(),
        None => return CellValue::err("vision cell has no endpoint"),
    };

    let (backend_name, model_path) = if let Some(stripped) = endpoint.strip_prefix("tensorrt://") {
        ("tensorrt", stripped.to_string())
    } else if let Some(stripped) = endpoint.strip_prefix("onnx://") {
        ("onnx", stripped.to_string())
    } else {
        return CellValue::err(format!("vision: unknown scheme in {endpoint}"));
    };

    // Determine the task. Default is `detect`. Overridable via the
    // `task` header in the cell def.
    let task = cell
        .def
        .headers
        .get("task")
        .and_then(|s| VisionTask::from_str(s))
        .unwrap_or(VisionTask::Detect);

    // Build the request. Image source defaults to the `source` field
    // of the cell def, or `camera:/dev/video0` if not set.
    let source = cell.def.source.clone();
    let request = VisionRequest {
        model: model_path.clone(),
        task,
        image: None,
        source,
        conf_threshold: cell
            .def
            .headers
            .get("conf_threshold")
            .and_then(|s| s.parse().ok()),
        iou_threshold: cell
            .def
            .headers
            .get("iou_threshold")
            .and_then(|s| s.parse().ok()),
        classes: None,
    };

    // Pick the backend. For now, always the stub; when the
    // `vision-onnx` feature is enabled, the caller wires in a real
    // backend.
    let backend: std::sync::Arc<dyn VisionBackend> = std::sync::Arc::new(StubBackend);

    let result = backend.infer(request).await;
    let duration = now_millis().saturating_sub(started);

    match result {
        Ok(resp) => {
            let payload = json!({
                "model": resp.model,
                "task": resp.task,
                "detections": resp.detections,
                "classification": resp.classification,
                "segmentation": resp.segmentation,
                "backend": backend_name,
            });
            CellValue {
                data: payload,
                status: CellStatus::Ready,
                computed_at: Some(now_millis()),
                error: None,
                effects: vec![
                    Effect::Vision {
                        model: resp.model,
                        task: resp.task,
                        latency_ms: resp.latency_ms,
                    },
                    Effect::Compute { ms: duration },
                ],
            }
        }
        Err(e) => CellValue::err(format!("vision: {e}")),
    }
}

/// Parse a `tensorrt://` or `onnx://` URL into `(backend, model_path)`.
pub fn parse_vision_url(url: &str) -> Result<(&'static str, String)> {
    if let Some(stripped) = url.strip_prefix("tensorrt://") {
        Ok(("tensorrt", stripped.to_string()))
    } else if let Some(stripped) = url.strip_prefix("onnx://") {
        Ok(("onnx", stripped.to_string()))
    } else {
        Err(Error::Config(format!("not a vision URL: {url}")))
    }
}

/// Compute the Intersection over Union of two bounding boxes.
/// Useful for the post-processing of detection results.
pub fn iou(a: &BoundingBox, b: &BoundingBox) -> f64 {
    let ax2 = a.x + a.width;
    let ay2 = a.y + a.height;
    let bx2 = b.x + b.width;
    let by2 = b.y + b.height;

    let inter_x1 = a.x.max(b.x);
    let inter_y1 = a.y.max(b.y);
    let inter_x2 = ax2.min(bx2);
    let inter_y2 = ay2.min(by2);

    let inter_w = (inter_x2 - inter_x1).max(0.0);
    let inter_h = (inter_y2 - inter_y1).max(0.0);
    let inter_area = inter_w * inter_h;

    let a_area = a.width * a.height;
    let b_area = b.width * b.height;
    let union_area = a_area + b_area - inter_area;

    if union_area <= 0.0 {
        0.0
    } else {
        inter_area / union_area
    }
}

/// Non-Maximum Suppression — remove overlapping detections.
pub fn nms(mut detections: Vec<Detection>, iou_threshold: f64) -> Vec<Detection> {
    // Sort by confidence descending.
    detections.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal));
    let mut keep = Vec::new();
    while let Some(best) = detections.first().cloned() {
        keep.push(best.clone());
        detections.remove(0);
        detections.retain(|d| iou(&best.bbox, &d.bbox) < iou_threshold);
    }
    keep
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CellDef, CellKind};
    use serde_json::json;

    fn vision_cell(endpoint: &str) -> Cell {
        Cell::new(CellDef {
            id: "v".into(),
            kind: CellKind::Api,
            endpoint: Some(endpoint.to_string()),
            ..Default::default()
        })
    }

    #[tokio::test]
    async fn tensorrt_endpoint_with_stub() {
        let cell = vision_cell("tensorrt:///opt/models/yolov8n.engine");
        let v = evaluate_vision(cell, crate::types::CallerContext::default()).await;
        assert!(v.is_ready());
        let detections = v.data["detections"].as_array().unwrap();
        assert!(!detections.is_empty());
        assert_eq!(v.data["backend"], "tensorrt");
    }

    #[tokio::test]
    async fn onnx_endpoint_with_stub() {
        let cell = vision_cell("onnx:///opt/models/resnet50.onnx");
        let v = evaluate_vision(cell, crate::types::CallerContext::default()).await;
        assert!(v.is_ready());
        assert_eq!(v.data["backend"], "onnx");
    }

    #[tokio::test]
    async fn classify_task_via_header() {
        let mut cell = vision_cell("onnx:///opt/models/resnet50.onnx");
        cell.def.headers.insert("task".into(), "classify".into());
        let v = evaluate_vision(cell, crate::types::CallerContext::default()).await;
        assert!(v.is_ready());
        assert!(v.data["classification"].is_object());
    }

    #[tokio::test]
    async fn segment_task_via_header() {
        let mut cell = vision_cell("onnx:///opt/models/unet.onnx");
        cell.def.headers.insert("task".into(), "segment".into());
        let v = evaluate_vision(cell, crate::types::CallerContext::default()).await;
        assert!(v.is_ready());
        assert!(v.data["segmentation"].is_object());
    }

    #[tokio::test]
    async fn unknown_scheme_errors() {
        let cell = vision_cell("http://example.com/model");
        let v = evaluate_vision(cell, crate::types::CallerContext::default()).await;
        assert!(v.is_error());
    }

    #[tokio::test]
    async fn missing_endpoint_errors() {
        let cell = Cell::new(CellDef {
            id: "v".into(),
            kind: CellKind::Api,
            ..Default::default()
        });
        let v = evaluate_vision(cell, crate::types::CallerContext::default()).await;
        assert!(v.is_error());
    }

    #[test]
    fn parse_tensorrt_url() {
        let (b, p) = parse_vision_url("tensorrt:///opt/yolo.engine").unwrap();
        assert_eq!(b, "tensorrt");
        assert_eq!(p, "/opt/yolo.engine");
    }

    #[test]
    fn parse_onnx_url() {
        let (b, p) = parse_vision_url("onnx:///opt/yolo.onnx").unwrap();
        assert_eq!(b, "onnx");
        assert_eq!(p, "/opt/yolo.onnx");
    }

    #[test]
    fn parse_invalid_url_errors() {
        assert!(parse_vision_url("http://example.com").is_err());
    }

    #[test]
    fn iou_identical_boxes_is_one() {
        let a = BoundingBox { x: 0.0, y: 0.0, width: 10.0, height: 10.0 };
        assert!((iou(&a, &a) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn iou_disjoint_is_zero() {
        let a = BoundingBox { x: 0.0, y: 0.0, width: 10.0, height: 10.0 };
        let b = BoundingBox { x: 20.0, y: 20.0, width: 10.0, height: 10.0 };
        assert_eq!(iou(&a, &b), 0.0);
    }

    #[test]
    fn iou_half_overlap() {
        let a = BoundingBox { x: 0.0, y: 0.0, width: 10.0, height: 10.0 };
        let b = BoundingBox { x: 5.0, y: 0.0, width: 10.0, height: 10.0 };
        // Intersection: 5x10 = 50. Union: 100+100-50 = 150. IoU = 1/3.
        assert!((iou(&a, &b) - 1.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn nms_removes_overlaps() {
        let detections = vec![
            Detection {
                bbox: BoundingBox { x: 0.0, y: 0.0, width: 10.0, height: 10.0 },
                class_id: 0,
                class_name: None,
                confidence: 0.9,
            },
            Detection {
                bbox: BoundingBox { x: 1.0, y: 1.0, width: 10.0, height: 10.0 },
                class_id: 0,
                class_name: None,
                confidence: 0.8,
            },
            Detection {
                bbox: BoundingBox { x: 100.0, y: 100.0, width: 10.0, height: 10.0 },
                class_id: 1,
                class_name: None,
                confidence: 0.7,
            },
        ];
        let kept = nms(detections, 0.5);
        // First two should collapse into the highest-confidence one.
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].confidence, 0.9);
        assert_eq!(kept[1].confidence, 0.7);
    }

    #[test]
    fn vision_task_round_trip() {
        assert_eq!(VisionTask::from_str("detect"), Some(VisionTask::Detect));
        assert_eq!(VisionTask::from_str("classify"), Some(VisionTask::Classify));
        assert_eq!(VisionTask::from_str("segment"), Some(VisionTask::Segment));
        assert_eq!(VisionTask::from_str("bogus"), None);
        assert_eq!(VisionTask::Detect.as_str(), "detect");
    }

    #[tokio::test]
    async fn vision_response_serializable() {
        let resp = VisionResponse {
            model: "yolov8n".into(),
            task: "detect".into(),
            detections: vec![],
            classification: None,
            segmentation: None,
            latency_ms: 5,
        };
        let s = serde_json::to_string(&resp).unwrap();
        let back: VisionResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(back.model, "yolov8n");
    }

    #[test]
    fn detection_serializes_with_all_fields() {
        let d = Detection {
            bbox: BoundingBox { x: 1.0, y: 2.0, width: 3.0, height: 4.0 },
            class_id: 0,
            class_name: Some("person".into()),
            confidence: 0.95,
        };
        let s = serde_json::to_string(&d).unwrap();
        assert!(s.contains("\"class_id\":0"));
        assert!(s.contains("\"class_name\":\"person\""));
        assert!(s.contains("\"x\":1.0"));
    }
}
