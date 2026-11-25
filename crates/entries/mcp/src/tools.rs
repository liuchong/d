//! Self-evolving MCP tools and resources exposing live `frame` state,
//! schema catalog discovery, HTTP probing, and dynamic reconfiguration.

use crate::json::{
    JsonValue, config_to_json, config_to_json_redacted, json_obj,
    json_to_config, parse,
};
use crate::protocol::{
    JsonRpcError, McpBackend, ResourceDefinition, ToolCallOutcome,
    ToolDefinition, handle_json_rpc,
};
use d_cli::LogLevel;
use d_daemon::RuntimeControlService;
use frame::{
    BoxFuture, ConfigFieldSchema, ConfigValue, Context, EntryPatch,
    EntryTarget, FiberState, FrameError, InsertAnchor, LoaderService,
    PluginDescriptor, PluginEntry, PluginSchema, merge_config_values,
};
use http_api::{HttpRequest, Method};
use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::Arc;

const MAX_HTTP_PROBE_PREVIEW_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone)]
pub struct McpServerPolicy {
    pub read_only: bool,
    pub allow_shutdown: bool,
    pub stdio: bool,
    pub http_addr: Option<SocketAddr>,
    pub http_path: String,
    pub auth_token_configured: bool,
}

#[derive(Clone)]
pub struct McpServer {
    ctx: Context,
    loader: LoaderService,
    control: Arc<RuntimeControlService>,
    policy: McpServerPolicy,
}

impl McpServer {
    pub fn new(
        ctx: Context,
        loader: LoaderService,
        control: Arc<RuntimeControlService>,
        policy: McpServerPolicy,
    ) -> Self {
        Self {
            ctx,
            loader,
            control,
            policy,
        }
    }

