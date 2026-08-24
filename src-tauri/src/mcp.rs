//! In-process, loopback streamable-HTTP MCP server exposing the eud-tools registry.
//!
//! Topology (decision A2): codex's MCP transport accepts only `command` (stdio)
//! or `url` (HTTP) — it cannot attach an in-process Rust server directly. So the
//! agent process hosts one **127.0.0.1-only** streamable-HTTP server per session
//! on an ephemeral port and registers it as `http://127.0.0.1:<port>/mcp`.
//! The handler shares only that worker's [`SessionToolRuntime`]; no mutable
//! global request pointer identifies callers.
//!
//! rules.md's "panel ↔ core is Tauri IPC only — NO localhost socket" bounds the
//! PANEL boundary; it does not apply to this codex ↔ core MCP channel. The server
//! binds loopback only (rmcp's default `allowed_hosts` is `localhost/127.0.0.1/
//! ::1`), and the codex approval handler already accepts only the `eud-tools`
//! server, so no bearer token is layered on (loopback + ephemeral port is the
//! trust boundary, matching the single-editor-per-machine topology).

use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, CreateElicitationRequestParams,
    ElicitationAction, ElicitationSchema, Implementation, InitializeResult, ListToolsResult, Meta,
    PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use serde_json::Value;

use crate::tool_exec::SessionToolRuntime;
use crate::tools::{map_mcp_tool_descriptors, mcp_tool_descriptors};

/// The MCP server name codex registers (matched by the approval handler).
pub const SERVER_NAME: &str = "eud-tools";

/// MCP handler bridging Codex tool calls to one session's runtime.
#[derive(Clone)]
pub struct EudToolHandler {
    runtime: SessionToolRuntime,
}

impl EudToolHandler {
    pub fn new(runtime: SessionToolRuntime) -> Self {
        Self { runtime }
    }
}

impl ServerHandler for EudToolHandler {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
            .with_instructions(if self.runtime.kind() == crate::session::SessionKind::Map {
                "Map Agent candidate tools. Draft tools can modify only the request-owned candidate; original Apply is not exposed."
            } else {
                "EUD Editor 3 tools. Shared writes use the project coordinator and changeset review."
            })
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(tool_list(
            self.runtime.kind(),
        )))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let name = request.name.to_string();
        let args = Value::Object(request.arguments.unwrap_or_default());
        let runtime = self.runtime.clone();
        if name == crate::tools::ASK_TOOL {
            return match call_ask(context, &args).await {
                Ok(value) => Ok(CallToolResult::success(render_contents(&value))),
                Err(message) => Ok(CallToolResult::error(vec![Content::text(message)])),
            };
        }

        // Tool execution does blocking bridge / map file I/O; keep it off the
        // async runtime so the MCP server stays responsive.
        let outcome = tokio::task::spawn_blocking(move || runtime.execute(&name, &args)).await;

        match outcome {
            // A correctable tool error (EvidenceRequired / admission / bridge
            // message) is returned as an MCP tool error so codex can self-correct
            // — never an MCP protocol error.
            Ok(Ok(value)) => Ok(CallToolResult::success(render_contents(&value))),
            Ok(Err(message)) => Ok(CallToolResult::error(vec![Content::text(message)])),
            Err(join_error) => Ok(CallToolResult::error(vec![Content::text(format!(
                "tool execution task failed: {join_error}"
            ))])),
        }
    }
}

pub(crate) const ASK_ELICITATION_META_KEY: &str = "eudAgentAsk";
pub(crate) const ASK_ELICITATION_PAYLOAD_KEY: &str = "payload";

fn ask_elicitation_request(args: &Value) -> Result<CreateElicitationRequestParams, String> {
    let schema = ElicitationSchema::builder()
        .required_string(ASK_ELICITATION_PAYLOAD_KEY)
        .build()
        .map_err(|error| format!("failed to build ASK elicitation schema: {error}"))?;
    let mut meta = serde_json::Map::new();
    meta.insert(ASK_ELICITATION_META_KEY.to_string(), args.clone());
    Ok(CreateElicitationRequestParams::FormElicitationParams {
        meta: Some(Meta(meta)),
        message: "eud-agent structured ASK".to_string(),
        requested_schema: schema,
    })
}

