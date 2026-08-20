//! Shared tool services and one request runtime per conversation session.
//!
//! [`ToolServices`] owns app-wide immutable/shared services. Each
//! [`SessionToolRuntime`] is cloned only by one session engine and its MCP
//! handler, so request ids, evidence/plan/budget gates, preflight state, and
//! write tickets cannot overwrite another session.
//!
//! [`SessionToolRuntime::execute`] is the single tool entry point. It verifies
//! write-lease ownership before admitting mutations, applies the existing
//! validation and safety gates, dispatches bridge/RAG/mapsafe operations, and
//! journals every project write for changeset review.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde_json::{json, Value};

use crate::bridge_io::{BridgeIo, SendOpts, HEARTBEAT_STALE_AFTER};
use crate::config::DataDirs;
use crate::edd_runner;
use crate::eps_preflight::{EpsAnalyzer, EpsCandidate, EpsPreflight};
use crate::journal::{DatTable, JournalEntry, JournalStore, JournalTarget, Snapshot, WriteTool};
use crate::mapsafe::{CompilingStatus, IsomEngine, MapSafe, WindowsLockProbe};
use crate::memory::ProjectMemory;
use crate::rag::Rag;
use crate::tools::{self, RequestState};

/// Maximum `search_docs` top-k (mirrors the registry/feature 11 clamp).
const SEARCH_DOCS_MAX_K: i64 = 10;
const SEARCH_DOCS_DEFAULT_K: i64 = 5;

/// Editor build-state probe backed by the editor `status.txt`, resolved from
/// `config.json` on each read (the editor path can change at runtime, and the
/// editor may be down). A read failure reports NOT compiling — the map-write
/// lock probe and the bridge's own compiling guard remain as independent rails.
#[derive(Clone)]
pub struct BridgeCompilingStatus {
    dirs: DataDirs,
}

impl CompilingStatus for BridgeCompilingStatus {
    fn is_compiling(&self) -> bool {
        crate::ipc::bridge_from_config(&self.dirs)
            .ok()
            .and_then(|bridge| bridge.read_status_snapshot(HEARTBEAT_STALE_AFTER).ok())
            .map(|snapshot| snapshot.compiling)
            .unwrap_or(false)
    }
}

/// Production map-write service: bridge-backed compiling guard, Windows share
/// probe, isom static-lib engine.
pub type ProductionMapSafe = MapSafe<BridgeCompilingStatus, WindowsLockProbe, IsomEngine>;

/// Shared, immutable production services. Session workers clone these handles,
/// while every request gate, plan, preflight snapshot, and write ticket remains
/// inside a [`SessionToolRuntime`].
#[derive(Clone)]
pub struct ToolServices {
    dirs: DataDirs,
    journal: JournalStore,
    rag: Arc<Rag>,
    map_safe: Arc<ProductionMapSafe>,
    analyzer: Arc<dyn EpsAnalyzer>,
    writes: crate::write_coordinator::ProjectWriteCoordinator,
}

impl ToolServices {
    pub fn new(
        dirs: DataDirs,
        analyzer: Arc<dyn EpsAnalyzer>,
        writes: crate::write_coordinator::ProjectWriteCoordinator,
    ) -> Self {
        let journal = JournalStore::new(dirs.app_data());
        let rag = Arc::new(load_rag(&dirs));
        let map_safe = Arc::new(MapSafe::new(
            dirs.app_data().to_path_buf(),
            BridgeCompilingStatus { dirs: dirs.clone() },
            WindowsLockProbe,
            IsomEngine,
        ));
        Self {
            dirs,
            journal,
            rag,
            map_safe,
            analyzer,
            writes,
        }
    }

    pub fn session(&self, session_id: impl Into<String>) -> SessionToolRuntime {
        SessionToolRuntime::new(self.clone(), session_id.into())
    }

    pub fn rag(&self) -> Arc<Rag> {
        Arc::clone(&self.rag)
    }

    pub fn writes(&self) -> &crate::write_coordinator::ProjectWriteCoordinator {
        &self.writes
    }
}

#[derive(Debug, Clone)]
struct SessionRequest {
    request_id: String,
    project_id: String,
}

#[derive(Debug, Default)]
struct SessionWriteState {
    ticket: Option<crate::write_coordinator::WriteTicket>,
    reason: Option<String>,
}

/// One session's MCP request state. Clones are shared only by that session's
/// engine and loopback MCP handler.
#[derive(Clone)]
pub struct SessionToolRuntime {
    services: ToolServices,
    session_id: String,
    eps_preflight: Arc<EpsPreflight>,
    request: Arc<Mutex<Option<SessionRequest>>>,
    request_state: Arc<Mutex<Option<RequestState>>>,
    pending_plan: Arc<Mutex<Option<(String, String)>>>,
    write_state: Arc<Mutex<SessionWriteState>>,
    execution_lock: Arc<Mutex<()>>,
}