    pub fn handle_str(&self, raw: &str) -> BoxFuture<'static, Option<String>> {
        handle_json_rpc(self, raw)
    }

    fn schema_index(&self) -> HashMap<String, PluginSchema> {
        self.loader
            .catalog()
            .into_iter()
            .map(|d| (d.name, d.schema))
            .collect()
    }

    fn entry_to_json(
        entry: &PluginEntry,
        schemas: &HashMap<String, PluginSchema>,
    ) -> JsonValue {
        let schema = match &entry.target {
            EntryTarget::Name(n) => schemas.get(n),
            EntryTarget::Plugin(p) => schemas.get(p.name()),
            EntryTarget::Group => None,
        };
        let target_val = match &entry.target {
            EntryTarget::Name(n) => json_obj(vec![
                ("kind", JsonValue::Str("name".to_string())),
                ("name", JsonValue::Str(n.clone())),
            ]),
            EntryTarget::Plugin(p) => json_obj(vec![
                ("kind", JsonValue::Str("plugin".to_string())),
                ("name", JsonValue::Str(p.name().to_string())),
            ]),
            EntryTarget::Group => {
                json_obj(vec![("kind", JsonValue::Str("group".to_string()))])
            }
        };
        let isolate: Vec<JsonValue> = entry
            .isolate
            .iter()
            .map(|k| JsonValue::Str(k.name.to_string()))
            .collect();
        let children: Vec<JsonValue> = entry
            .children
            .iter()
            .map(|c| Self::entry_to_json(c, schemas))
            .collect();
        json_obj(vec![
            ("id", JsonValue::Str(entry.id.clone())),
            ("target", target_val),
            ("disabled", JsonValue::Bool(entry.disabled)),
            ("config", config_to_json_redacted(&entry.config, schema)),
            ("isolate", JsonValue::Array(isolate)),
            ("children", JsonValue::Array(children)),
        ])
    }

    fn descriptor_to_json(desc: &PluginDescriptor) -> JsonValue {
        let fields: Vec<JsonValue> = desc
            .schema
            .fields
            .iter()
            .map(field_schema_to_json)
            .collect();
        json_obj(vec![
            ("name", JsonValue::Str(desc.name.clone())),
            ("description", JsonValue::Str(desc.description.clone())),
            (
                "required_deps",
                JsonValue::Array(
                    desc.required_deps
                        .iter()
                        .map(|s| JsonValue::Str((*s).to_string()))
                        .collect(),
                ),
            ),
            (
                "optional_deps",
                JsonValue::Array(
                    desc.optional_deps
                        .iter()
                        .map(|s| JsonValue::Str((*s).to_string()))
                        .collect(),
                ),
            ),
            ("fields", JsonValue::Array(fields)),
        ])
    }

    fn build_server_info_json(&self) -> JsonValue {
        let snap = self.ctx.inspect();
        let mut active = 0i64;
        let mut pending = 0i64;
        let mut failed = 0i64;
        let mut other = 0i64;
        for f in &snap.fibers {
            match f.state {
                FiberState::Active => active += 1,
                FiberState::Pending => pending += 1,
                FiberState::Failed => failed += 1,
                _ => other += 1,
            }
        }
        json_obj(vec![
            (
                "assembly",
                JsonValue::Str(self.control.assembly_name().to_string()),
            ),
            (
                "version",
                JsonValue::Str(self.control.version().to_string()),
            ),
            (
                "local_addr",
                self.control
                    .local_addr()
                    .map_or(JsonValue::Null, |a| JsonValue::Str(a.to_string())),
            ),
            (
                "log_level",
                JsonValue::Str(self.control.log_level().as_str().to_string()),
            ),
            (
                "mcp",
                json_obj(vec![
                    ("stdio", JsonValue::Bool(self.policy.stdio)),
                    (
                        "http_addr",
                        self.policy.http_addr.map_or(JsonValue::Null, |a| {
                            JsonValue::Str(a.to_string())
                        }),
                    ),
                    (
                        "http_path",
                        JsonValue::Str(self.policy.http_path.clone()),
                    ),
                    (
                        "auth_token_configured",
                        JsonValue::Bool(self.policy.auth_token_configured),
                    ),
                    ("read_only", JsonValue::Bool(self.policy.read_only)),
                    (
                        "allow_shutdown",
                        JsonValue::Bool(self.policy.allow_shutdown),
                    ),
                ]),
            ),
            (
                "fiber_counts",
                json_obj(vec![
                    ("total", JsonValue::Int(snap.fibers.len() as i64)),
                    ("active", JsonValue::Int(active)),
                    ("pending", JsonValue::Int(pending)),
                    ("failed", JsonValue::Int(failed)),
                    ("other", JsonValue::Int(other)),
                ]),
            ),
        ])
    }

    fn build_catalog_json(&self) -> JsonValue {
        let items = self
            .loader
            .catalog()
            .iter()
            .map(Self::descriptor_to_json)
            .collect();
        json_obj(vec![("plugins", JsonValue::Array(items))])
    }

    fn build_entries_json(&self) -> JsonValue {
        let schemas = self.schema_index();
        let entries = self
            .loader
            .entries()
            .iter()
            .map(|e| Self::entry_to_json(e, &schemas))
            .collect();
        let states = self
            .loader
            .inspect()
            .into_iter()
            .map(|s| {
                json_obj(vec![
                    ("id", JsonValue::Str(s.id)),
                    ("state", JsonValue::Str(format!("{:?}", s.state))),
                ])
            })
            .collect();
        json_obj(vec![
            ("entries", JsonValue::Array(entries)),
            ("states", JsonValue::Array(states)),
        ])
    }

    fn build_fibers_json(&self) -> JsonValue {
        let snap = self.ctx.inspect();
        let fibers = snap
            .fibers
            .into_iter()
            .map(|f| {
                json_obj(vec![
                    ("id", JsonValue::Int(f.id.0 as i64)),
                    (
                        "parent",
                        f.parent.map_or(JsonValue::Null, |p| {
                            JsonValue::Int(p.0 as i64)
                        }),
                    ),
                    (
                        "children",
                        JsonValue::Array(
                            f.children
                                .into_iter()
                                .map(|c| JsonValue::Int(c.0 as i64))
                                .collect(),
                        ),
                    ),
                    ("name", JsonValue::Str(f.name)),
                    ("state", JsonValue::Str(format!("{:?}", f.state))),
                    (
                        "dependencies",
                        JsonValue::Array(
                            f.dependencies
                                .into_iter()
                                .map(|s| JsonValue::Str(s.to_string()))
                                .collect(),
                        ),
                    ),
                    (
                        "optional_dependencies",
                        JsonValue::Array(
                            f.optional_dependencies
                                .into_iter()
                                .map(|s| JsonValue::Str(s.to_string()))
                                .collect(),
                        ),
                    ),
                    (
                        "provided",
                        JsonValue::Array(
                            f.provided
                                .into_iter()
                                .map(|s| JsonValue::Str(s.to_string()))
                                .collect(),
                        ),
                    ),
                    ("effect_count", JsonValue::Int(f.effect_count as i64)),
                    (
                        "error_summary",
                        f.error_summary.map_or(JsonValue::Null, JsonValue::Str),
                    ),
                ])
            })
            .collect();
        json_obj(vec![("fibers", JsonValue::Array(fibers))])
    }

    fn build_inspect_json(&self) -> JsonValue {
        let schemas = self.schema_index();
        let entries = self
            .loader
            .entries()
            .iter()
            .map(|e| Self::entry_to_json(e, &schemas))
            .collect();
        let loader_states = self
            .loader
            .inspect()
            .into_iter()
            .map(|s| {
                json_obj(vec![
                    ("id", JsonValue::Str(s.id)),
                    ("state", JsonValue::Str(format!("{:?}", s.state))),
                ])
            })
            .collect();
        json_obj(vec![
            ("server", self.build_server_info_json()),
            ("loader_states", JsonValue::Array(loader_states)),
            ("entries", JsonValue::Array(entries)),
            (
                "fibers",
                self.build_fibers_json()
                    .get("fibers")
                    .cloned()
                    .unwrap_or_else(|| JsonValue::Array(Vec::new())),
            ),
        ])
    }
}