async fn call_ask(context: RequestContext<RoleServer>, args: &Value) -> Result<Value, String> {
    let result = context
        .peer
        .create_elicitation(ask_elicitation_request(args)?)
        .await
        .map_err(|error| format!("ASK elicitation failed: {error}"))?;

    match result.action {
        ElicitationAction::Accept => {
            let payload = result
                .content
                .as_ref()
                .and_then(Value::as_object)
                .and_then(|content| content.get(ASK_ELICITATION_PAYLOAD_KEY))
                .and_then(Value::as_str)
                .ok_or_else(|| "ASK elicitation response omitted its payload".to_string())?;
            serde_json::from_str(payload)
                .map_err(|error| format!("ASK elicitation returned invalid JSON: {error}"))
        }
        ElicitationAction::Decline => Err("ask request declined".to_string()),
        ElicitationAction::Cancel => Err("ask request cancelled".to_string()),
    }
}

/// Build the MCP `Tool` list from the registry's MCP descriptors (verbatim
/// inputSchema per tool).
fn tool_list(kind: crate::session::SessionKind) -> Vec<Tool> {
    let descriptors = if kind == crate::session::SessionKind::Map {
        map_mcp_tool_descriptors()
    } else {
        mcp_tool_descriptors()
    };
    descriptors
        .into_iter()
        .filter_map(|descriptor| {
            let name = descriptor.get("name")?.as_str()?.to_string();
            let description = descriptor
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let schema = descriptor
                .get("inputSchema")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            Some(Tool::new(name, description, Arc::new(schema)))
        })
        .collect()
}

/// Render a tool result as the MCP text content block: a string passes through;
/// any other JSON value is emitted as compact JSON (MCP content is plain text).
fn render_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => serde_json::to_string(other).unwrap_or_else(|_| other.to_string()),
    }
}

/// Convert a normal JSON tool result to text, except for the map-minimap image
/// envelope. That envelope becomes one compact metadata text block plus a real
/// MCP image block so the model can inspect pixels instead of parsing base64.
fn render_contents(value: &Value) -> Vec<Content> {
    let Some(image) = value.get("image").and_then(Value::as_object) else {
        return vec![Content::text(render_value(value))];
    };
    let Some(data) = image.get("data").and_then(Value::as_str) else {
        return vec![Content::text(render_value(value))];
    };
    let Some(mime_type) = image.get("mimeType").and_then(Value::as_str) else {
        return vec![Content::text(render_value(value))];
    };

    let mut metadata = value.clone();
    if let Some(image) = metadata.get_mut("image").and_then(Value::as_object_mut) {
        image.remove("data");
    }
    vec![
        Content::text(render_value(&metadata)),
        Content::image(data.to_owned(), mime_type.to_owned()),
    ]
}