impl SessionToolRuntime {
    pub fn new(services: ToolServices, session_id: String) -> Self {
        let eps_preflight = Arc::new(EpsPreflight::new(
            services.dirs.clone(),
            Arc::clone(&services.analyzer),
        ));
        Self {
            services,
            session_id,
            eps_preflight,
            request: Arc::new(Mutex::new(None)),
            request_state: Arc::new(Mutex::new(None)),
            pending_plan: Arc::new(Mutex::new(None)),
            write_state: Arc::new(Mutex::new(SessionWriteState::default())),
            execution_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn journal(&self) -> &JournalStore {
        &self.services.journal
    }

    pub fn app_data_dir(&self) -> std::path::PathBuf {
        self.services.dirs.app_data().to_path_buf()
    }

    pub fn data_dirs(&self) -> DataDirs {
        self.services.dirs.clone()
    }

    pub fn begin_request(&self, request_id: &str, project_id: &str) -> Result<(), String> {
        if let Some(ticket) = self.write_state.lock().ticket.as_ref() {
            return Err(format!(
                "previous write ticket {} is still active; settle or abort it before opening {request_id}",
                ticket.request_id()
            ));
        }
        *self.request.lock() = Some(SessionRequest {
            request_id: request_id.to_owned(),
            project_id: project_id.to_owned(),
        });
        *self.request_state.lock() = Some(RequestState::for_request(request_id));
        *self.pending_plan.lock() = None;
        self.eps_preflight.begin_request(request_id);
        Ok(())
    }

    pub fn current_request_id(&self) -> Option<String> {
        self.request
            .lock()
            .as_ref()
            .map(|request| request.request_id.clone())
    }

    pub fn current_project_id(&self) -> Option<String> {
        self.request
            .lock()
            .as_ref()
            .map(|request| request.project_id.clone())
    }

    pub fn approve_current_plan(&self) {
        if let Some(state) = self.request_state.lock().as_mut() {
            state.approve_plan();
        }
    }

    pub fn clear_current(&self) {
        *self.request.lock() = None;
        *self.request_state.lock() = None;
        *self.pending_plan.lock() = None;
    }

    pub fn take_pending_plan(&self, request_id: &str) -> Option<String> {
        let mut pending = self.pending_plan.lock();
        match pending.as_ref() {
            Some((id, _)) if id == request_id => pending.take().map(|(_, markdown)| markdown),
            _ => None,
        }
    }

    pub fn request_write_lane(
        &self,
        reason: impl Into<String>,
    ) -> Result<crate::write_coordinator::WriteTicket, String> {
        let request = self
            .request
            .lock()
            .clone()
            .ok_or_else(|| "no agent request is open".to_string())?;
        let mut write = self.write_state.lock();
        if let Some(ticket) = &write.ticket {
            return Ok(ticket.clone());
        }
        let ticket = self.services.writes.request(
            &request.project_id,
            &self.session_id,
            &request.request_id,
        )?;
        write.reason = Some(reason.into());
        write.ticket = Some(ticket.clone());
        Ok(ticket)
    }

    pub fn restore_review(
        &self,
        project_id: &str,
        request_id: &str,
    ) -> Result<crate::write_coordinator::WriteTicket, String> {
        let ticket =
            self.services
                .writes
                .restore_review(project_id, &self.session_id, request_id)?;
        *self.write_state.lock() = SessionWriteState {
            ticket: Some(ticket.clone()),
            reason: Some("restored pending review".to_string()),
        };
        Ok(ticket)
    }

    pub fn write_ticket(&self) -> Option<crate::write_coordinator::WriteTicket> {
        self.write_state.lock().ticket.clone()
    }

    pub fn write_reason(&self) -> Option<String> {
        self.write_state.lock().reason.clone()
    }

    pub fn owns_write_lease(&self) -> bool {
        let Some(ticket) = self.write_ticket() else {
            return false;
        };
        self.services.writes.owns(
            ticket.project_id(),
            ticket.session_id(),
            ticket.request_id(),
        )
    }

    pub fn release_write_lease(&self) -> Result<bool, String> {
        let ticket = self.write_state.lock().ticket.clone();
        let Some(ticket) = ticket else {
            return Ok(false);
        };
        let released = self.services.writes.release(ticket.request_id())?;
        if released {
            *self.write_state.lock() = SessionWriteState::default();
        }
        Ok(released)
    }

    pub fn cancel_waiting_write(&self) -> Result<bool, String> {
        let ticket = self.write_state.lock().ticket.clone();
        let Some(ticket) = ticket else {
            return Ok(false);
        };
        if !matches!(
            ticket.state(),
            crate::write_coordinator::TicketState::Waiting(_)
        ) {
            return Ok(false);
        }
        let cancelled = self.services.writes.cancel(ticket.request_id())?;
        if cancelled {
            *self.write_state.lock() = SessionWriteState::default();
        }
        Ok(cancelled)
    }

    /// Release a read turn's write intent after the turn itself failed. Read mode
    /// cannot mutate the session workspace or call mutating MCP tools, so a
    /// journal entry here is an invariant violation and must retain the lease.
    pub fn abort_unmutated_write_intent(&self) -> Result<(), String> {
        let Some(ticket) = self.write_ticket() else {
            return Ok(());
        };
        if let Some(request_id) = self.current_request_id() {
            if self.services.journal.entry_count(&request_id) > 0 {
                return Err(format!(
                    "cannot abort write ticket {}; request has journaled mutations",
                    ticket.request_id()
                ));
            }
        }
        if self.owns_write_lease() && self.release_write_lease()? {
            return Ok(());
        }
        if self.services.writes.cancel(ticket.request_id())? {
            *self.write_state.lock() = SessionWriteState::default();
            return Ok(());
        }
        if self.owns_write_lease() && self.release_write_lease()? {
            return Ok(());
        }
        if ticket.state() == crate::write_coordinator::TicketState::Cancelled {
            *self.write_state.lock() = SessionWriteState::default();
            return Ok(());
        }
        Err(format!(
            "failed to abort stale write ticket {}",
            ticket.request_id()
        ))
    }

    pub fn emit_activity(
        &self,
        activity: crate::write_coordinator::SessionActivity,
        queue_position: Option<usize>,
    ) {
        self.services
            .writes
            .emit_activity(self.session_id.clone(), activity, queue_position);
    }

    pub fn execute(&self, tool: &str, args: &Value) -> Result<Value, String> {
        let _execution = self.execution_lock.lock();
        let request_id = self.current_request_id().ok_or_else(|| {
            "no agent request is open; tool calls are only valid during a turn".to_string()
        })?;

        if tools::is_mutating_tool(tool) && !self.owns_write_lease() {
            return Err(
                "WriteLeaseRequired: call request_write_lane with the reason for the change, \
stop this turn, and wait for the backend to resume the same thread in write mode."
                    .to_string(),
            );
        }

        {
            let mut state = self.request_state.lock();
            let state = state
                .as_mut()
                .filter(|state| state.request_id == request_id)
                .ok_or_else(|| format!("request state for {request_id} is missing"))?;
            tools::admit_tool_call(state, tool, args).map_err(|error| error.to_string())?;
        }

        let result = self.dispatch(&request_id, tool, args);
        if result.is_ok() && tool == tools::SEARCH_DOCS_TOOL {
            if let Some(state) = self.request_state.lock().as_mut() {
                state.record_search_docs();
            }
        }
        result
    }

    #[cfg(test)]
    fn request_state_snapshot(&self) -> Option<RequestState> {
        self.request_state.lock().clone()
    }

    fn bridge(&self) -> Result<BridgeIo, String> {
        crate::ipc::bridge_from_config(&self.services.dirs)
    }

    fn dispatch(&self, request_id: &str, tool: &str, args: &Value) -> Result<Value, String> {
        let opts = SendOpts::default();
        match tool {
            // ---- read tools (no journal) ----
            "project_status" => {
                let reply = self.bridge()?.status(&opts, None).map_err(stringify)?;
                Ok(json!({ "status": reply.trim() }))
            }
            "list_files" => {
                let files = self.bridge()?.list(&opts, None).map_err(stringify)?;
                let items: Vec<Value> = files
                    .into_iter()
                    .map(|file| {
                        json!({ "path": file.path, "ftype": file.ftype, "settable": file.settable })
                    })
                    .collect();
                Ok(json!({ "count": items.len(), "files": items }))
            }
            "read_file" => {
                let path = str_arg(args, "path")?;
                let content = self.bridge()?.get(path, &opts, None).map_err(stringify)?;
                Ok(json!({ "path": path, "content": content }))
            }
            tools::EPS_CHECK_TOOL => {
                let files: Vec<EpsCandidate> = serde_json::from_value(
                    args.get("files")
                        .cloned()
                        .ok_or_else(|| "missing argument 'files'".to_string())?,
                )
                .map_err(|error| format!("invalid eps_check files: {error}"))?;
                let result = self.eps_preflight.check(request_id, files)?;
                serde_json::to_value(result)
                    .map_err(|error| format!("failed to serialize eps_check result: {error}"))
            }
            "dat_get" => {
                let (dat, param, obj_id) = (
                    str_arg(args, "dat")?,
                    str_arg(args, "param")?,
                    i64_arg(args, "objId")?,
                );
                let reply = self
                    .bridge()?
                    .getdat(dat, param, obj_id, &opts, None)
                    .map_err(stringify)?;
                Ok(json!({ "value": reply_value(&reply) }))
            }
            "xdat_get" => {
                let (dat, name, obj_id) = (
                    str_arg(args, "dat")?,
                    str_arg(args, "name")?,
                    i64_arg(args, "objId")?,
                );
                let reply = self.send(&format!("GETXDAT {dat}|{name}|{obj_id}"))?;
                Ok(json!({ "value": reply_value(&reply) }))
            }
            "tbl_get" => {
                let index = i64_arg(args, "index")?;
                let reply = self.send(&format!("GETTBL {index}"))?;
                Ok(json!({ "value": reply_value(&reply) }))
            }
            "req_get" => {
                let (dat, obj_id) = (str_arg(args, "dat")?, i64_arg(args, "objId")?);
                let reply = self.send(&format!("GETREQ {dat}|{obj_id}"))?;
                Ok(json!({ "value": reply_value(&reply) }))
            }
            "btn_get" => {
                let set_id = i64_arg(args, "setId")?;
                let reply = self.send(&format!("GETBTN {set_id}"))?;
                Ok(json!({ "csv": reply_value(&reply) }))
            }
            "settings_get" => {
                let (scope, key) = (str_arg(args, "scope")?, str_arg(args, "key")?);
                let reply = self.send(&format!("GETSET {scope}|{key}"))?;
                Ok(json!({ "value": reply_value(&reply) }))
            }
            "plugins_list" => {
                let reply = self.send("PLUGLIST")?;
                Ok(json!({ "plugins": reply.trim() }))
            }
            tools::MAP_INFO_TOOL => {
                let bridge = self.bridge()?;
                tools::map_info(&bridge, args).map_err(stringify)
            }
            tools::SEARCH_DOCS_TOOL => Ok(self.search_docs(args)),
            tools::REQUEST_WRITE_LANE_TOOL => {
                let reason = str_arg(args, "reason")?;
                let ticket = self.request_write_lane(reason)?;
                let (status, queue_position) = match ticket.state() {
                    crate::write_coordinator::TicketState::Granted => ("granted", None),
                    crate::write_coordinator::TicketState::Waiting(position) => {
                        ("waiting", Some(position))
                    }
                    crate::write_coordinator::TicketState::Cancelled => ("cancelled", None),
                };
                Ok(json!({
                    "ok": true,
                    "status": status,
                    "queuePosition": queue_position,
                    "note": "Write intent recorded. Stop this turn now; the backend will resume this thread after the project write lease is granted."
                }))
            }

            // ---- write tools (journaled) ----
            "dat_set" => self.dat_family_set(
                request_id,
                WriteTool::DatSet,
                DatTable::Dat,
                "dat",
                str_arg(args, "param")?,
                args,
            ),
            "xdat_set" => self.dat_family_set(
                request_id,
                WriteTool::XdatSet,
                DatTable::Xdat,
                "xdat",
                str_arg(args, "name")?,
                args,
            ),
            "tbl_set" => self.tbl_set(request_id, args),
            "req_set" => self.req_set(request_id, args),
            "btn_set" => self.btn_set(request_id, args),
            "dat_reset" => self.dat_reset(request_id, args),
            "file_create" => self.file_create(request_id, args),
            "file_write" => self.file_write(request_id, args),
            "file_rename" => self.file_rename(request_id, args),
            "file_delete" => self.file_delete(request_id, args),
            "file_move" => self.file_move(request_id, args),
            "mkdir" => self.mkdir(request_id, args),
            "set_main" => self.set_main(request_id, args),
            "settings_set" => self.settings_set(request_id, args),
            "plugin_add" => self.plugin_add(request_id, args),
            "plugin_edit" => self.plugin_edit(request_id, args),
            "plugin_remove" => self.plugin_remove(request_id, args),
            "plugin_move" => self.plugin_move(request_id, args),
            tools::BUILD_RUN_TOOL => {
                let bridge = self.bridge()?;
                let result = edd_runner::build_run(&bridge)?;
                serde_json::to_value(result)
                    .map_err(|error| format!("failed to serialize build result: {error}"))
            }
            "location_write" => {
                let bridge = self.bridge()?;
                tools::location_write(
                    &bridge,
                    &self.services.map_safe,
                    &self.services.journal,
                    request_id,
                    args,
                )
                .map_err(stringify)
            }
            "player_setup" => {
                let bridge = self.bridge()?;
                tools::player_setup(
                    &bridge,
                    &self.services.map_safe,
                    &self.services.journal,
                    request_id,
                    args,
                )
                .map_err(stringify)
            }
            tools::MEMORY_WRITE_TOOL => self.memory_write(args),
            "propose_plan" => {
                let markdown = str_arg(args, "markdown")?.to_string();
                *self.pending_plan.lock() = Some((request_id.to_owned(), markdown));
                Ok(json!({
                    "ok": true,
                    "note": "Plan recorded for user review. Stop this turn now and wait for the user to approve before applying any change."
                }))
            }
            other => Err(format!("unknown tool '{other}'")),
        }
    }

    // ---- write-tool helpers ----

    fn dat_family_set(
        &self,
        request_id: &str,
        tool: WriteTool,
        table: DatTable,
        kind: &str,
        property: &str,
        args: &Value,
    ) -> Result<Value, String> {
        let obj_id = i64_arg(args, "objId")?;
        let dat = str_arg(args, "dat")?;
        let value = args.get("value").cloned().unwrap_or(Value::Null);
        let value_text = value_to_text(&value);

        // `property` is the param (dat) or field name (xdat); both commands share
        // the `<dat>|<property>|<objId>[|<value>]` shape, only the verb differs.
        let (get_cmd, set_cmd) = if kind == "dat" {
            (
                format!("GETDAT {dat}|{property}|{obj_id}"),
                format!("SETDAT {dat}|{property}|{obj_id}|{value_text}"),
            )
        } else {
            (
                format!("GETXDAT {dat}|{property}|{obj_id}"),
                format!("SETXDAT {dat}|{property}|{obj_id}|{value_text}"),
            )
        };

        let old = json_num_or_str(&reply_value(&self.send(&get_cmd)?));
        let reply = self.send(&set_cmd)?;
        self.record_dat(request_id, tool, table, dat, obj_id, property, old, value)?;
        Ok(json!({ "ok": true, "result": reply.trim() }))
    }

    fn tbl_set(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let index = i64_arg(args, "index")?;
        let value = str_arg(args, "value")?;
        let old = json_num_or_str(&reply_value(&self.send(&format!("GETTBL {index}"))?));
        let reply = self.send(&format!("SETTBL {index}\n{value}"))?;
        self.record_dat(
            request_id,
            WriteTool::TblSet,
            DatTable::Tbl,
            "",
            index,
            "text",
            old,
            Value::String(value.to_string()),
        )?;
        Ok(json!({ "ok": true, "result": reply.trim() }))
    }

    fn req_set(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let (dat, obj_id, payload) = (
            str_arg(args, "dat")?,
            i64_arg(args, "objId")?,
            str_arg(args, "payload")?,
        );
        let old = json_num_or_str(&reply_value(&self.send(&format!("GETREQ {dat}|{obj_id}"))?));
        let reply = self.send(&format!("SETREQ {dat}|{obj_id}\n{payload}"))?;
        self.record_dat(
            request_id,
            WriteTool::ReqSet,
            DatTable::Req,
            dat,
            obj_id,
            "payload",
            old,
            Value::String(payload.to_string()),
        )?;
        Ok(json!({ "ok": true, "result": reply.trim() }))
    }

    fn btn_set(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let (set_id, csv) = (i64_arg(args, "setId")?, str_arg(args, "csv")?);
        let old = json_num_or_str(&reply_value(&self.send(&format!("GETBTN {set_id}"))?));
        let reply = self.send(&format!("SETBTN {set_id}\n{csv}"))?;
        self.record_dat(
            request_id,
            WriteTool::BtnSet,
            DatTable::Btn,
            "",
            set_id,
            "csv",
            old,
            Value::String(csv.to_string()),
        )?;
        Ok(json!({ "ok": true, "result": reply.trim() }))
    }

    fn dat_reset(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let kind = str_arg(args, "kind")?;
        let obj_id = i64_arg(args, "objId")?;
        let dat = args.get("dat").and_then(Value::as_str).unwrap_or("");
        let param = args.get("param").and_then(Value::as_str).unwrap_or("");

        let (get_cmd, table, tool) = match kind {
            "dat" => (
                format!("GETDAT {dat}|{param}|{obj_id}"),
                DatTable::Dat,
                WriteTool::DatSet,
            ),
            "xdat" => (
                format!("GETXDAT {dat}|{param}|{obj_id}"),
                DatTable::Xdat,
                WriteTool::XdatSet,
            ),
            "tbl" => (format!("GETTBL {obj_id}"), DatTable::Tbl, WriteTool::TblSet),
            other => return Err(format!("invalid reset kind '{other}' (dat/xdat/tbl)")),
        };

        let old = json_num_or_str(&reply_value(&self.send(&get_cmd)?));
        let reply = self.send(&format!("RESETDAT {kind}|{dat}|{param}|{obj_id}"))?;
        // The inverse restores the captured old value (was_default:false), so a
        // later real rollback re-sets it rather than resetting again.
        let property = if kind == "tbl" { "text" } else { param };
        let dat = if kind == "tbl" { "" } else { dat };
        self.record_dat(
            request_id,
            tool,
            table,
            dat,
            obj_id,
            property,
            old,
            Value::Null,
        )?;
        Ok(json!({ "ok": true, "result": reply.trim() }))
    }

    // An internal journal-entry builder: the dat/xdat/tbl/req/btn writes all
    // share this exact shape (target coordinates + before/after value), so the
    // argument count is inherent rather than a sign of a missing abstraction.
    #[allow(clippy::too_many_arguments)]
    fn record_dat(
        &self,
        request_id: &str,
        tool: WriteTool,
        table: DatTable,
        dat: &str,
        obj_id: i64,
        property: &str,
        old: Value,
        new: Value,
    ) -> Result<(), String> {
        let obj_id = u32::try_from(obj_id)
            .map_err(|_| "objId must be a non-negative integer".to_string())?;
        let seq = self.next_seq(request_id);
        self.record(JournalEntry {
            id: format!("dat-{seq}"),
            seq,
            tool,
            target: JournalTarget::Dat {
                table,
                dat: dat.to_owned(),
                obj_id,
                property: property.to_owned(),
            },
            before: Snapshot::DatValue {
                value: old,
                was_default: false,
            },
            after: Snapshot::DatValue {
                value: new,
                was_default: false,
            },
            ts: epoch_secs(),
        })
    }

    fn file_create(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let (requested_path, ftype) = (str_arg(args, "path")?, str_arg(args, "ftype")?);
        let code = args.get("code").and_then(Value::as_str).unwrap_or("");
        // The editor stores a script's name WITHOUT its type extension (a
        // natively-created CUIEps `test` has FileName `test`; the editor adds
        // `.eps` for display and at build time). The model, mirroring the
        // LIST/GET paths it reads (e.g. main.eps), passes a path WITH the
        // extension, which would persist as FileName `main.eps` and build to
        // `main.eps.eps`. Strip it so the stored name matches a native file.
        let path = normalize_create_path(requested_path, ftype);
        let reply = self.send(&format!("NEWFILE {path}|{ftype}\n{code}"))?;
        let mirror_path = created_project_path(requested_path, ftype);
        self.eps_preflight
            .write_applied(request_id, &mirror_path, code);
        self.record_file(
            request_id,
            WriteTool::FileCreate,
            &path,
            Snapshot::Created,
            Snapshot::Created,
        )?;
        Ok(json!({ "ok": true, "result": reply.trim() }))
    }

    fn file_write(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let (path, code) = (str_arg(args, "path")?, str_arg(args, "code")?);
        let old = self
            .bridge()?
            .get(path, &SendOpts::default(), None)
            .map_err(stringify)?;
        let reply = self
            .bridge()?
            .set(path, code, &SendOpts::default(), None)
            .map_err(stringify)?;
        self.eps_preflight.write_applied(request_id, path, code);
        self.record_file(
            request_id,
            WriteTool::FileWrite,
            path,
            Snapshot::FileContent { content: old },
            Snapshot::FileContent {
                content: code.to_string(),
            },
        )?;
        Ok(json!({ "ok": true, "result": reply.trim() }))
    }

    fn file_delete(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let path = str_arg(args, "path")?;
        // Best-effort content snapshot for a future restore; a folder GET errors,
        // which we tolerate (the delete still journals as a tail entry).
        let old = self
            .bridge()?
            .get(path, &SendOpts::default(), None)
            .unwrap_or_default();
        let reply = self.send(&format!("DELFILE {path}"))?;
        self.eps_preflight.delete_applied(request_id, path);
        self.record_file(
            request_id,
            WriteTool::FileDelete,
            path,
            Snapshot::DeletedFile {
                content: old,
                position: None,
            },
            Snapshot::Deleted,
        )?;
        Ok(json!({ "ok": true, "result": reply.trim() }))
    }

    fn mkdir(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let path = str_arg(args, "path")?;
        let reply = self.send(&format!("MKDIR {path}"))?;
        self.record_file(
            request_id,
            WriteTool::Mkdir,
            path,
            Snapshot::Created,
            Snapshot::Created,
        )?;
        Ok(json!({ "ok": true, "result": reply.trim() }))
    }

    fn file_rename(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let (path, newname) = (str_arg(args, "path")?, str_arg(args, "newname")?);
        let to = sibling_path(path, newname);
        let reply = self.send(&format!("RENAME {path}\n{newname}"))?;
        self.eps_preflight.rename_applied(request_id, path, &to);
        self.record_rename(request_id, WriteTool::FileRename, path, &to)?;
        Ok(json!({ "ok": true, "result": reply.trim() }))
    }

    fn file_move(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let path = str_arg(args, "path")?;
        let dest = args.get("destFolder").and_then(Value::as_str).unwrap_or("");
        let to = moved_path(path, dest);
        let reply = self.send(&format!("MOVEFILE {path}\n{dest}"))?;
        self.eps_preflight.rename_applied(request_id, path, &to);
        self.record_rename(request_id, WriteTool::FileMove, path, &to)?;
        Ok(json!({ "ok": true, "result": reply.trim() }))
    }

    fn set_main(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let path = str_arg(args, "path")?;
        let old = self
            .send("GETMAIN")
            .map(|reply| reply.trim().to_string())
            .unwrap_or_default();
        let reply = self.send(&format!("SETMAIN {path}"))?;
        let before = Snapshot::MainPath {
            path: (!old.is_empty()).then(|| old.clone()),
        };
        let after = Snapshot::MainPath {
            path: Some(path.to_string()),
        };
        let seq = self.next_seq(request_id);
        self.record(JournalEntry {
            id: format!("main-{seq}"),
            seq,
            tool: WriteTool::SetMain,
            target: JournalTarget::Path {
                path: path.to_string(),
            },
            before,
            after,
            ts: epoch_secs(),
        })?;
        Ok(json!({ "ok": true, "result": reply.trim() }))
    }

    fn settings_set(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let (scope, key, value) = (
            str_arg(args, "scope")?,
            str_arg(args, "key")?,
            str_arg(args, "value")?,
        );
        let old = reply_value(&self.send(&format!("GETSET {scope}|{key}"))?);
        let reply = self.send(&format!("SETSET {scope}|{key}\n{value}"))?;
        let seq = self.next_seq(request_id);
        self.record(JournalEntry {
            id: format!("set-{seq}"),
            seq,
            tool: WriteTool::SettingsSet,
            target: JournalTarget::Setting {
                key: format!("{scope}|{key}"),
            },
            before: Snapshot::SettingValue {
                value: Value::String(old),
            },
            after: Snapshot::SettingValue {
                value: Value::String(value.to_string()),
            },
            ts: epoch_secs(),
        })?;
        Ok(json!({ "ok": true, "result": reply.trim() }))
    }

    fn plugin_add(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let index = args.get("index").and_then(Value::as_i64).unwrap_or(-1);
        let texts = args.get("texts").and_then(Value::as_str).unwrap_or("");
        let reply = self.send(&format!("PLUGADD {index}\n{texts}"))?;
        let at = parse_trailing_index(&reply, "plugadd at ").unwrap_or(index.max(0));
        let seq = self.next_seq(request_id);
        self.record(JournalEntry {
            id: format!("plug-{seq}"),
            seq,
            tool: WriteTool::PluginAdd,
            target: JournalTarget::Plugin {
                plugin_id: at.to_string(),
            },
            before: Snapshot::PluginAbsent,
            after: Snapshot::PluginTexts {
                texts: vec![texts.to_string()],
                index: at as usize,
            },
            ts: epoch_secs(),
        })?;
        Ok(json!({ "ok": true, "result": reply.trim() }))
    }

    fn plugin_edit(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let index = i64_arg(args, "index")?;
        let texts = args.get("texts").and_then(Value::as_str).unwrap_or("");
        let reply = self.send(&format!("PLUGSET {index}\n{texts}"))?;
        let index_usize = index.max(0) as usize;
        // The bridge exposes only a plugin's FIRST line (PLUGLIST), so the old
        // Texts cannot be fully snapshotted here — the before keeps the index for
        // tail-reject targeting, with empty texts (a documented rollback limit).
        let seq = self.next_seq(request_id);
        self.record(JournalEntry {
            id: format!("plug-{seq}"),
            seq,
            tool: WriteTool::PluginEdit,
            target: JournalTarget::Plugin {
                plugin_id: index.to_string(),
            },
            before: Snapshot::PluginTexts {
                texts: Vec::new(),
                index: index_usize,
            },
            after: Snapshot::PluginTexts {
                texts: vec![texts.to_string()],
                index: index_usize,
            },
            ts: epoch_secs(),
        })?;
        Ok(json!({ "ok": true, "result": reply.trim() }))
    }

    fn plugin_remove(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let index = i64_arg(args, "index")?;
        let reply = self.send(&format!("PLUGDEL {index}"))?;
        let index_usize = index.max(0) as usize;
        let seq = self.next_seq(request_id);
        self.record(JournalEntry {
            id: format!("plug-{seq}"),
            seq,
            tool: WriteTool::PluginRemove,
            target: JournalTarget::Plugin {
                plugin_id: index.to_string(),
            },
            before: Snapshot::PluginTexts {
                texts: Vec::new(),
                index: index_usize,
            },
            after: Snapshot::PluginAbsent,
            ts: epoch_secs(),
        })?;
        Ok(json!({ "ok": true, "result": reply.trim() }))
    }

    fn plugin_move(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let (from, to) = (i64_arg(args, "from")?, i64_arg(args, "to")?);
        let reply = self.send(&format!("PLUGMOVE {from}|{to}"))?;
        let seq = self.next_seq(request_id);
        self.record(JournalEntry {
            id: format!("plug-{seq}"),
            seq,
            tool: WriteTool::PluginMove,
            target: JournalTarget::Plugin {
                plugin_id: from.to_string(),
            },
            before: Snapshot::PluginTexts {
                texts: Vec::new(),
                index: from.max(0) as usize,
            },
            after: Snapshot::PluginTexts {
                texts: Vec::new(),
                index: to.max(0) as usize,
            },
            ts: epoch_secs(),
        })?;
        Ok(json!({ "ok": true, "result": reply.trim() }))
    }

    fn record_file(
        &self,
        request_id: &str,
        tool: WriteTool,
        path: &str,
        before: Snapshot,
        after: Snapshot,
    ) -> Result<(), String> {
        let seq = self.next_seq(request_id);
        self.record(JournalEntry {
            id: format!("file-{seq}"),
            seq,
            tool,
            target: JournalTarget::Path {
                path: path.to_string(),
            },
            before,
            after,
            ts: epoch_secs(),
        })
    }

    fn record_rename(
        &self,
        request_id: &str,
        tool: WriteTool,
        from: &str,
        to: &str,
    ) -> Result<(), String> {
        let seq = self.next_seq(request_id);
        self.record(JournalEntry {
            id: format!("file-{seq}"),
            seq,
            tool,
            target: JournalTarget::Rename {
                from: from.to_string(),
                to: to.to_string(),
            },
            before: Snapshot::Path {
                path: from.to_string(),
            },
            after: Snapshot::Path {
                path: to.to_string(),
            },
            ts: epoch_secs(),
        })
    }

    fn memory_write(&self, args: &Value) -> Result<Value, String> {
        let file = str_arg(args, "file")?;
        let content = str_arg(args, "content")?;
        let project = self.current_project();
        if project.is_empty() {
            return Err("no project is open; memory_write needs a connected project".to_string());
        }
        let memory = ProjectMemory::new(self.services.dirs.memory_dir(), project);
        let result = memory.write(file, content);
        if result.ok {
            Ok(json!({ "ok": true, "file": file }))
        } else {
            Err(result.reason)
        }
    }

    fn search_docs(&self, args: &Value) -> Value {
        let query = args.get("query").and_then(Value::as_str).unwrap_or("");
        let k = args
            .get("k")
            .and_then(Value::as_i64)
            .unwrap_or(SEARCH_DOCS_DEFAULT_K)
            .clamp(1, SEARCH_DOCS_MAX_K) as usize;

        // Empty index (no asset yet) returns zero hits. Otherwise hybrid search:
        // lexical substring hits (exact identifiers/Korean terms, no model needed)
        // first, then dense semantic hits fill the rest. A model still warming
        // yields no semantic hits but lexical still works; zero hits either way
        // still lift the evidence gate.
        let hits = if self.services.rag.is_empty() {
            Vec::new()
        } else {
            self.services.rag.search_hybrid(query, k)
        };

        let items: Vec<Value> = hits
            .iter()
            .map(|hit| json!({ "source": hit.source, "text": hit.text, "score": hit.score }))
            .collect();
        let note = if items.is_empty() {
            "no reference document matched; treat affected items as 근거 없음 (일반 EUD 지식) — never fabricate a source"
        } else {
            ""
        };
        json!({ "query": query, "count": items.len(), "hits": items, "note": note })
    }

    fn current_project(&self) -> String {
        self.bridge()
            .ok()
            .and_then(|bridge| bridge.read_status_snapshot(HEARTBEAT_STALE_AFTER).ok())
            .map(|snapshot| snapshot.project)
            .unwrap_or_default()
    }

    fn send(&self, command: &str) -> Result<String, String> {
        self.bridge()?
            .send(command, &SendOpts::default(), None)
            .map_err(stringify)
    }

    fn next_seq(&self, request_id: &str) -> u64 {
        self.services.journal.entry_count(request_id) as u64 + 1
    }

    fn record(&self, entry: JournalEntry) -> Result<(), String> {
        let request_id = self
            .current_request_id()
            .ok_or_else(|| "no open request to journal against".to_string())?;
        self.services
            .journal
            .record(&request_id, entry)
            .map_err(stringify)
    }
}

#[cfg(test)]
impl ToolServices {
    pub fn for_tests() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let base = std::env::temp_dir().join(format!("eud-agent-runtime-test-{nanos}"));
        let dirs = DataDirs::from_bases(&base, &base);
        let analyzer = Arc::new(crate::eps_preflight::NodeEpsAnalyzer::unavailable(
            crate::eps_preflight::SkipReason::AdapterMissing,
            "test runtime has no adapter resource",
        ));
        Self::new(
            dirs,
            analyzer,
            crate::write_coordinator::ProjectWriteCoordinator::silent(),
        )
    }
}

#[cfg(test)]
impl SessionToolRuntime {
    pub fn for_tests() -> Self {
        ToolServices::for_tests().session("test-session")
    }
}

fn load_rag(dirs: &DataDirs) -> Rag {
    let index_path = dirs.rag_dir().join(crate::bootstrap::RAG_INDEX_FILENAME);
    let cache_dir = Some(dirs.models_dir());
    match Rag::from_index_file(&index_path, cache_dir.clone()) {
        Ok(rag) => rag,
        Err(_) => Rag::new(Vec::new(), cache_dir),
    }
}

fn stringify(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn str_arg<'a>(args: &'a Value, name: &str) -> Result<&'a str, String> {
    args.get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing or non-string argument '{name}'"))
}