impl McpBackend for McpServer {
    fn server_version(&self) -> &str {
        self.control.version()
    }

    fn list_tools(&self) -> Vec<ToolDefinition> {
        let mut tools = vec![
            ToolDefinition {
                name: "d_server_info",
                description: "Inspect live assembly name, version, bound HTTP address, log level, MCP policy, and fiber state counts.",
                input_schema: empty_object_schema(),
                read_only_hint: true,
            },
            ToolDefinition {
                name: "d_catalog",
                description: "Discover every plugin registered in this assembly, including dependencies and typed configuration schemas.",
                input_schema: empty_object_schema(),
                read_only_hint: true,
            },
            ToolDefinition {
                name: "d_inspect",
                description: "Inspect full live runtime state: server summary, loader entry states, redacted configs, and sanitized fiber topology.",
                input_schema: empty_object_schema(),
                read_only_hint: true,
            },
            ToolDefinition {
                name: "d_validate_patches",
                description: "Dry-run preflight an EntryPatch list against the current manifest without mutating live plugins.",
                input_schema: patches_input_schema(),
                read_only_hint: true,
            },
            ToolDefinition {
                name: "d_http_probe",
                description: "Dispatch a synthetic in-memory HTTP request through the live router and filter pipeline to verify routing, headers, or UI.",
                input_schema: http_probe_schema(),
                read_only_hint: true,
            },
        ];

        if !self.policy.read_only {
            tools.extend([
                ToolDefinition {
                    name: "d_set_plugin_config",
                    description: "Update or merge configuration (and optional disabled flag) on a loader entry, or insert it if create_if_missing=true.",
                    input_schema: set_plugin_config_schema(),
                    read_only_hint: false,
                },
                ToolDefinition {
                    name: "d_toggle_plugin",
                    description: "Enable or disable (or remove) a plugin entry by id; inserts it from catalog if absent when enabling.",
                    input_schema: toggle_plugin_schema(),
                    read_only_hint: false,
                },
                ToolDefinition {
                    name: "d_restart_plugin",
                    description: "Restart an active managed plugin fiber in place by entry id.",
                    input_schema: id_only_schema(),
                    read_only_hint: false,
                },
                ToolDefinition {
                    name: "d_apply_patches",
                    description: "Atomically preflight and apply EntryPatch operations (insert, remove, replace_config) and settle the runtime.",
                    input_schema: patches_input_schema(),
                    read_only_hint: false,
                },
                ToolDefinition {
                    name: "d_apply_manifest",
                    description: "Atomically preflight and replace the entire managed PluginEntry manifest and settle the runtime.",
                    input_schema: manifest_input_schema(),
                    read_only_hint: false,
                },
                ToolDefinition {
                    name: "d_set_log_level",
                    description: "Dynamically adjust the daemon logging verbosity (error, warn, info, debug, trace).",
                    input_schema: log_level_schema(),
                    read_only_hint: false,
                },
            ]);

            if self.policy.allow_shutdown {
                tools.push(ToolDefinition {
                    name: "d_shutdown",
                    description: "Trigger graceful daemon shutdown and L4 connection drain.",
                    input_schema: empty_object_schema(),
                    read_only_hint: false,
                });
            }
        }

        tools
    }