/// Lifetime handle for one session's loopback MCP endpoint.
pub struct McpServerHandle {
    port: u16,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl McpServerHandle {
    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for McpServerHandle {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.task.abort();
    }
}

/// Start one session-bound loopback MCP server on an ephemeral port.
pub async fn serve(runtime: SessionToolRuntime) -> Result<McpServerHandle, String> {
    let service = StreamableHttpService::new(
        move || Ok(EudToolHandler::new(runtime.clone())),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );

    let app = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|error| format!("eud-tools MCP server failed to bind loopback: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| format!("eud-tools MCP server has no local address: {error}"))?
        .port();
    let (shutdown, stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let server = axum::serve(listener, app).with_graceful_shutdown(async move {
            let _ = stopped.await;
        });
        if let Err(error) = server.await {
            eprintln!("eud-tools MCP server stopped: {error}");
        }
    });

    Ok(McpServerHandle {
        port,
        shutdown: Some(shutdown),
        task,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn tool_list_exposes_every_registry_tool_with_its_schema() {
        let tools = tool_list(crate::session::SessionKind::Eps);
        let registry = crate::tools::tool_registry();
        assert_eq!(tools.len(), registry.len());

        // Names round-trip and a representative tool keeps its inputSchema.
        let search = tools
            .iter()
            .find(|tool| tool.name == "search_docs")
            .expect("search_docs must be advertised");
        assert!(search.input_schema.contains_key("properties"));
        assert!(tools.iter().any(|tool| tool.name == "map_info"));
        assert!(tools.iter().any(|tool| tool.name == "map_minimap"));
        assert!(tools.iter().any(|tool| tool.name == "switch_write"));
        assert!(tools.iter().any(|tool| tool.name == crate::tools::ASK_TOOL));
        // SCA is fully defunct — it must never appear as a tool.
        assert!(!tools.iter().any(|tool| tool.name.contains("sca")));
    }

    #[test]
    fn ask_tool_uses_form_elicitation_without_a_response_deadline() {
        let args = serde_json::json!({
            "questions": [{
                "id": "mode",
                "question": "방식을 고르세요.",
                "options": [{"label": "A"}, {"label": "B"}],
                "multi": false
            }]
        });
        let request = serde_json::to_value(ask_elicitation_request(&args).unwrap()).unwrap();

        assert_eq!(
            request["_meta"][ASK_ELICITATION_META_KEY], args,
            "the app-server callback must receive the original structured questions"
        );
        assert_eq!(
            request["requestedSchema"]["required"],
            serde_json::json!([ASK_ELICITATION_PAYLOAD_KEY])
        );
        assert!(
            request.get("timeout").is_none(),
            "ASK elicitation must not carry a response deadline"
        );
    }

    #[test]
    fn map_tool_list_excludes_original_apply_and_eps_mutations() {
        let tools = tool_list(crate::session::SessionKind::Map);
        let registry = crate::tools::map_tool_registry();
        assert_eq!(tools.len(), registry.len());
        assert!(tools
            .iter()
            .any(|tool| tool.name == "map_candidate_finalize"));
        assert!(tools.iter().any(|tool| tool.name == crate::tools::ASK_TOOL));
        let palette = tools
            .iter()
            .find(|tool| tool.name == "map_palette_query")
            .expect("map_palette_query must be advertised");
        assert_eq!(
            palette.input_schema["properties"]["kind"]["enum"],
            serde_json::json!([
                "brushes",
                "tiles",
                "units",
                "buildings",
                "doodads",
                "sprites"
            ])
        );
        assert!(palette.input_schema["properties"]["kind"]["description"]
            .as_str()
            .is_some_and(|description| description.contains("semanticTerrain")));
        assert!(!tools.iter().any(|tool| tool.name.contains("apply")));
        assert!(!tools.iter().any(|tool| tool.name == "file_write"));
        assert!(!tools.iter().any(|tool| tool.name == "location_write"));
    }

    #[test]
    fn render_value_passes_strings_through_and_json_encodes_objects() {
        assert_eq!(render_value(&Value::String("hello".into())), "hello");
        assert_eq!(
            render_value(&serde_json::json!({"ok": true})),
            "{\"ok\":true}"
        );
    }

    #[test]
    fn render_contents_emits_minimap_as_metadata_plus_image_block() {
        let contents = render_contents(&serde_json::json!({
            "map": {"path": "demo.scx"},
            "image": {
                "mimeType": "image/png",
                "width": 2,
                "height": 1,
                "data": "cG5n",
            },
        }));
        let value = serde_json::to_value(contents).unwrap();

        assert_eq!(value.as_array().unwrap().len(), 2);
        assert_eq!(value[0]["type"], "text");
        assert!(!value[0]["text"].as_str().unwrap().contains("cG5n"));
        assert_eq!(value[1]["type"], "image");
        assert_eq!(value[1]["mimeType"], "image/png");
        assert_eq!(value[1]["data"], "cG5n");
    }

    #[tokio::test]
    async fn loopback_server_binds_and_serves_the_mcp_endpoint() {
        let runtime = SessionToolRuntime::for_tests();
        let server = serve(runtime)
            .await
            .expect("MCP server should bind loopback");
        let port = server.port();

        // A streamable-HTTP MCP initialize round-trip over loopback: the server
        // must accept the handshake (proving the /mcp endpoint is live, routed,
        // and bound to 127.0.0.1) — not refuse the connection.
        let client = reqwest::Client::new();
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            client
                .post(format!("http://127.0.0.1:{port}/mcp"))
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream")
                .body(
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "method": "initialize",
                        "params": {
                            "protocolVersion": "2025-06-18",
                            "capabilities": {},
                            "clientInfo": {"name": "eud-agent-test", "version": "0"}
                        }
                    })
                    .to_string(),
                )
                .send(),
        )
        .await
        .expect("initialize must not hang")
        .expect("initialize must reach the loopback MCP server");

        assert!(
            response.status().is_success(),
            "MCP initialize should be accepted, got {}",
            response.status()
        );
    }
}