fn i64_arg(args: &Value, name: &str) -> Result<i64, String> {
    args.get(name)
        .and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        })
        .ok_or_else(|| format!("missing or non-integer argument '{name}'"))
}

/// Render a JSON arg as the bare bridge token: a string passes through, a number
/// stringifies. (Tool-arg validation already rails the accepted shapes.)
fn value_to_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Extract the value after the bridge `OK: ... = <value>` separator (the reply
/// shape every GET* command shares); falls back to the trimmed reply.
fn reply_value(reply: &str) -> String {
    reply
        .split_once(" = ")
        .map(|(_, value)| value.trim().to_string())
        .unwrap_or_else(|| reply.trim().to_string())
}

fn json_num_or_str(text: &str) -> Value {
    match text.parse::<i64>() {
        Ok(number) => Value::from(number),
        Err(_) => Value::String(text.to_string()),
    }
}

/// New full path for a rename: keep the source's parent folder, swap the leaf.
// Type extension the editor appends to a script's base name for display and at
// build time, so file_create must strip it from the leaf to avoid a doubled
// name (CUIEps `main.eps` -> stored FileName `main.eps` -> built `main.eps.eps`;
// a native `test` stores FileName `test`). Only CUIEps is confirmed against an
// editor-native file. CUIPy is almost certainly symmetric (`.py`) but the
// editor exposes no native-create path to verify it, so it stays untouched
// until a build check confirms. RawText keeps its extension — it is part of the
// user-chosen name (e.g. notes.txt), not a fixed type marker.
fn auto_extension(ftype: &str) -> Option<&'static str> {
    match ftype {
        "CUIEps" => Some(".eps"),
        _ => None,
    }
}

