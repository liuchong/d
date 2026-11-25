//! MCP JSON-RPC 2.0 protocol dispatcher (`2025-03-26`).

use crate::json::{JsonValue, json_obj, parse};
use frame::BoxFuture;
use std::collections::BTreeMap;

pub const MCP_PROTOCOL_VERSION: &str = "2025-03-26";

#[derive(Debug, Clone)]
pub struct ToolDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: JsonValue,
    pub read_only_hint: bool,
}

#[derive(Debug, Clone)]
pub struct ResourceDefinition {
    pub uri: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub mime_type: &'static str,
}

#[derive(Debug, Clone)]
pub struct ToolCallOutcome {
    pub text: String,
    pub is_error: bool,
    pub structured_content: Option<JsonValue>,
}

pub trait McpBackend: Send + Sync + 'static {
    fn server_version(&self) -> &str;
    fn list_tools(&self) -> Vec<ToolDefinition>;
    fn call_tool(
        &self,
        name: &str,
        arguments: JsonValue,
    ) -> BoxFuture<'static, Result<ToolCallOutcome, JsonRpcError>>;
    fn list_resources(&self) -> Vec<ResourceDefinition>;
    fn read_resource(
        &self,
        uri: &str,
    ) -> BoxFuture<'static, Result<(String, &'static str), JsonRpcError>>;
}

#[derive(Debug, Clone)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<JsonValue>,
}

impl JsonRpcError {
    pub fn invalid_params(msg: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: msg.into(),
            data: None,
        }
    }

    pub fn method_not_found(method: &str) -> Self {
        Self {
            code: -32601,
            message: format!("Method not found: {method}"),
            data: None,
        }
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: msg.into(),
            data: None,
        }
    }

    fn to_json(&self) -> JsonValue {
        let mut pairs = vec![
            ("code", JsonValue::Int(self.code)),
            ("message", JsonValue::Str(self.message.clone())),
        ];
        if let Some(data) = &self.data {
            pairs.push(("data", data.clone()));
        }
        json_obj(pairs)
    }
}

fn error_response(id: JsonValue, err: JsonRpcError) -> JsonValue {
    json_obj(vec![
        ("jsonrpc", JsonValue::Str("2.0".to_string())),
        ("id", id),
        ("error", err.to_json()),
    ])
}

fn ok_response(id: JsonValue, result: JsonValue) -> JsonValue {
    json_obj(vec![
        ("jsonrpc", JsonValue::Str("2.0".to_string())),
        ("id", id),
        ("result", result),
    ])
}

pub fn handle_json_rpc<B: McpBackend + Clone>(
    backend: &B,
    raw: &str,
) -> BoxFuture<'static, Option<String>> {
    let backend = backend.clone();
    let raw = raw.to_string();
    Box::pin(async move {
        let parsed = match parse(&raw) {
            Ok(v) => v,
            Err(e) => {
                let resp = error_response(
                    JsonValue::Null,
                    JsonRpcError {
                        code: -32700,
                        message: format!("Parse error: {e}"),
                        data: None,
                    },
                );
                return Some(resp.to_compact_string());
            }
        };

        match parsed {
            JsonValue::Array(items) => {
                if items.is_empty() {
                    return Some(
                        error_response(
                            JsonValue::Null,
                            JsonRpcError {
                                code: -32600,
                                message: "Invalid Request: empty batch"
                                    .to_string(),
                                data: None,
                            },
                        )
                        .to_compact_string(),
                    );
                }
                let mut replies = Vec::new();
                for item in items {
                    if let Some(resp) = dispatch_single(&backend, item).await {
                        replies.push(resp);
                    }
                }
                if replies.is_empty() {
                    None
                } else {
                    Some(JsonValue::Array(replies).to_compact_string())
                }
            }
            single => dispatch_single(&backend, single)
                .await
                .map(|r| r.to_compact_string()),
        }
    })
}

