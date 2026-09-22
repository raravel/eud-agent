//! In-process, loopback streamable-HTTP MCP server exposing the eud-tools registry.
//!
//! Topology (decision A2): codex's MCP transport accepts only `command` (stdio)
//! or `url` (HTTP) — it cannot attach an in-process Rust server directly. So the
//! agent process hosts one **127.0.0.1-only** streamable-HTTP server per run
//! on an ephemeral port with an unguessable path unique to that server. The
//! handler holds only that run's [`RunGate`]. A cached URL cannot initialize a
//! later run's handler even when the operating system reuses the same port.
//!
//! rules.md's "panel ↔ core is Tauri IPC only — NO localhost socket" bounds the
//! PANEL boundary; it does not apply to this codex ↔ core MCP channel. The server
//! binds loopback only (rmcp's default `allowed_hosts` is `localhost/127.0.0.1/
//! ::1`), and the codex approval handler already accepts only the `eud-tools`
//! server. The run-specific URL isolates old native clients without a separate
//! bearer-token protocol.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, Implementation, InitializeResult,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use serde_json::Value;

use crate::provider_tool_loop::RunGate;

/// The MCP server name codex registers (matched by the approval handler).
pub const SERVER_NAME: &str = "eud-tools";

static NEXT_HANDLER_ID: AtomicU64 = AtomicU64::new(1);

/// MCP handler bridging native tool calls through one run's admission gate.
#[derive(Clone)]
pub struct EudToolHandler {
    gate: RunGate,
    handler_id: u64,
}

impl EudToolHandler {
    pub fn new(gate: RunGate) -> Self {
        Self {
            gate,
            handler_id: NEXT_HANDLER_ID.fetch_add(1, Ordering::Relaxed),
        }
    }
}

impl ServerHandler for EudToolHandler {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
            .with_instructions(if self.gate.identity().session_kind == crate::session::SessionKind::Map {
                "Map Agent candidate tools. Draft tools can modify only the request-owned candidate; original Apply is not exposed."
            } else {
                "Native EUD project tools. Shared writes use the project coordinator and changeset review."
            })
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(tool_list(
            self.gate.descriptors(),
        )))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let name = request.name.to_string();
        let args = Value::Object(request.arguments.unwrap_or_default());
        let call_id = Some(format!("mcp-{}-{:?}", self.handler_id, context.id));
        match self.gate.dispatch_native(call_id, name, args).await {
            Ok(result) if result.is_error => Ok(CallToolResult::error(vec![Content::text(
                render_value(&result.result),
            )])),
            Ok(result) => Ok(CallToolResult::success(render_contents(&result.result))),
            Err(message) => Ok(CallToolResult::error(vec![Content::text(message)])),
        }
    }
}