// Drop the type's extension from the path's leaf when present, keeping any
// parent folders. Idempotent: a leaf without the extension passes through. A
// leaf that is only the extension (".eps", "folder/.eps") is left intact.
fn normalize_create_path(path: &str, ftype: &str) -> String {
    let Some(ext) = auto_extension(ftype) else {
        return path.to_string();
    };
    match path.strip_suffix(ext) {
        Some(stripped) if !stripped.is_empty() && !stripped.ends_with('/') => stripped.to_string(),
        _ => path.to_string(),
    }
}

fn created_project_path(path: &str, ftype: &str) -> String {
    let Some(extension) = auto_extension(ftype) else {
        return path.to_string();
    };
    if path.ends_with(extension) {
        path.to_string()
    } else {
        format!("{path}{extension}")
    }
}

fn sibling_path(path: &str, newname: &str) -> String {
    match path.rsplit_once('/') {
        Some((parent, _)) => format!("{parent}/{newname}"),
        None => newname.to_string(),
    }
}

/// New full path for a move: the leaf under `dest` (an empty dest = project root).
fn moved_path(path: &str, dest: &str) -> String {
    let leaf = path.rsplit_once('/').map(|(_, leaf)| leaf).unwrap_or(path);
    if dest.is_empty() {
        leaf.to_string()
    } else {
        format!("{dest}/{leaf}")
    }
}