async fn dispatch_single<B: McpBackend>(
    backend: &B,
    val: JsonValue,
) -> Option<JsonValue> {
    let Some(obj) = val.as_object() else {
        return Some(error_response(
            JsonValue::Null,
            JsonRpcError {
                code: -32600,
                message: "Invalid Request: expected JSON object".to_string(),
                data: None,
            },
        ));
    };

    let id = obj.get("id").cloned();
    let is_notification = id.is_none();
    let reply_id = id.unwrap_or(JsonValue::Null);

    if obj.get("jsonrpc").and_then(JsonValue::as_str) != Some("2.0") {
        return Some(error_response(
            reply_id,
            JsonRpcError {
                code: -32600,
                message: "Invalid Request: jsonrpc must be \"2.0\"".to_string(),
                data: None,
            },
        ));
    }

    let Some(method) = obj.get("method").and_then(JsonValue::as_str) else {
        return Some(error_response(
            reply_id,
            JsonRpcError {
                code: -32600,
                message: "Invalid Request: missing string `method`".to_string(),
                data: None,
            },
        ));
    };

    let params = obj
        .get("params")
        .cloned()
        .unwrap_or_else(|| JsonValue::Object(BTreeMap::new()));

    if is_notification && method.starts_with("notifications/") {
        return None;
    }

    let result: Result<JsonValue, JsonRpcError> = match method {
        "initialize" => {
            let client_version = params
                .get("protocolVersion")
                .and_then(JsonValue::as_str)
                .unwrap_or(MCP_PROTOCOL_VERSION);
            let negotiated = if client_version == MCP_PROTOCOL_VERSION
                || client_version == "2024-11-05"
            {
                client_version
            } else {
                MCP_PROTOCOL_VERSION
            };
            Ok(json_obj(vec![
                (
                    "protocolVersion",
                    JsonValue::Str(negotiated.to_string()),
                ),
                (
                    "capabilities",
                    json_obj(vec![
                        ("tools", json_obj(vec![])),
                        ("resources", json_obj(vec![])),
                    ]),
                ),
                (
                    "serverInfo",
                    json_obj(vec![
                        ("name", JsonValue::Str("d-mcp".to_string())),
                        (
                            "version",
                            JsonValue::Str(
                                backend.server_version().to_string(),
                            ),
                        ),
                    ]),
                ),
                (
                    "instructions",
                    JsonValue::Str(
                        "Inspect and dynamically reconfigure the running d instance. \
                         Always call d_catalog and d_inspect first before applying patches."
                            .to_string(),
                    ),
                ),
            ]))
        }
        "ping" => Ok(json_obj(vec![])),
        "notifications/initialized" => return None,
        "tools/list" => {
            let tools = backend
                .list_tools()
                .into_iter()
                .map(|t| {
                    json_obj(vec![
                        ("name", JsonValue::Str(t.name.to_string())),
                        (
                            "description",
                            JsonValue::Str(t.description.to_string()),
                        ),
                        ("inputSchema", t.input_schema),
                        (
                            "annotations",
                            json_obj(vec![(
                                "readOnlyHint",
                                JsonValue::Bool(t.read_only_hint),
                            )]),
                        ),
                    ])
                })
                .collect();
            Ok(json_obj(vec![("tools", JsonValue::Array(tools))]))
        }
        "tools/call" => {
            let Some(name) = params.get("name").and_then(JsonValue::as_str)
            else {
                return Some(error_response(
                    reply_id,
                    JsonRpcError::invalid_params("missing tool `name`"),
                ));
            };
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| JsonValue::Object(BTreeMap::new()));
            match backend.call_tool(name, args).await {
                Ok(out) => {
                    let mut pairs = vec![
                        (
                            "content",
                            JsonValue::Array(vec![json_obj(vec![
                                ("type", JsonValue::Str("text".to_string())),
                                ("text", JsonValue::Str(out.text)),
                            ])]),
                        ),
                        ("isError", JsonValue::Bool(out.is_error)),
                    ];
                    if let Some(sc) = out.structured_content {
                        pairs.push(("structuredContent", sc));
                    }
                    Ok(json_obj(pairs))
                }
                Err(e) => Err(e),
            }
        }
        "resources/list" => {
            let resources = backend
                .list_resources()
                .into_iter()
                .map(|r| {
                    json_obj(vec![
                        ("uri", JsonValue::Str(r.uri.to_string())),
                        ("name", JsonValue::Str(r.name.to_string())),
                        (
                            "description",
                            JsonValue::Str(r.description.to_string()),
                        ),
                        ("mimeType", JsonValue::Str(r.mime_type.to_string())),
                    ])
                })
                .collect();
            Ok(json_obj(vec![("resources", JsonValue::Array(resources))]))
        }
        "resources/read" => {
            let Some(uri) = params.get("uri").and_then(JsonValue::as_str)
            else {
                return Some(error_response(
                    reply_id,
                    JsonRpcError::invalid_params("missing resource `uri`"),
                ));
            };
            match backend.read_resource(uri).await {
                Ok((text, mime)) => Ok(json_obj(vec![(
                    "contents",
                    JsonValue::Array(vec![json_obj(vec![
                        ("uri", JsonValue::Str(uri.to_string())),
                        ("mimeType", JsonValue::Str(mime.to_string())),
                        ("text", JsonValue::Str(text)),
                    ])]),
                )])),
                Err(e) => Err(e),
            }
        }
        other => Err(JsonRpcError::method_not_found(other)),
    };

    if is_notification {
        return None;
    }

    Some(match result {
        Ok(res) => ok_response(reply_id, res),
        Err(err) => error_response(reply_id, err),
    })
}