    fn call_tool(
        &self,
        name: &str,
        arguments: JsonValue,
    ) -> BoxFuture<'static, Result<ToolCallOutcome, JsonRpcError>> {
        let srv = self.clone();
        let name = name.to_string();
        Box::pin(async move {
            let is_mutating = matches!(
                name.as_str(),
                "d_set_plugin_config"
                    | "d_toggle_plugin"
                    | "d_restart_plugin"
                    | "d_apply_patches"
                    | "d_apply_manifest"
                    | "d_set_log_level"
                    | "d_shutdown"
            );
            if srv.policy.read_only && is_mutating {
                return Ok(tool_error(
                    "READ_ONLY",
                    "MCP server is running in read-only mode; mutations are disabled",
                    None,
                ));
            }

            match name.as_str() {
                "d_server_info" => Ok(tool_ok(srv.build_server_info_json())),
                "d_catalog" => Ok(tool_ok(srv.build_catalog_json())),
                "d_inspect" => Ok(tool_ok(srv.build_inspect_json())),
                "d_validate_patches" => {
                    let patches = parse_patches_arg(&arguments)?;
                    match srv.loader.validate_patches(&patches) {
                        Ok(candidate) => {
                            let schemas = srv.schema_index();
                            let rendered = candidate
                                .iter()
                                .map(|e| Self::entry_to_json(e, &schemas))
                                .collect();
                            Ok(tool_ok(json_obj(vec![
                                ("valid", JsonValue::Bool(true)),
                                (
                                    "candidate_entries",
                                    JsonValue::Array(rendered),
                                ),
                            ])))
                        }
                        Err(err) => Ok(frame_error_outcome(&err)),
                    }
                }
                "d_http_probe" => srv.exec_http_probe(arguments).await,
                "d_set_plugin_config" => {
                    srv.exec_set_plugin_config(arguments).await
                }
                "d_toggle_plugin" => srv.exec_toggle_plugin(arguments).await,
                "d_restart_plugin" => srv.exec_restart_plugin(arguments).await,
                "d_apply_patches" => srv.exec_apply_patches(arguments).await,
                "d_apply_manifest" => srv.exec_apply_manifest(arguments).await,
                "d_set_log_level" => srv.exec_set_log_level(arguments),
                "d_shutdown" => {
                    if !srv.policy.allow_shutdown {
                        return Ok(tool_error(
                            "FORBIDDEN",
                            "d_shutdown requires --mcp-allow-shutdown / D_MCP_ALLOW_SHUTDOWN=true",
                            None,
                        ));
                    }
                    srv.control.request_shutdown();
                    Ok(tool_ok(json_obj(vec![(
                        "shutdown_requested",
                        JsonValue::Bool(true),
                    )])))
                }
                _ => Err(JsonRpcError::invalid_params(format!(
                    "unknown tool `{name}`"
                ))),
            }
        })
    }

    fn list_resources(&self) -> Vec<ResourceDefinition> {
        vec![
            ResourceDefinition {
                uri: "d://state",
                name: "d-state",
                description: "Live d server info, local_addr, log level, and fiber state counts.",
                mime_type: "application/json",
            },
            ResourceDefinition {
                uri: "d://catalog",
                name: "d-catalog",
                description: "Live catalog of registered plugins, dependencies, and config schemas.",
                mime_type: "application/json",
            },
            ResourceDefinition {
                uri: "d://entries",
                name: "d-entries",
                description: "Current declarative PluginEntry manifest (sensitive fields redacted) and loader states.",
                mime_type: "application/json",
            },
            ResourceDefinition {
                uri: "d://fibers",
                name: "d-fibers",
                description: "Sanitized FrameSnapshot fiber tree.",
                mime_type: "application/json",
            },
        ]
    }

    fn read_resource(
        &self,
        uri: &str,
    ) -> BoxFuture<'static, Result<(String, &'static str), JsonRpcError>> {
        let val = match uri {
            "d://state" => Ok(self.build_server_info_json()),
            "d://catalog" => Ok(self.build_catalog_json()),
            "d://entries" => Ok(self.build_entries_json()),
            "d://fibers" => Ok(self.build_fibers_json()),
            other => Err(JsonRpcError::invalid_params(format!(
                "unknown resource `{other}`"
            ))),
        };
        Box::pin(async move {
            val.map(|v| (v.to_pretty_string(), "application/json"))
        })
    }
}

impl McpServer {
    async fn exec_http_probe(
        &self,
        args: JsonValue,
    ) -> Result<ToolCallOutcome, JsonRpcError> {
        let uri =
            args.get("uri").and_then(JsonValue::as_str).ok_or_else(|| {
                JsonRpcError::invalid_params("missing string `uri`")
            })?;
        let method_raw = args
            .get("method")
            .and_then(JsonValue::as_str)
            .unwrap_or("GET")
            .to_ascii_uppercase();
        let Some(method) = Method::from_bytes(method_raw.as_bytes()) else {
            return Err(JsonRpcError::invalid_params(format!(
                "invalid HTTP method `{method_raw}`"
            )));
        };

        let mut req = HttpRequest::new(method, uri.to_string());
        if let Some(hdrs) = args.get("headers") {
            let Some(map) = hdrs.as_object() else {
                return Err(JsonRpcError::invalid_params(
                    "`headers` must be a string map",
                ));
            };
            for (k, v) in map {
                let Some(vs) = v.as_str() else {
                    return Err(JsonRpcError::invalid_params(format!(
                        "header `{k}` value must be a string"
                    )));
                };
                req = req.with_header(k.clone(), vs.to_string());
            }
        }

        let resp = match self.control.dispatch_http(req).await {
            Ok(r) => r,
            Err(e) => return Ok(frame_error_outcome(&e)),
        };

        let status = resp.status.as_u16();
        let headers: Vec<JsonValue> = resp
            .headers
            .iter()
            .map(|(k, v)| {
                json_obj(vec![
                    ("name", JsonValue::Str(k.to_string())),
                    ("value", JsonValue::Str(v.to_string())),
                ])
            })
            .collect();
        let bytes = match resp.body.into_bytes().await {
            Ok(b) => b,
            Err(e) => {
                return Ok(tool_error(
                    "PROBE_BODY_ERROR",
                    &format!("failed reading HTTP probe body: {e}"),
                    None,
                ));
            }
        };
        let total_len = bytes.len();
        let truncated = total_len > MAX_HTTP_PROBE_PREVIEW_BYTES;
        let slice = if truncated {
            &bytes[..MAX_HTTP_PROBE_PREVIEW_BYTES]
        } else {
            &bytes[..]
        };
        let preview = String::from_utf8_lossy(slice).into_owned();

        Ok(tool_ok(json_obj(vec![
            ("status", JsonValue::Int(status as i64)),
            ("headers", JsonValue::Array(headers)),
            ("body_bytes", JsonValue::Int(total_len as i64)),
            ("truncated", JsonValue::Bool(truncated)),
            ("body_preview", JsonValue::Str(preview)),
        ])))
    }

