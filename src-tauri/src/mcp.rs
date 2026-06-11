//! In-process, loopback streamable-HTTP MCP server exposing the eud-tools registry.
//!
//! Topology (decision A2): codex's MCP transport accepts only `command` (stdio)
//! or `url` (HTTP) — it cannot attach an in-process Rust server directly. So the
//! agent process hosts a **127.0.0.1-only** streamable-HTTP MCP server on an
//! ephemeral port and registers codex against `http://127.0.0.1:<port>/mcp`. The
//! handler runs in the SAME process as the engine, so it shares the live
//! [`ToolRuntime`] (request state, journal, bridge, RAG, mapsafe) directly — the
//! whole point of A2 over an out-of-process shim.
//!
//! rules.md's "panel ↔ core is Tauri IPC only — NO localhost socket" bounds the
//! PANEL boundary; it does not apply to this codex ↔ core MCP channel. The server
//! binds loopback only (rmcp's default `allowed_hosts` is `localhost/127.0.0.1/
//! ::1`), and the codex approval handler already accepts only the `eud-tools`
//! server, so no bearer token is layered on (loopback + ephemeral port is the
//! trust boundary, matching the single-editor-per-machine topology).

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

use crate::tool_exec::ToolRuntime;
use crate::tools::mcp_tool_descriptors;

/// The MCP server name codex registers (matched by the approval handler).
pub const SERVER_NAME: &str = "eud-tools";

/// MCP handler bridging codex tool calls to the shared [`ToolRuntime`].
#[derive(Clone)]
pub struct EudToolHandler {
    runtime: ToolRuntime,
}

impl EudToolHandler {
    pub fn new(runtime: ToolRuntime) -> Self {
        Self { runtime }
    }
}

impl ServerHandler for EudToolHandler {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
            .with_instructions(
                "EUD Editor 3 map tools. Edits go through the connected editor and a change \
journal; mutating tools are gated until search_docs has grounded the change.",
            )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(tool_list()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let name = request.name.to_string();
        let args = Value::Object(request.arguments.unwrap_or_default());
        let runtime = self.runtime.clone();

        // Tool execution does blocking bridge / map file I/O; keep it off the
        // async runtime so the MCP server stays responsive.
        let outcome = tokio::task::spawn_blocking(move || runtime.execute(&name, &args)).await;

        match outcome {
            // A correctable tool error (EvidenceRequired / admission / bridge
            // message) is returned as an MCP tool error so codex can self-correct
            // — never an MCP protocol error.
            Ok(Ok(value)) => Ok(CallToolResult::success(vec![Content::text(render_value(
                &value,
            ))])),
            Ok(Err(message)) => Ok(CallToolResult::error(vec![Content::text(message)])),
            Err(join_error) => Ok(CallToolResult::error(vec![Content::text(format!(
                "tool execution task failed: {join_error}"
            ))])),
        }
    }
}

/// Build the MCP `Tool` list from the registry's MCP descriptors (verbatim
/// inputSchema per tool).
fn tool_list() -> Vec<Tool> {
    mcp_tool_descriptors()
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

/// Start the loopback MCP server on an ephemeral port and return the bound port.
///
/// The server runs as a background task for the app's lifetime; the returned
/// port is injected into codex as `mcp_servers.eud-tools.url`.
pub async fn serve(runtime: ToolRuntime) -> Result<u16, String> {
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

    tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, app).await {
            eprintln!("eud-tools MCP server stopped: {error}");
        }
    });

    Ok(port)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn tool_list_exposes_every_registry_tool_with_its_schema() {
        let tools = tool_list();
        let registry = crate::tools::tool_registry();
        assert_eq!(tools.len(), registry.len());

        // Names round-trip and a representative tool keeps its inputSchema.
        let search = tools
            .iter()
            .find(|tool| tool.name == "search_docs")
            .expect("search_docs must be advertised");
        assert!(search.input_schema.contains_key("properties"));
        assert!(tools.iter().any(|tool| tool.name == "map_info"));
        // SCA is fully defunct — it must never appear as a tool.
        assert!(!tools.iter().any(|tool| tool.name.contains("sca")));
    }

    #[test]
    fn render_value_passes_strings_through_and_json_encodes_objects() {
        assert_eq!(render_value(&Value::String("hello".into())), "hello");
        assert_eq!(
            render_value(&serde_json::json!({"ok": true})),
            "{\"ok\":true}"
        );
    }

    #[tokio::test]
    async fn loopback_server_binds_and_serves_the_mcp_endpoint() {
        let runtime = ToolRuntime::for_tests();
        let port = serve(runtime)
            .await
            .expect("MCP server should bind loopback");

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
