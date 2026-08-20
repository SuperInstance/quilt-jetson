//! # cells/api.rs
//!
//! API cell evaluator — async, may have effects.
//!
//! ## Role in the system
//!
//! Async, may have effects (network, model), may be expensive. Caller
//! context can route which model/endpoint to use. The endpoint can be:
//!
//! - an HTTP URL (with `{{caller.row}}`-style template substitution)
//! - a `model:foo` pseudo-URL (placeholder; a real implementation
//!   would look up the provider, swap based on context, call the model
//!   API)
//! - an `mcp://server/tool` reference (placeholder)
//! - a `tensorrt://path/to/engine` URL (routed to the vision
//!   evaluator — see `vision.rs`)
//! - an `onnx://path/to/model.onnx` URL (routed to the vision
//!   evaluator)
//!
//! ## Depends on
//!
//! - `reqwest` — async HTTP client.
//! - `crate::types` — `Cell`, `CellValue`, `CallerContext`, `Effect`.
//!
//! ## Used by
//!
//! - `crate::engine` — dispatches to this on `get`/`call` for `api`
//!   cells. The engine also routes `tensorrt://` and `onnx://`
//!   endpoints to `vision::evaluate_vision` before falling through to
//!   this evaluator.

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Response;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::types::{now_millis, Cell, CellStatus, CellValue, Effect};

/// Pluggable HTTP transport. Tests use the `Stub` variant to avoid
/// the network; production uses `Reqwest`.
#[async_trait]
pub trait ApiExecutor: Send + Sync {
    /// Issue an HTTP request and return the response.
    async fn execute(
        &self,
        method: &str,
        url: &str,
        headers: &BTreeMap<String, String>,
        body: Option<&str>,
    ) -> Result<ApiResponse>;
}

/// A stripped-down HTTP response — just what the cell needs.
#[derive(Debug, Clone)]
pub struct ApiResponse {
    /// The HTTP status code.
    pub status: u16,
    /// The reason phrase (e.g. `"OK"`).
    pub status_text: String,
    /// Response headers, lowercased keys.
    pub headers: BTreeMap<String, String>,
    /// The body, as a JSON value if the content type was JSON;
    /// otherwise as a JSON string.
    pub body: Value,
}

/// The default executor. Wraps `reqwest::Client`.
pub struct ReqwestExecutor {
    /// The underlying client. Cloned for each call (reqwest clients
    /// are cheap to clone and internally share a connection pool).
    client: reqwest::Client,
}

impl ReqwestExecutor {
    /// Create a new executor with a default-configured client.
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client should build with default config");
        Self { client }
    }
}

impl Default for ReqwestExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ApiExecutor for ReqwestExecutor {
    async fn execute(
        &self,
        method: &str,
        url: &str,
        headers: &BTreeMap<String, String>,
        body: Option<&str>,
    ) -> Result<ApiResponse> {
        let method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|e| Error::Config(format!("invalid HTTP method '{method}': {e}")))?;
        let mut req = self.client.request(method, url);
        for (k, v) in headers {
            req = req.header(k.as_str(), v.as_str());
        }
        if let Some(b) = body {
            req = req.body(b.to_string());
        }
        let resp = req.send().await?;
        Ok(response_to_apiresponse(resp).await?)
    }
}

async fn response_to_apiresponse(resp: Response) -> Result<ApiResponse> {
    let status = resp.status();
    let status_code = status.as_u16();
    let status_text = status.canonical_reason().unwrap_or("").to_string();
    let mut headers = BTreeMap::new();
    for (k, v) in resp.headers().iter() {
        headers.insert(k.as_str().to_lowercase(), v.to_str().unwrap_or("").to_string());
    }
    let content_type = headers.get("content-type").cloned().unwrap_or_default();
    let body_text = resp.text().await?;
    let body = if content_type.contains("application/json") {
        serde_json::from_str(&body_text).unwrap_or(Value::String(body_text))
    } else {
        Value::String(body_text)
    };
    Ok(ApiResponse {
        status: status_code,
        status_text,
        headers,
        body,
    })
}

/// A test executor. Returns a canned response regardless of the
/// request.
pub struct StubExecutor {
    /// The response to return.
    pub response: ApiResponse,
}

#[async_trait]
impl ApiExecutor for StubExecutor {
    async fn execute(
        &self,
        _method: &str,
        _url: &str,
        _headers: &BTreeMap<String, String>,
        _body: Option<&str>,
    ) -> Result<ApiResponse> {
        Ok(self.response.clone())
    }
}

