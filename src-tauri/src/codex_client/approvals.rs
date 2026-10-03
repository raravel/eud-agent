use super::{should_accept_mcp_elicitation, write_json_rpc_line, AppServerError, AppServerWriter};

pub(super) async fn handle_server_request<W>(
    writer: &AppServerWriter<W>,
    method: &str,
    id: serde_json::Value,
    params: Option<&serde_json::Value>,
    native_tools_enabled: bool,
) -> Result<(), AppServerError>
where
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let result = match method {
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval"
            if native_tools_enabled =>
        {
            serde_json::json!({ "decision": "accept" })
        }
        "execCommandApproval" | "applyPatchApproval" if native_tools_enabled => {
            serde_json::json!({ "decision": "approved" })
        }
        _ if should_accept_mcp_elicitation(method, params) => {
            serde_json::json!({ "action": "accept", "content": null })
        }
        _ => decline_approval_result(method),
    };

    write_json_rpc_line(
        writer,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }),
    )
    .await
}

fn decline_approval_result(method: &str) -> serde_json::Value {
    match method {
        "mcpServer/elicitation/request" => {
            serde_json::json!({ "action": "decline", "content": null })
        }
        "item/fileChange/requestApproval" | "item/permissions/requestApproval" => {
            serde_json::json!({ "decision": "decline" })
        }
        "applyPatchApproval" => serde_json::json!({ "decision": "denied" }),
        _ => serde_json::json!({ "decision": "decline" }),
    }
}