    async fn exec_set_plugin_config(
        &self,
        args: JsonValue,
    ) -> Result<ToolCallOutcome, JsonRpcError> {
        let id = require_str(&args, "id")?;
        let merge = args
            .get("merge")
            .and_then(JsonValue::as_bool)
            .unwrap_or(true);
        let disabled = match args.get("disabled") {
            None | Some(JsonValue::Null) => None,
            Some(JsonValue::Bool(b)) => Some(*b),
            Some(_) => {
                return Err(JsonRpcError::invalid_params(
                    "`disabled` must be boolean",
                ));
            }
        };
        let create_if_missing = args
            .get("create_if_missing")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        let target_name = args
            .get("target_name")
            .and_then(JsonValue::as_str)
            .map(str::to_string);
        let incoming = args
            .get("config")
            .map(json_to_config)
            .unwrap_or(ConfigValue::Null);

        let entries = self.loader.entries();
        let existing = entries.iter().find(|e| e.id == id);

        let patch = if let Some(cur) = existing {
            let next_cfg = if merge {
                merge_config_values(cur.config.clone(), incoming)
            } else {
                incoming
            };
            EntryPatch::ReplaceConfig {
                id: id.clone(),
                config: next_cfg,
                disabled,
            }
        } else if create_if_missing {
            let plugin_name = target_name.unwrap_or_else(|| id.clone());
            EntryPatch::Insert {
                anchor: None,
                entry: PluginEntry {
                    id: id.clone(),
                    target: EntryTarget::Name(plugin_name),
                    config: incoming,
                    disabled: disabled.unwrap_or(false),
                    isolate: Vec::new(),
                    children: Vec::new(),
                },
            }
        } else {
            return Ok(tool_error(
                "UnknownEntry",
                &format!(
                    "entry `{id}` not found; pass create_if_missing=true to insert it"
                ),
                None,
            ));
        };

        self.apply_and_report(vec![patch]).await
    }

    async fn exec_toggle_plugin(
        &self,
        args: JsonValue,
    ) -> Result<ToolCallOutcome, JsonRpcError> {
        let id = require_str(&args, "id")?;
        let enabled = args
            .get("enabled")
            .and_then(JsonValue::as_bool)
            .ok_or_else(|| {
                JsonRpcError::invalid_params("missing boolean `enabled`")
            })?;
        let remove = args
            .get("remove")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        let target_name = args
            .get("target_name")
            .and_then(JsonValue::as_str)
            .map(str::to_string);
        let supplied_cfg = args.get("config").map(json_to_config);

        let entries = self.loader.entries();
        let patch = if let Some(cur) = entries.iter().find(|e| e.id == id) {
            if !enabled && remove {
                EntryPatch::Remove { id }
            } else {
                let next_cfg = match supplied_cfg {
                    Some(c) => merge_config_values(cur.config.clone(), c),
                    None => cur.config.clone(),
                };
                EntryPatch::ReplaceConfig {
                    id,
                    config: next_cfg,
                    disabled: Some(!enabled),
                }
            }
        } else if enabled {
            let plugin_name = target_name.unwrap_or_else(|| id.clone());
            EntryPatch::Insert {
                anchor: None,
                entry: PluginEntry {
                    id,
                    target: EntryTarget::Name(plugin_name),
                    config: supplied_cfg.unwrap_or(ConfigValue::Null),
                    disabled: false,
                    isolate: Vec::new(),
                    children: Vec::new(),
                },
            }
        } else {
            return Ok(tool_ok(json_obj(vec![
                ("changed", JsonValue::Bool(false)),
                ("server", self.build_server_info_json()),
                (
                    "states",
                    self.build_entries_json()
                        .get("states")
                        .cloned()
                        .unwrap_or_else(|| JsonValue::Array(Vec::new())),
                ),
            ])));
        };

        self.apply_and_report(vec![patch]).await
    }

    async fn exec_restart_plugin(
        &self,
        args: JsonValue,
    ) -> Result<ToolCallOutcome, JsonRpcError> {
        let id = require_str(&args, "id")?;
        match self.loader.restart(&id).await {
            Ok(()) => {
                self.ctx.settle().await;
                Ok(tool_ok(json_obj(vec![
                    ("restarted", JsonValue::Str(id)),
                    ("server", self.build_server_info_json()),
                ])))
            }
            Err(err) => Ok(frame_error_outcome(&err)),
        }
    }