/// Convenience alias used in `CellDef`-shaped contexts. The full type
/// is `Arc<dyn ApiExecutor>`.
pub type ApiExecutorRef = std::sync::Arc<dyn ApiExecutor>;

/// Evaluate an API cell. The `executor` parameter lets tests
/// substitute a stub. Pass `None` to use the default reqwest-based
/// executor.
pub async fn evaluate_api(
    cell: Cell,
    ctx: crate::types::CallerContext,
    _input: Option<Value>,
    executor: Option<ApiExecutorRef>,
) -> CellValue {
    let started_at = now_millis();
    let endpoint = match &cell.def.endpoint {
        Some(e) => e.clone(),
        None => return CellValue::err("api cell has no endpoint"),
    };

    // Model pseudo-endpoints: a real implementation would look up
    // the provider and call the model API. For now, return a
    // synthetic result.
    if let Some(stripped) = endpoint.strip_prefix("model:") {
        return CellValue {
            data: serde_json::json!({
                "model": stripped,
                "note": "model calls not yet implemented",
            }),
            status: CellStatus::Ready,
            computed_at: Some(now_millis()),
            error: None,
            effects: vec![Effect::Model {
                provider: stripped.to_string(),
                tokens_in: None,
                tokens_out: None,
            }],
        };
    }
    if endpoint.starts_with("mcp://") {
        return CellValue {
            data: serde_json::json!({
                "tool": endpoint,
                "note": "MCP tool calls not yet implemented",
            }),
            status: CellStatus::Ready,
            computed_at: Some(now_millis()),
            error: None,
            effects: vec![Effect::Network {
                url: endpoint.clone(),
                method: "MCP".to_string(),
            }],
        };
    }
    // tensorrt:// and onnx:// are handled by the engine, not here.
    // If we got one, the engine has a bug — return an error.
    if endpoint.starts_with("tensorrt://") || endpoint.starts_with("onnx://") {
        return CellValue::err(format!(
            "api cell with {endpoint} should be routed to the vision evaluator"
        ));
    }

    let url = substitute(&endpoint, &ctx);
    let method = cell.def.method.clone().unwrap_or_else(|| "GET".to_string());
    let mut headers = cell.def.headers.clone();
    if method != "GET" && method != "HEAD" && !headers.contains_key("content-type") {
        headers.insert("content-type".to_string(), "application/json".to_string());
    }

    let executor: ApiExecutorRef =
        executor.unwrap_or_else(|| std::sync::Arc::new(ReqwestExecutor::new()));

    let result = executor.execute(&method, &url, &headers, None).await;
    let duration = now_millis().saturating_sub(started_at);

    match result {
        Ok(resp) if (200..300).contains(&resp.status) => CellValue {
            data: resp.body,
            status: CellStatus::Ready,
            computed_at: Some(now_millis()),
            error: None,
            effects: vec![
                Effect::Network {
                    url,
                    method,
                },
                Effect::Compute { ms: duration },
            ],
        },
        Ok(resp) => CellValue::err(format!("HTTP {} {}", resp.status, resp.status_text)),
        Err(err) => CellValue::err(format!("{err}")),
    }
}

/// Substitute `{{path}}` placeholders by walking into the context.
fn substitute(template: &str, ctx: &crate::types::CallerContext) -> String {
    let mut out = String::with_capacity(template.len());
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'{' && bytes[i + 1] == b'{' {
            // Find the closing `}}`
            let mut j = i + 2;
            while j + 1 < bytes.len() && !(bytes[j] == b'}' && bytes[j + 1] == b'}') {
                j += 1;
            }
            if j + 1 < bytes.len() {
                let path = &template[i + 2..j];
                out.push_str(&lookup(ctx, path));
                i = j + 2;
                continue;
            }
        }
        let c = template[i..].chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    out
}

