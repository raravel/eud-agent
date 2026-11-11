use super::{should_accept_mcp_elicitation, write_json_rpc_line, AppServerError, AppServerWriter};

/// The paths of each `fileChange` item the app-server announced and has not
/// completed, by item id: `item/fileChange/requestApproval` names only the
/// item, so its paths come from the item's own `item/started`.
pub(super) type FileChangePaths = std::collections::HashMap<String, Vec<String>>;

/// At most this many open `fileChange` items are remembered; a stream that
/// never completes them starts over, so an approval for a forgotten item is
/// declined rather than the map growing.
const MAX_OPEN_FILE_CHANGES: usize = 256;

pub(super) async fn handle_server_request<W>(
    writer: &AppServerWriter<W>,
    method: &str,
    id: serde_json::Value,
    params: Option<&serde_json::Value>,
    native_tools_enabled: bool,
    file_changes: &FileChangePaths,
    root: Option<&std::path::Path>,
) -> Result<(), AppServerError>
where
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let result = match method {
        "item/commandExecution/requestApproval" if native_tools_enabled => {
            serde_json::json!({ "decision": "accept" })
        }
        "execCommandApproval" if native_tools_enabled => {
            serde_json::json!({ "decision": "approved" })
        }
        "item/fileChange/requestApproval"
            if native_tools_enabled && file_change_allowed(method, params, file_changes, root) =>
        {
            serde_json::json!({ "decision": "accept" })
        }
        "applyPatchApproval"
            if native_tools_enabled && file_change_allowed(method, params, file_changes, root) =>
        {
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

/// Remember (or, once completed, forget) the paths of a `fileChange` item
/// from an `item/started`, `item/updated` or `item/completed` notification.
pub(super) fn record_file_change(
    file_changes: &mut FileChangePaths,
    method: &str,
    params: Option<&serde_json::Value>,
) {
    let Some(item) = params.and_then(|params| params.get("item")) else {
        return;
    };
    if !matches!(
        item.get("type").and_then(serde_json::Value::as_str),
        Some("fileChange" | "file_change")
    ) {
        return;
    }
    let Some(item_id) = item.get("id").and_then(serde_json::Value::as_str) else {
        return;
    };
    match method {
        "item/started" | "item/updated" => {
            if file_changes.len() >= MAX_OPEN_FILE_CHANGES && !file_changes.contains_key(item_id) {
                file_changes.clear();
            }
            let paths = item
                .get("changes")
                .and_then(serde_json::Value::as_array)
                .map(|changes| changes.iter().flat_map(change_paths).collect())
                .unwrap_or_default();
            file_changes.insert(item_id.to_string(), paths);
        }
        "item/completed" => {
            file_changes.remove(item_id);
        }
        _ => {}
    }
}

/// The paths one change names: where it writes, and where it moves to.
fn change_paths(change: &serde_json::Value) -> Vec<String> {
    let kind = change.get("kind");
    [
        change.get("path"),
        change.get("movePath"),
        change.get("move_path"),
        kind.and_then(|kind| kind.get("move_path")),
        kind.and_then(|kind| kind.get("movePath")),
    ]
    .into_iter()
    .flatten()
    .filter_map(serde_json::Value::as_str)
    .map(str::to_string)
    .collect()
}

/// Whether a native file change may be applied: every path it touches is
/// known and none is an agent-instruction file. Those go through
/// `fs_write`/`fs_edit`, where the user allows each one; a change whose
/// paths cannot be told is declined the same way. Paths are judged relative
/// to the app-server's project `root` when it is known.
fn file_change_allowed(
    method: &str,
    params: Option<&serde_json::Value>,
    file_changes: &FileChangePaths,
    root: Option<&std::path::Path>,
) -> bool {
    let paths = match method {
        "item/fileChange/requestApproval" => params
            .and_then(|params| params.get("itemId"))
            .and_then(serde_json::Value::as_str)
            .and_then(|item_id| file_changes.get(item_id))
            .cloned(),
        "applyPatchApproval" => params
            .and_then(|params| params.get("fileChanges"))
            .and_then(serde_json::Value::as_object)
            .map(|changes| {
                changes
                    .iter()
                    .flat_map(|(path, change)| {
                        std::iter::once(path.clone()).chain(change_paths(change))
                    })
                    .collect::<Vec<_>>()
            }),
        _ => None,
    };
    paths.is_some_and(|paths| {
        !paths.is_empty()
            && !paths
                .iter()
                .any(|path| crate::fs_tools::touches_agent_instructions(path, root))
    })
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