    async fn exec_apply_patches(
        &self,
        args: JsonValue,
    ) -> Result<ToolCallOutcome, JsonRpcError> {
        let patches = parse_patches_arg(&args)?;
        self.apply_and_report(patches).await
    }

    async fn exec_apply_manifest(
        &self,
        args: JsonValue,
    ) -> Result<ToolCallOutcome, JsonRpcError> {
        let raw_entries = args
            .get("entries")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| {
                JsonRpcError::invalid_params("missing array `entries`")
            })?;
        let mut entries = Vec::with_capacity(raw_entries.len());
        for item in raw_entries {
            entries.push(parse_entry_json(item)?);
        }
        match self.loader.apply(entries).await {
            Ok(active_ids) => {
                self.ctx.settle().await;
                Ok(tool_ok(json_obj(vec![
                    (
                        "active_ids",
                        JsonValue::Array(
                            active_ids
                                .into_iter()
                                .map(JsonValue::Str)
                                .collect(),
                        ),
                    ),
                    ("server", self.build_server_info_json()),
                    (
                        "entries",
                        self.build_entries_json()
                            .get("entries")
                            .cloned()
                            .unwrap_or_else(|| JsonValue::Array(Vec::new())),
                    ),
                ])))
            }
            Err(err) => Ok(frame_error_outcome(&err)),
        }
    }

    fn exec_set_log_level(
        &self,
        args: JsonValue,
    ) -> Result<ToolCallOutcome, JsonRpcError> {
        let raw = require_str(&args, "level")?;
        let Some(level) = LogLevel::parse(&raw) else {
            return Err(JsonRpcError::invalid_params(format!(
                "invalid log level `{raw}`; expected error|warn|info|debug|trace"
            )));
        };
        let prev = self.control.log_level();
        self.control.set_log_level(level);
        Ok(tool_ok(json_obj(vec![
            ("previous", JsonValue::Str(prev.as_str().to_string())),
            ("current", JsonValue::Str(level.as_str().to_string())),
        ])))
    }

    async fn apply_and_report(
        &self,
        patches: Vec<EntryPatch>,
    ) -> Result<ToolCallOutcome, JsonRpcError> {
        match self.loader.apply_patches(patches).await {
            Ok(active_ids) => {
                self.ctx.settle().await;
                Ok(tool_ok(json_obj(vec![
                    (
                        "active_ids",
                        JsonValue::Array(
                            active_ids
                                .into_iter()
                                .map(JsonValue::Str)
                                .collect(),
                        ),
                    ),
                    ("server", self.build_server_info_json()),
                    (
                        "entries",
                        self.build_entries_json()
                            .get("entries")
                            .cloned()
                            .unwrap_or_else(|| JsonValue::Array(Vec::new())),
                    ),
                ])))
            }
            Err(err) => Ok(frame_error_outcome(&err)),
        }
    }
}

pub fn parse_json_str(raw: &str) -> Result<JsonValue, String> {
    parse(raw)
}

fn require_str(args: &JsonValue, key: &str) -> Result<String, JsonRpcError> {
    args.get(key)
        .and_then(JsonValue::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            JsonRpcError::invalid_params(format!(
                "missing non-empty string `{key}`"
            ))
        })
}

fn parse_patches_arg(
    args: &JsonValue,
) -> Result<Vec<EntryPatch>, JsonRpcError> {
    let arr = args
        .get("patches")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| {
            JsonRpcError::invalid_params("missing array `patches`")
        })?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        out.push(parse_patch_json(item)?);
    }
    Ok(out)
}

fn parse_patch_json(val: &JsonValue) -> Result<EntryPatch, JsonRpcError> {
    let op = val.get("op").and_then(JsonValue::as_str).ok_or_else(|| {
        JsonRpcError::invalid_params(
            "patch object requires string `op` (insert|remove|replace_config)",
        )
    })?;
    match op {
        "insert" => {
            let entry_val = val.get("entry").ok_or_else(|| {
                JsonRpcError::invalid_params("insert patch requires `entry`")
            })?;
            let entry = parse_entry_json(entry_val)?;
            let anchor = match val.get("anchor") {
                None | Some(JsonValue::Null) => None,
                Some(a) => {
                    if let Some(before) =
                        a.get("before").and_then(JsonValue::as_str)
                    {
                        Some(InsertAnchor::Before(before.to_string()))
                    } else if let Some(after) =
                        a.get("after").and_then(JsonValue::as_str)
                    {
                        Some(InsertAnchor::After(after.to_string()))
                    } else {
                        return Err(JsonRpcError::invalid_params(
                            "anchor must be { before: id } or { after: id }",
                        ));
                    }
                }
            };
            Ok(EntryPatch::Insert { anchor, entry })
        }
        "remove" => {
            let id = require_str(val, "id")?;
            Ok(EntryPatch::Remove { id })
        }
        "replace_config" => {
            let id = require_str(val, "id")?;
            let config = val
                .get("config")
                .map(json_to_config)
                .unwrap_or(ConfigValue::Null);
            let disabled = match val.get("disabled") {
                None | Some(JsonValue::Null) => None,
                Some(JsonValue::Bool(b)) => Some(*b),
                Some(_) => {
                    return Err(JsonRpcError::invalid_params(
                        "replace_config `disabled` must be boolean",
                    ));
                }
            };
            Ok(EntryPatch::ReplaceConfig {
                id,
                config,
                disabled,
            })
        }
        other => Err(JsonRpcError::invalid_params(format!(
            "unsupported patch op `{other}`; expected insert|remove|replace_config"
        ))),
    }
}