fn lookup(ctx: &crate::types::CallerContext, path: &str) -> String {
    let mut parts = path.trim().split('.');
    let first = parts.next().unwrap_or("");
    let value: Option<Value> = match first {
        "sheet" => ctx.sheet.clone().map(Value::String),
        "row" => ctx.row.clone(),
        "column" => ctx.column.clone(),
        "identity" => ctx.identity.as_ref().map(|i| {
            serde_json::json!({
                "id": i.id,
                "type": i.kind,
                "tags": i.tags,
            })
        }),
        "metadata" => Some(Value::Object(
            ctx.metadata
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        )),
        "caller" => ctx.caller.clone().map(Value::String),
        _ => None,
    };
    let mut cur: Option<&Value> = value.as_ref();

    for p in parts {
        if let Some(v) = cur {
            if let Value::Object(map) = v {
                cur = map.get(p);
            } else {
                cur = None;
                break;
            }
        } else {
            break;
        }
    }

    match cur {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CellDef, CellKind};
    use std::sync::Arc;

    fn api_cell(endpoint: &str) -> Cell {
        Cell::new(CellDef {
            id: "api".into(),
            kind: CellKind::Api,
            endpoint: Some(endpoint.to_string()),
            ..Default::default()
        })
    }

    #[tokio::test]
    async fn stub_returns_canned_response() {
        let stub = StubExecutor {
            response: ApiResponse {
                status: 200,
                status_text: "OK".into(),
                headers: Default::default(),
                body: serde_json::json!({"ok": true}),
            },
        };
        let cell = api_cell("https://example.com/test");
        let v = evaluate_api(
            cell,
            crate::types::CallerContext::default(),
            None,
            Some(Arc::new(stub)),
        )
        .await;
        assert_eq!(v.status, CellStatus::Ready);
        assert_eq!(v.data, serde_json::json!({"ok": true}));
    }

    #[tokio::test]
    async fn model_pseudo_endpoint_returns_synthetic() {
        let cell = api_cell("model:gpt-4o");
        let v = evaluate_api(
            cell,
            crate::types::CallerContext::default(),
            None,
            None,
        )
        .await;
        assert_eq!(v.status, CellStatus::Ready);
        assert_eq!(v.data["model"], "gpt-4o");
    }

    #[tokio::test]
    async fn mcp_pseudo_endpoint_returns_synthetic() {
        let cell = api_cell("mcp://server/tool");
        let v = evaluate_api(
            cell,
            crate::types::CallerContext::default(),
            None,
            None,
        )
        .await;
        assert_eq!(v.status, CellStatus::Ready);
        assert!(v.data["tool"].as_str().unwrap().contains("mcp://"));
    }

    #[tokio::test]
    async fn tensorrt_endpoint_returns_error_from_here() {
        let cell = api_cell("tensorrt:///opt/models/yolo.engine");
        let v = evaluate_api(
            cell,
            crate::types::CallerContext::default(),
            None,
            None,
        )
        .await;
        // This should error because the engine is supposed to route
        // tensorrt:// to the vision evaluator.
        assert!(v.is_error());
    }

    #[tokio::test]
    async fn missing_endpoint_errors() {
        let cell = Cell::new(CellDef {
            id: "api".into(),
            kind: CellKind::Api,
            endpoint: None,
            ..Default::default()
        });
        let v = evaluate_api(
            cell,
            crate::types::CallerContext::default(),
            None,
            None,
        )
        .await;
        assert_eq!(v.status, CellStatus::Error);
    }

    #[tokio::test]
    async fn stub_returns_500() {
        let stub = StubExecutor {
            response: ApiResponse {
                status: 500,
                status_text: "Internal".into(),
                headers: Default::default(),
                body: Value::Null,
            },
        };
        let cell = api_cell("https://example.com/test");
        let v = evaluate_api(
            cell,
            crate::types::CallerContext::default(),
            None,
            Some(Arc::new(stub)),
        )
        .await;
        assert!(v.is_error());
    }

    #[test]
    fn substitute_replaces_paths() {
        let mut ctx = crate::types::CallerContext::default();
        ctx.row = Some(serde_json::json!(7));
        let s = substitute("https://example.com/r/{{row}}", &ctx);
        assert_eq!(s, "https://example.com/r/7");
    }

    #[test]
    fn substitute_handles_multiple_paths() {
        let mut ctx = crate::types::CallerContext::default();
        ctx.row = Some(serde_json::json!(1));
        ctx.column = Some(serde_json::json!(2));
        let s = substitute("r/{{row}}/c/{{column}}", &ctx);
        assert_eq!(s, "r/1/c/2");
    }

    #[test]
    fn substitute_preserves_unknown_paths() {
        let ctx = crate::types::CallerContext::default();
        let s = substitute("foo/{{unknown}}", &ctx);
        assert_eq!(s, "foo/");
    }

    #[test]
    fn substitute_no_paths() {
        let ctx = crate::types::CallerContext::default();
        let s = substitute("no-substitution", &ctx);
        assert_eq!(s, "no-substitution");
    }

    #[test]
    fn lookup_identity_returns_object() {
        let mut ctx = crate::types::CallerContext::default();
        ctx.identity = Some(crate::types::Identity {
            id: "u1".into(),
            kind: "user".into(),
            tags: vec!["premium".into()],
        });
        let v = lookup(&ctx, "identity.id");
        assert_eq!(v, "u1");
    }
}