/// Parse the index out of a bridge reply like `OK: plugadd at 3 (12B)`.
fn parse_trailing_index(reply: &str, marker: &str) -> Option<i64> {
    let rest = reply.split_once(marker)?.1;
    let token: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    token.parse().ok()
}

fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eps_preflight::{
        AnalyzerError, AnalyzerRequest, AnalyzerSuccess, EpsDiagnostic, EpsImport,
    };
    use base64::Engine;
    use serde_json::json;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    fn open_runtime(request_id: &str) -> SessionToolRuntime {
        let runtime = SessionToolRuntime::for_tests();
        runtime.begin_request(request_id, "test-project").unwrap();
        runtime.request_write_lane("test mutation").unwrap();
        runtime
    }

    #[test]
    fn execute_without_open_request_is_rejected() {
        // No begin_request -> no live request id to resolve against.
        let runtime = SessionToolRuntime::for_tests();
        let error = runtime
            .execute("project_status", &json!({}))
            .expect_err("a tool call outside a turn must be rejected");
        assert!(error.contains("no agent request is open"), "got: {error}");
    }

    #[test]
    fn failed_read_write_intent_cannot_become_a_ghost_owner() {
        let services = ToolServices::for_tests();
        let runtime = services.session("only-session");
        runtime.begin_request("request-old", "project").unwrap();
        let old = runtime.request_write_lane("write after read").unwrap();
        assert_eq!(old.state(), crate::write_coordinator::TicketState::Granted);

        let error = runtime
            .begin_request("request-new", "project")
            .expect_err("an active ticket must never be silently discarded");
        assert!(error.contains("request-old"));

        runtime.abort_unmutated_write_intent().unwrap();
        runtime.clear_current();
        runtime.begin_request("request-new", "project").unwrap();
        let next = runtime.request_write_lane("retry write").unwrap();
        assert_eq!(
            next.state(),
            crate::write_coordinator::TicketState::Granted,
            "the only session must not queue behind its failed stale request"
        );
    }

    #[test]
    fn every_mutating_tool_requires_the_exact_session_write_lease() {
        let runtime = SessionToolRuntime::for_tests();
        runtime.begin_request("req-read-only", "project").unwrap();

        for spec in tools::tool_registry()
            .into_iter()
            .filter(|spec| spec.mutating)
        {
            let error = runtime
                .execute(spec.name, &json!({}))
                .expect_err("read mode must reject every project mutation");
            assert!(
                error.starts_with("WriteLeaseRequired:"),
                "{} bypassed the write lease: {error}",
                spec.name
            );
        }
    }

    #[test]
    fn opening_one_session_request_does_not_clear_another_sessions_state() {
        let services = ToolServices::for_tests();
        let session_a = services.session("session-a");
        let session_b = services.session("session-b");
        session_a.begin_request("request-a", "project").unwrap();
        {
            let mut state = session_a.request_state.lock();
            let state = state.as_mut().expect("session A state");
            state.record_search_docs();
            state.approve_plan();
            state.mutation_count = 2;
            state.build_fix_attempts = 1;
        }

        session_b.begin_request("request-b", "project").unwrap();

        let state_a = session_a
            .request_state_snapshot()
            .expect("session B must not clear session A");
        assert!(state_a.docs_searched);
        assert!(state_a.plan_approved);
        assert_eq!(state_a.mutation_count, 2);
        assert_eq!(state_a.build_fix_attempts, 1);
        assert_eq!(
            session_b.request_state_snapshot().unwrap().request_id,
            "request-b"
        );
    }

    #[test]
    fn search_docs_with_empty_index_returns_zero_hits_and_lifts_the_evidence_gate() {
        let runtime = open_runtime("req-search");

        // A mutating call BEFORE any search is blocked by the evidence gate.
        let before = runtime
            .execute(
                "dat_set",
                &json!({"dat": "units", "param": "HP", "objId": 0, "value": 100}),
            )
            .expect_err("dat_set before search must hit the evidence gate");
        assert!(before.contains("evidence gate"), "got: {before}");

        // search_docs runs (zero hits on the empty test index) and lifts the gate.
        let result = runtime
            .execute("search_docs", &json!({"query": "마린 생성"}))
            .expect("search_docs should succeed even with an empty index");
        assert_eq!(result["count"], 0);

        // The SAME mutating call now passes admission and reaches execution,
        // failing only because the test runtime has no connected editor — never
        // again on the evidence gate.
        let after = runtime
            .execute(
                "dat_set",
                &json!({"dat": "units", "param": "HP", "objId": 0, "value": 100}),
            )
            .expect_err("no editor is connected in the test runtime");
        assert!(
            !after.contains("evidence gate"),
            "the gate must be lifted after search_docs, got: {after}"
        );
    }

    #[test]
    fn propose_plan_parks_markdown_for_the_engine_to_pick_up() {
        let runtime = open_runtime("req-plan");
        let result = runtime
            .execute("propose_plan", &json!({"markdown": "# Plan\n1. do it"}))
            .expect("propose_plan should record the plan");
        assert_eq!(result["ok"], true);

        // The engine reads this after the turn to end as a plan review; it is a
        // one-shot take keyed by the open request id.
        assert_eq!(
            runtime.take_pending_plan("req-plan").as_deref(),
            Some("# Plan\n1. do it")
        );
        assert_eq!(runtime.take_pending_plan("req-plan"), None);
    }

    struct ReturningAnalyzer {
        calls: AtomicUsize,
    }

    impl EpsAnalyzer for ReturningAnalyzer {
        fn analyze(&self, request: &AnalyzerRequest) -> Result<AnalyzerSuccess, AnalyzerError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(request.candidates.len(), 1);
            assert_eq!(request.candidates[0].path, "main.eps");
            Ok(AnalyzerSuccess {
                checked_files: vec!["main.eps".to_string()],
                diagnostics: Vec::<EpsDiagnostic>::new(),
                imports: Vec::<EpsImport>::new(),
                truncated: false,
                omitted_diagnostics: 0,
                omitted_message_bytes: 0,
            })
        }

        fn reset_project(&self) {}
    }

    #[test]
    fn eps_check_returns_fake_analyzer_result_without_journal_or_budget_changes() {
        let base = std::env::temp_dir().join(format!(
            "eud-agent-runtime-eps-check-{}",
            uuid::Uuid::new_v4()
        ));
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        dirs.ensure_dirs().unwrap();
        let editor = base.join("editor");
        let agent_dir = editor.join("Data").join("agent");
        let inbox = agent_dir.join("inbox");
        let outbox = agent_dir.join("outbox");
        fs::create_dir_all(&inbox).unwrap();
        fs::create_dir_all(&outbox).unwrap();
        let config = crate::config::Config {
            editor_path: editor.to_string_lossy().to_string(),
            ..Default::default()
        };
        dirs.save_config(&config).unwrap();

        let responder_outbox = outbox.clone();
        let responder = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                for entry in fs::read_dir(&inbox).unwrap().filter_map(Result::ok) {
                    let file_name = entry.file_name().to_string_lossy().to_string();
                    if !file_name.starts_with("srv-") || !file_name.ends_with(".cmd") {
                        continue;
                    }
                    let command = fs::read_to_string(entry.path()).unwrap();
                    let Some(token) = command.strip_prefix("EPSNAPSHOT ") else {
                        continue;
                    };
                    let token = token.trim();
                    let snapshot_dir = responder_outbox.join(format!("epsnapshot-{token}"));
                    fs::create_dir_all(&snapshot_dir).unwrap();
                    let code = "function onPluginStart() {}";
                    fs::write(snapshot_dir.join("000001.eps"), code.as_bytes()).unwrap();
                    let manifest = [
                        "EUD-EPSNAPSHOT\t1".to_string(),
                        format!("token\t{token}"),
                        format!(
                            "project\t{}",
                            base64::engine::general_purpose::STANDARD.encode("Project".as_bytes())
                        ),
                        format!(
                            "identity\t{}",
                            base64::engine::general_purpose::STANDARD
                                .encode("Project\nmap.scx".as_bytes())
                        ),
                        format!(
                            "file\t1\tCUIEps\t{}\t{}\tok",
                            base64::engine::general_purpose::STANDARD.encode("main.eps".as_bytes()),
                            code.len()
                        ),
                    ]
                    .join("\n");
                    fs::write(snapshot_dir.join("manifest.tsv"), manifest.as_bytes()).unwrap();
                    let stem = file_name.trim_end_matches(".cmd").to_string();
                    fs::remove_file(entry.path()).unwrap();
                    fs::write(
                        responder_outbox.join(format!("{stem}.result")),
                        b"OK: epsnapshot 1 files",
                    )
                    .unwrap();
                    return;
                }
                assert!(
                    Instant::now() < deadline,
                    "eps snapshot command did not arrive"
                );
                thread::sleep(Duration::from_millis(5));
            }
        });

        let analyzer = Arc::new(ReturningAnalyzer {
            calls: AtomicUsize::new(0),
        });
        let services = ToolServices::new(
            dirs,
            analyzer.clone(),
            crate::write_coordinator::ProjectWriteCoordinator::silent(),
        );
        let runtime = services.session("eps-session");
        runtime.begin_request("req-eps", "project").unwrap();
        let result = runtime
            .execute(
                tools::EPS_CHECK_TOOL,
                &json!({
                    "files": [{"path": "main.eps", "code": "function onPluginStart() {}"}]
                }),
            )
            .unwrap();
        responder.join().unwrap();

        assert_eq!(result["status"], "diagnosed");
        assert_eq!(result["checkedFiles"], json!(["main.eps"]));
        assert_eq!(analyzer.calls.load(Ordering::SeqCst), 1);
        assert_eq!(runtime.journal().entry_count("req-eps"), 0);
        let state = runtime.request_state_snapshot().unwrap();
        assert_eq!(state.action_count, 0);
        assert_eq!(state.mutation_count, 0);
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn unknown_tool_is_rejected_with_a_clear_message() {
        let runtime = open_runtime("req-unknown");
        let error = runtime
            .execute("teleport", &json!({}))
            .expect_err("an unregistered tool must be rejected");
        assert!(error.contains("unknown tool"), "got: {error}");
    }

    #[test]
    fn reply_value_extracts_the_bridge_ok_payload() {
        assert_eq!(reply_value("OK: units|HP|0 = 80"), "80");
        assert_eq!(
            reply_value("OK: project|OpenMapName = C:/maps/x.scx"),
            "C:/maps/x.scx"
        );
        assert_eq!(reply_value("no separator here"), "no separator here");
    }

    #[test]
    fn moved_and_sibling_paths_keep_the_leaf() {
        assert_eq!(sibling_path("folder/a.eps", "b.eps"), "folder/b.eps");
        assert_eq!(sibling_path("a.eps", "b.eps"), "b.eps");
        assert_eq!(moved_path("folder/a.eps", "dest"), "dest/a.eps");
        assert_eq!(moved_path("folder/a.eps", ""), "a.eps");
    }

    #[test]
    fn normalize_create_path_strips_a_doubled_eps_extension() {
        // CUIEps leaf the model suffixed: strip so FileName matches a native
        // file (`test`), which the editor displays/builds as `test.eps`.
        assert_eq!(normalize_create_path("main.eps", "CUIEps"), "main");
        assert_eq!(
            normalize_create_path("folder/main.eps", "CUIEps"),
            "folder/main"
        );
        // Idempotent: no extension to strip passes through unchanged.
        assert_eq!(normalize_create_path("main", "CUIEps"), "main");
        // A leaf that is only the extension is left intact.
        assert_eq!(normalize_create_path(".eps", "CUIEps"), ".eps");
        assert_eq!(
            normalize_create_path("folder/.eps", "CUIEps"),
            "folder/.eps"
        );
        // RawText keeps its extension (part of the user-chosen name); CUIPy is
        // left untouched until verified.
        assert_eq!(normalize_create_path("notes.txt", "RawText"), "notes.txt");
        assert_eq!(normalize_create_path("raw.eps", "RawText"), "raw.eps");
        assert_eq!(normalize_create_path("script.py", "CUIPy"), "script.py");
    }

    #[test]
    fn parse_trailing_index_reads_the_plugadd_slot() {
        assert_eq!(
            parse_trailing_index("OK: plugadd at 3 (12B)", "plugadd at "),
            Some(3)
        );
        assert_eq!(parse_trailing_index("OK: nothing", "plugadd at "), None);
    }
}