fn parse_entry_json(val: &JsonValue) -> Result<PluginEntry, JsonRpcError> {
    let id = require_str(val, "id")?;
    let target = match val.get("target") {
        None | Some(JsonValue::Null) => EntryTarget::Name(id.clone()),
        Some(JsonValue::Str(s)) => EntryTarget::Name(s.clone()),
        Some(obj) if obj.as_object().is_some() => {
            let kind = obj
                .get("kind")
                .and_then(JsonValue::as_str)
                .unwrap_or("name");
            match kind {
                "name" => {
                    let name = obj
                        .get("name")
                        .and_then(JsonValue::as_str)
                        .unwrap_or(&id);
                    EntryTarget::Name(name.to_string())
                }
                "group" => EntryTarget::Group,
                other => {
                    return Err(JsonRpcError::invalid_params(format!(
                        "unsupported entry target kind `{other}`; expected name|group"
                    )));
                }
            }
        }
        _ => {
            return Err(JsonRpcError::invalid_params(
                "`entry.target` must be string or object",
            ));
        }
    };
    let config = val
        .get("config")
        .map(json_to_config)
        .unwrap_or(ConfigValue::Null);
    let disabled = val
        .get("disabled")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false);
    let mut children = Vec::new();
    if let Some(arr) = val.get("children") {
        let Some(items) = arr.as_array() else {
            return Err(JsonRpcError::invalid_params(
                "`entry.children` must be an array",
            ));
        };
        for child in items {
            children.push(parse_entry_json(child)?);
        }
    }
    Ok(PluginEntry {
        id,
        target,
        config,
        disabled,
        isolate: Vec::new(),
        children,
    })
}

fn field_schema_to_json(f: &ConfigFieldSchema) -> JsonValue {
    json_obj(vec![
        ("name", JsonValue::Str(f.name.clone())),
        ("type", JsonValue::Str(f.kind.as_str().to_string())),
        ("required", JsonValue::Bool(f.required)),
        ("nullable", JsonValue::Bool(f.nullable)),
        ("sensitive", JsonValue::Bool(f.sensitive)),
        (
            "default",
            f.default.as_ref().map_or(JsonValue::Null, config_to_json),
        ),
        ("description", JsonValue::Str(f.description.clone())),
    ])
}

fn tool_ok(val: JsonValue) -> ToolCallOutcome {
    ToolCallOutcome {
        text: val.to_pretty_string(),
        is_error: false,
        structured_content: Some(val),
    }
}

fn tool_error(
    kind: &str,
    message: &str,
    extra: Option<Vec<(&str, JsonValue)>>,
) -> ToolCallOutcome {
    let mut pairs = vec![
        ("error", JsonValue::Str(kind.to_string())),
        ("message", JsonValue::Str(message.to_string())),
    ];
    if let Some(more) = extra {
        pairs.extend(more);
    }
    let val = json_obj(pairs);
    ToolCallOutcome {
        text: val.to_pretty_string(),
        is_error: true,
        structured_content: Some(val),
    }
}

fn frame_error_outcome(err: &FrameError) -> ToolCallOutcome {
    let issues: Vec<JsonValue> = err
        .issues
        .iter()
        .map(|i| {
            json_obj(vec![
                ("path", JsonValue::Str(i.path.clone())),
                ("message", JsonValue::Str(i.message.clone())),
            ])
        })
        .collect();
    tool_error(
        &format!("{:?}", err.kind),
        &err.message,
        Some(vec![("issues", JsonValue::Array(issues))]),
    )
}

fn empty_object_schema() -> JsonValue {
    json_obj(vec![
        ("type", JsonValue::Str("object".to_string())),
        ("properties", JsonValue::Object(BTreeMap::new())),
    ])
}

fn prop_str(desc: &str) -> JsonValue {
    json_obj(vec![
        ("type", JsonValue::Str("string".to_string())),
        ("description", JsonValue::Str(desc.to_string())),
    ])
}

fn prop_bool(desc: &str) -> JsonValue {
    json_obj(vec![
        ("type", JsonValue::Str("boolean".to_string())),
        ("description", JsonValue::Str(desc.to_string())),
    ])
}