/// Build the MCP `Tool` list from the gate's advertised descriptors (verbatim
/// inputSchema per tool), so a native CLI sees exactly what the gate admits.
fn tool_list(descriptors: Vec<Value>) -> Vec<Tool> {
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

/// Lifetime handle for one run's loopback MCP endpoint.
pub struct McpServerHandle {
    port: u16,
    endpoint: String,
    gate: RunGate,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl McpServerHandle {
    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub async fn close_and_drain(&mut self, within: std::time::Duration) -> Result<(), String> {
        let deadline = tokio::time::Instant::now() + within;
        self.gate.close();
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(mut task) = self.task.take() {
            match tokio::time::timeout_at(deadline, &mut task).await {
                Ok(result) => {
                    result.map_err(|error| format!("eud-tools MCP server task failed: {error}"))?
                }
                Err(_) => task.abort(),
            }
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        self.gate.drain(remaining).await
    }
}

impl Drop for McpServerHandle {
    fn drop(&mut self) {
        self.gate.close();
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// Start one run-bound loopback MCP server on an ephemeral port.
pub async fn serve(gate: RunGate) -> Result<McpServerHandle, String> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|error| format!("eud-tools MCP server failed to bind loopback: {error}"))?;
    serve_with_listener(gate, listener)
}

fn serve_with_listener(
    gate: RunGate,
    listener: tokio::net::TcpListener,
) -> Result<McpServerHandle, String> {
    let handler_gate = gate.clone();
    let service = StreamableHttpService::new(
        move || Ok(EudToolHandler::new(handler_gate.clone())),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );

    let path = format!("/mcp/{}", uuid::Uuid::new_v4().simple());
    let app = axum::Router::new().nest_service(&path, service);
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
        endpoint: format!("http://127.0.0.1:{port}{path}"),
        gate,
        shutdown: Some(shutdown),
        task: Some(task),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        provider_runtime::{RunId, RunIdentity},
        tool_exec::SessionToolRuntime,
    };
    use std::time::Duration;

    fn run_gate(runtime: SessionToolRuntime, request_id: &str, generation: u64) -> RunGate {
        RunGate::new(
            RunIdentity {
                session_id: runtime.session_id().to_string(),
                run_id: RunId::new(generation + 1),
                request_id: request_id.to_string(),
                session_kind: runtime.kind(),
                cancellation_generation: generation,
            },
            runtime,
            crate::provider_runtime::WorkspaceAccess::Read,
            None,
        )
    }

    #[test]
    fn tool_list_exposes_every_registry_tool_with_its_schema() {
        let tools = tool_list(crate::tools::mcp_tool_descriptors());
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
        assert!(!tools
            .iter()
            .any(|tool| tool.name == "request_write_workspace"));
        // SCA is fully defunct — it must never appear as a tool (as a name
        // segment: `map_task_discard` legitimately contains the letters).
        assert!(!tools
            .iter()
            .any(|tool| tool.name.split('_').any(|segment| segment == "sca")));
    }

    #[test]
    fn delegated_gate_tool_list_is_exactly_the_profile_plus_submit_result() {
        let runtime = SessionToolRuntime::for_tests();
        let (_cancel, cancellation) = tokio::sync::watch::channel(0_u64);
        runtime.set_cancellation(cancellation);
        runtime.begin_request("delegated-list", "project").unwrap();
        let schema = serde_json::json!({"type": "object", "properties": {"summary": {"type": "string"}}, "required": ["summary"]});
        let gate = RunGate::delegated(
            RunIdentity {
                session_id: runtime.session_id().to_string(),
                run_id: RunId::new(3),
                request_id: "delegated-list".to_string(),
                session_kind: runtime.kind(),
                cancellation_generation: 0,
            },
            runtime,
            crate::provider_tool_loop::DelegatedToolProfile::new(
                ["read_file", "search_docs", "list_files"],
                &schema,
            )
            .unwrap(),
        );
        let names = tool_list(gate.descriptors())
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "list_files",
                "read_file",
                "search_docs",
                crate::provider_tool_loop::SUBMIT_RESULT_TOOL
            ]
        );
    }

    #[test]
    fn map_tool_list_excludes_original_apply_and_eps_mutations() {
        let tools = tool_list(crate::tools::map_mcp_tool_descriptors());
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
        let (_cancel, cancellation) = tokio::sync::watch::channel(0_u64);
        runtime.set_cancellation(cancellation);
        runtime.begin_request("request", "project").unwrap();
        let server = serve(run_gate(runtime, "request", 0))
            .await
            .expect("MCP server should bind loopback");
        // A streamable-HTTP MCP initialize round-trip over loopback: the server
        // must accept the handshake (proving the run endpoint is live, routed,
        // and bound to 127.0.0.1) — not refuse the connection.
        let client = reqwest::Client::new();
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            client
                .post(server.endpoint())
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

    #[tokio::test]
    async fn stale_endpoint_cannot_initialize_when_port_is_reused_by_a_new_run() {
        // Given: a native client initialized against an ended run's URL.
        let runtime = SessionToolRuntime::for_tests();
        let (cancel, cancellation) = tokio::sync::watch::channel(4_u64);
        runtime.set_cancellation(cancellation);
        runtime.begin_request("request-old", "project").unwrap();
        let old_gate = run_gate(runtime.clone(), "request-old", 4);
        let mut old_server = serve(old_gate.clone()).await.unwrap();
        let old_endpoint = old_server.endpoint().to_owned();
        let port = old_server.port();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .pool_max_idle_per_host(0)
            .build()
            .unwrap();
        let old_session = initialize_session(&client, &old_endpoint).await;
        old_server
            .close_and_drain(Duration::from_secs(5))
            .await
            .unwrap();
        runtime.clear_current();
        cancel.send_replace(5);
        runtime.begin_request("request-new", "project").unwrap();
        let new_gate = run_gate(runtime, "request-new", 5);
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .expect("the stopped server's exact port must be reusable");
        let mut new_server = serve_with_listener(new_gate.clone(), listener).unwrap();

        // When: the old client reconnects and then retries initialization without a session.
        let replay = mcp_post(&client, &old_endpoint, tool_call())
            .header("mcp-session-id", old_session)
            .send()
            .await
            .unwrap();
        assert_eq!(replay.status(), reqwest::StatusCode::NOT_FOUND);
        let reinitialize = mcp_post(&client, &old_endpoint, initialize_request())
            .send()
            .await
            .unwrap();

        // Then: fresh initialization cannot cross run authority, while the current URL works.
        assert_eq!(
            reinitialize.status(),
            reqwest::StatusCode::NOT_FOUND,
            "an old URL must not initialize a handler for the new run"
        );
        assert!(new_gate.completed().is_empty());
        let new_session = initialize_session(&client, new_server.endpoint()).await;
        let response = mcp_post(&client, new_server.endpoint(), tool_call())
            .header("mcp-session-id", new_session)
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        let body = response.text().await.unwrap();
        let result: Value = body
            .lines()
            .find_map(|line| {
                line.strip_prefix("data: ")
                    .and_then(|data| serde_json::from_str(data).ok())
            })
            .expect("tool response must contain an SSE JSON-RPC result");
        assert_eq!(result["id"], 2);
        assert_eq!(result["result"]["isError"], false);
        new_gate.drain(Duration::from_secs(5)).await.unwrap();
        let receipt_path = new_gate.receipt_path().unwrap();
        let receipt: Value =
            serde_json::from_slice(&std::fs::read(&receipt_path).unwrap()).unwrap();
        assert_eq!(receipt["requestId"], "request-new");
        assert_eq!(receipt["runId"], 6);
        assert_eq!(receipt["cancellationGeneration"], 5);
        let completions = receipt["completions"].as_array().unwrap();
        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0]["requestId"], "request-new");
        assert_eq!(completions[0]["runId"], 6);
        assert_eq!(completions[0]["name"], "search_docs");
        assert_eq!(completions[0]["isError"], false);
        assert!(old_gate.completed().is_empty());
        assert!(!old_gate.receipt_path().unwrap().exists());
        new_gate.acknowledge_receipts().unwrap();
        new_server
            .close_and_drain(Duration::from_secs(5))
            .await
            .unwrap();
    }

    fn mcp_post(client: &reqwest::Client, endpoint: &str, body: Value) -> reqwest::RequestBuilder {
        client
            .post(endpoint)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(body.to_string())
    }

    fn initialize_request() -> Value {
        serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": {"name": "endpoint-generation-test", "version": "0"}}
        })
    }

    fn tool_call() -> Value {
        serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": "search_docs", "arguments": {"query": "endpoint generation", "k": 1}}
        })
    }

    async fn initialize_session(client: &reqwest::Client, endpoint: &str) -> String {
        let response = mcp_post(client, endpoint, initialize_request())
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        let session = response
            .headers()
            .get("mcp-session-id")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let _body = response.text().await.unwrap();
        let initialized = mcp_post(
            client,
            endpoint,
            serde_json::json!({
                "jsonrpc": "2.0", "method": "notifications/initialized"
            }),
        )
        .header("mcp-session-id", &session)
        .send()
        .await
        .unwrap();
        assert!(initialized.status().is_success());
        session
    }

    #[tokio::test]
    async fn production_mcp_endpoint_rejects_a_stale_handler_after_request_rotation() {
        // Given: an initialized production streamable-HTTP MCP client bound to one run.
        let runtime = SessionToolRuntime::for_tests();
        let (_cancel, cancellation) = tokio::sync::watch::channel(4_u64);
        runtime.set_cancellation(cancellation);
        runtime.begin_request("request-old", "project").unwrap();
        let gate = run_gate(runtime.clone(), "request-old", 4);
        let server = serve(gate.clone()).await.unwrap();
        let endpoint = server.endpoint().to_owned();
        let client = reqwest::Client::new();
        let initialize = client
            .post(&endpoint)
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
                        "clientInfo": {"name": "stale-client-test", "version": "0"}
                    }
                })
                .to_string(),
            )
            .send()
            .await
            .unwrap();
        let session_id = initialize
            .headers()
            .get("mcp-session-id")
            .expect("initialize response must bind an MCP session")
            .to_str()
            .unwrap()
            .to_string();
        assert!(initialize.status().is_success());
        let initialized = client
            .post(&endpoint)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("mcp-session-id", &session_id)
            .body(
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/initialized"
                })
                .to_string(),
            )
            .send()
            .await
            .unwrap();
        assert!(initialized.status().is_success());
        let execution_lock = runtime.execution_lock_for_tests();
        let (locked, ready) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        let blocker = std::thread::spawn(move || {
            let _guard = execution_lock.lock();
            locked.send(()).unwrap();
            released.recv().unwrap();
        });
        ready.recv().unwrap();
        let request_endpoint = endpoint.clone();
        let request_client = client.clone();
        let request_session = session_id.clone();
        let disconnected = tokio::spawn(async move {
            request_client
                .post(request_endpoint)
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream")
                .header("mcp-session-id", request_session)
                .body(
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 2,
                        "method": "tools/call",
                        "params": {"name": "list_files", "arguments": {}}
                    })
                    .to_string(),
                )
                .send()
                .await
        });
        gate.wait_for_admission().await;
        disconnected.abort();
        let _ = disconnected.await;
        gate.cancel();
        let rotation_error = runtime
            .begin_request("request-new", "project")
            .expect_err("an admitted old tool must block request rotation");
        assert!(rotation_error.contains("previously admitted tool"));
        release.send(()).unwrap();
        blocker.join().unwrap();
        gate.drain(Duration::from_secs(5)).await.unwrap();
        let receipt_path = gate.receipt_path().unwrap();
        let receipt: Value =
            serde_json::from_slice(&std::fs::read(&receipt_path).unwrap()).unwrap();
        assert_eq!(receipt["completions"].as_array().unwrap().len(), 1);
        assert_eq!(receipt["completions"][0]["name"], "list_files");
        runtime.clear_current();
        runtime.begin_request("request-new", "project").unwrap();

        // When: that old client calls a valid tool after the new request is active.
        let response = client
            .post(&endpoint)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("mcp-session-id", session_id)
            .body(
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "tools/call",
                    "params": {"name": "list_files", "arguments": {}}
                })
                .to_string(),
            )
            .send()
            .await
            .unwrap();
        let body = response.text().await.unwrap();

        // Then: the real endpoint returns a stale-run error and adds no second completion.
        assert!(body.contains("stale provider run"), "response was: {body}");
        assert_eq!(gate.completed().len(), 1);
        gate.acknowledge_receipts().unwrap();
        assert!(!receipt_path.exists());
    }
}