fn prop_obj(desc: &str) -> JsonValue {
    json_obj(vec![
        ("type", JsonValue::Str("object".to_string())),
        ("description", JsonValue::Str(desc.to_string())),
    ])
}

fn id_only_schema() -> JsonValue {
    json_obj(vec![
        ("type", JsonValue::Str("object".to_string())),
        (
            "properties",
            json_obj(vec![("id", prop_str("Managed plugin entry id"))]),
        ),
        (
            "required",
            JsonValue::Array(vec![JsonValue::Str("id".into())]),
        ),
    ])
}

fn set_plugin_config_schema() -> JsonValue {
    json_obj(vec![
        ("type", JsonValue::Str("object".to_string())),
        (
            "properties",
            json_obj(vec![
                ("id", prop_str("Target loader entry id")),
                ("config", prop_obj("Config map (see d_catalog for fields)")),
                (
                    "merge",
                    prop_bool(
                        "Merge with existing entry config when true (default true)",
                    ),
                ),
                ("disabled", prop_bool("Optionally enable/disable the entry")),
                (
                    "create_if_missing",
                    prop_bool("Insert new entry if id is absent"),
                ),
                (
                    "target_name",
                    prop_str(
                        "Registered plugin name when creating (defaults to id)",
                    ),
                ),
            ]),
        ),
        (
            "required",
            JsonValue::Array(vec![JsonValue::Str("id".into())]),
        ),
    ])
}

fn toggle_plugin_schema() -> JsonValue {
    json_obj(vec![
        ("type", JsonValue::Str("object".to_string())),
        (
            "properties",
            json_obj(vec![
                ("id", prop_str("Plugin entry id")),
                ("enabled", prop_bool("Target enabled state")),
                (
                    "remove",
                    prop_bool(
                        "Remove entry completely instead of disabled=true when disabling",
                    ),
                ),
                (
                    "target_name",
                    prop_str(
                        "Registered plugin name when inserting (defaults to id)",
                    ),
                ),
                ("config", prop_obj("Optional config map to merge/apply")),
            ]),
        ),
        (
            "required",
            JsonValue::Array(vec![
                JsonValue::Str("id".into()),
                JsonValue::Str("enabled".into()),
            ]),
        ),
    ])
}

fn patches_input_schema() -> JsonValue {
    json_obj(vec![
        ("type", JsonValue::Str("object".to_string())),
        (
            "properties",
            json_obj(vec![(
                "patches",
                json_obj(vec![
                    ("type", JsonValue::Str("array".to_string())),
                    (
                        "description",
                        JsonValue::Str(
                            "Array of patch objects with op = insert | remove | replace_config".to_string(),
                        ),
                    ),
                ]),
            )]),
        ),
        (
            "required",
            JsonValue::Array(vec![JsonValue::Str("patches".into())]),
        ),
    ])
}

fn manifest_input_schema() -> JsonValue {
    json_obj(vec![
        ("type", JsonValue::Str("object".to_string())),
        (
            "properties",
            json_obj(vec![(
                "entries",
                json_obj(vec![
                    ("type", JsonValue::Str("array".to_string())),
                    (
                        "description",
                        JsonValue::Str(
                            "Complete replacement array of PluginEntry objects"
                                .to_string(),
                        ),
                    ),
                ]),
            )]),
        ),
        (
            "required",
            JsonValue::Array(vec![JsonValue::Str("entries".into())]),
        ),
    ])
}

fn http_probe_schema() -> JsonValue {
    json_obj(vec![
        ("type", JsonValue::Str("object".to_string())),
        (
            "properties",
            json_obj(vec![
                (
                    "method",
                    prop_str(
                        "HTTP method (GET, HEAD, OPTIONS, POST, ...), default GET",
                    ),
                ),
                (
                    "uri",
                    prop_str(
                        "Request path and optional query string, e.g. /guide.md?view=raw",
                    ),
                ),
                (
                    "headers",
                    prop_obj(
                        "Optional map of request header names to string values",
                    ),
                ),
            ]),
        ),
        (
            "required",
            JsonValue::Array(vec![JsonValue::Str("uri".into())]),
        ),
    ])
}

fn log_level_schema() -> JsonValue {
    json_obj(vec![
        ("type", JsonValue::Str("object".to_string())),
        (
            "properties",
            json_obj(vec![(
                "level",
                json_obj(vec![
                    ("type", JsonValue::Str("string".to_string())),
                    (
                        "enum",
                        JsonValue::Array(vec![
                            JsonValue::Str("error".into()),
                            JsonValue::Str("warn".into()),
                            JsonValue::Str("info".into()),
                            JsonValue::Str("debug".into()),
                            JsonValue::Str("trace".into()),
                        ]),
                    ),
                ]),
            )]),
        ),
        (
            "required",
            JsonValue::Array(vec![JsonValue::Str("level".into())]),
        ),
    ])
}
