/**
 * Typed WebSocket protocol v2 — both directions.
 *
 * Mirrors features/05_agent-core.md "WS protocol v2" and the SERVER emissions
 * (engine.py / agent_runner.py / journal.py). The panel speaks ONLY this
 * protocol to the local server over `ws://${location.host}/ws?token=...`.
 *
 * v2 replaces the v1 single-shot flow (instruct -> rag -> codex -> code event ->
 * manual apply). The v1 `instruct`/`apply`/`code`/`applied` messages are REMOVED
 * ENTIRELY (no compat shim — features/05 line 58).
 *
 *   client -> server: chat / plan_feedback / plan_approve / changeset_decision /
 *                     cancel / status / list
 *   server -> client: agent_event / answer / plan / changeset / rollback_result /
 *                     error / status / progress / list
 *
 * Discriminated unions (on the `type` field) drive narrowing; the runtime type
 * guards below are the gate for inbound dispatch — anything that fails
 * {@link isServerMessage} is an "unknown type" the client surfaces as a log
 * entry and NEVER throws on.
 */

// ---- progress stages (server → client) --------------------------------
/**
 * Progress stages the server may report. `rag_warmup` is kept (features/05:
 * "progress{stage,...} (kept: rag_warmup etc.)"); the warmup callback in app.py
 * emits a free `stage` string, so the panel treats stage as an open string and
 * only special-cases the known ones for labelling.
 */
export const PROGRESS_STAGES = [
  "rag",
  "rag_warmup",
  "codex",
  "lsp",
  "waiting_build",
] as const;
export type KnownProgressStage = (typeof PROGRESS_STAGES)[number];
/** Open string — the server may emit warmup/other stages not in the closed set. */
export type ProgressStage = string;

// ---- shared shapes -----------------------------------------------------
/** One entry from the project file list (LIST result). */
export interface FileEntry {
  /** File path within the project. */
  path: string;
  /**
   * EFileType enum NAME (display only; the panel never interprets it), e.g.
   * "CUIEps" / "GUI". NOT a numeric tag.
   */
  ftype: string;
  /** Whether SET is allowed (CUI/RawText true; GUI false). Display only in v2. */
  settable: boolean;
}

/**
 * Advisory diagnostic from the epscript-lsp gate (rules.md: advisory only). In
 * v2 the server attaches diagnostics per modified/created eps changeset item
 * (features/06 ## Behaviors → Diagnostics). Free-form by design; rendered
 * best-effort and never blocks a decision.
 */
export type Diagnostic =
  | string
  | {
      message?: string;
      text?: string;
      severity?: string;
      line?: number;
      [k: string]: unknown;
    };

/**
 * One changeset item (server `journal.changeset()` shape). The category drives
 * rendering (ChangesetView is a LATER task); the panel keeps the raw shape so a
 * field addition server-side does not break parsing. Every item carries an `id`
 * + `seq` so per-item accept/reject can target it.
 */
export interface ChangesetItem {
  /** "file" | "dat" | tbl/req/btn/settings/plugin/main (flat). */
  category: string;
  /** Stable per-item id (journal entry id) — the decision target. */
  id: string;
  /** Journal sequence (reverse-seq rollback order, render order hint). */
  seq: number;
  /** Remaining server fields (kind/path/diff/dat/objId/properties/old/new/…). */
  [k: string]: unknown;
}

// ---- server → client messages -----------------------------------------
/**
 * `agent_event {kind, detail}` — streamed turn activity. `detail` is a short
 * string. Known kinds (EUD-063 / features/05 "WS protocol v2"):
 *   - `thinking` — generic activity (no user-facing text);
 *   - `reasoning` — a reasoning-text DELTA in `detail`
 *     (`item/reasoning/summaryTextDelta` + `item/reasoning/textDelta`); the panel
 *     accumulates it into the dim/collapsible Reasoning surface;
 *   - `delta` — an answer-text DELTA in `detail`; the panel accumulates it into
 *     the prominent Streamdown Message/Response;
 *   - `tool_call` / `tool_result` — a tool call by name → Tool rows;
 *   - `token_usage` / `turn_done` / `item_started` / `item_completed` / `event` —
 *     internal bookkeeping; the panel surfaces NONE of these raw kind strings as
 *     literal UI text (no-raw-kind-leak contract, decision 06).
 * `kind` is an OPEN string (the server may emit other kinds) — the panel routes
 * the known ones and swallows the rest.
 */
export interface AgentEventMessage {
  type: "agent_event";
  kind: string;
  detail: string;
}

/** `answer {text}` — answer-only turn (no edits). */
export interface AnswerMessage {
  type: "answer";
  text: string;
}

/** `plan {markdown, revision}` — propose_plan ended the turn; revision replaces. */
export interface PlanMessage {
  type: "plan";
  markdown: string;
  revision: number;
}

/** `changeset {request_id, items}` — journaled writes awaiting accept/reject. */
export interface ChangesetMessage {
  type: "changeset";
  request_id: string;
  items: ChangesetItem[];
}

/** `rollback_result {ids, ok}` — outcome of a changeset_decision. */
export interface RollbackResultMessage {
  type: "rollback_result";
  ids: string[];
  ok: boolean;
}

/** `progress {stage, detail?}` — render as a conversation entry. */
export interface ProgressMessage {
  type: "progress";
  stage: ProgressStage;
  detail?: string;
}

/** `error {message}` — flow returns to ready. */
export interface ErrorMessage {
  type: "error";
  message: string;
}

/** `status {compiling, project}` — editor state for the header. */
export interface StatusMessage {
  type: "status";
  compiling: boolean;
  project: string;
}

/** `list {files}` or, when no project is open, `list {error}`. */
export interface ListMessage {
  type: "list";
  files?: FileEntry[];
  error?: string;
}

/** Discriminated union of every documented server → client message. */
export type ServerMessage =
  | AgentEventMessage
  | AnswerMessage
  | PlanMessage
  | ChangesetMessage
  | RollbackResultMessage
  | ProgressMessage
  | ErrorMessage
  | StatusMessage
  | ListMessage;

/** All server message `type` discriminants (closed set). */
export const SERVER_MESSAGE_TYPES = [
  "agent_event",
  "answer",
  "plan",
  "changeset",
  "rollback_result",
  "progress",
  "error",
  "status",
  "list",
] as const;
export type ServerMessageType = (typeof SERVER_MESSAGE_TYPES)[number];

// ---- client → server messages ------------------------------------------
/** `chat {text}` — start a turn (the agent picks files/targets itself). */
export interface ChatMessage {
  type: "chat";
  text: string;
}

/** `plan_feedback {text}` — iterate the plan; resumes the codex thread. */
export interface PlanFeedbackMessage {
  type: "plan_feedback";
  text: string;
}

/** `plan_approve {}` — approve the plan; lifts the mutation gate + resumes. */
export interface PlanApproveMessage {
  type: "plan_approve";
}

/**
 * `changeset_decision {decision, ids}` — accept/reject changeset items. `ids` is
 * the literal "all" (bulk) or a list of item ids. Matches engine.py
 * `_on_changeset_decision` (field `decision`, `ids` = "all" | list).
 */
export interface ChangesetDecisionMessage {
  type: "changeset_decision";
  decision: "accept" | "reject";
  ids: "all" | string[];
}

/** `cancel {}` — interrupt the in-flight turn (journal entries persist). */
export interface CancelMessage {
  type: "cancel";
}

/**
 * `reset {}` — new conversation ([새 대화]). The server drops the retained codex
 * thread so the next `chat` starts a fresh conversation (features/05 EUD-064);
 * the panel clears its log / plan / changeset / per-turn buffers.
 */
export interface ResetMessage {
  type: "reset";
}

/** `status {}` — request editor state. */
export interface StatusRequest {
  type: "status";
}

/** `list {}` — request the project file tree. */
export interface ListRequest {
  type: "list";
}

/** Discriminated union of every documented client → server message. */
export type ClientMessage =
  | ChatMessage
  | PlanFeedbackMessage
  | PlanApproveMessage
  | ChangesetDecisionMessage
  | CancelMessage
  | ResetMessage
  | StatusRequest
  | ListRequest;

/** All client message `type` discriminants (closed set). */
export const CLIENT_MESSAGE_TYPES = [
  "chat",
  "plan_feedback",
  "plan_approve",
  "changeset_decision",
  "cancel",
  "reset",
  "status",
  "list",
] as const;
export type ClientMessageType = (typeof CLIENT_MESSAGE_TYPES)[number];

// ---- runtime type guards (inbound dispatch gate) -----------------------
function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

/** True if `value` is an `agent_event` message. */
export function isAgentEventMessage(value: unknown): value is AgentEventMessage {
  return (
    isObject(value) &&
    value.type === "agent_event" &&
    typeof value.kind === "string" &&
    typeof value.detail === "string"
  );
}

/** True if `value` is an `answer` message. */
export function isAnswerMessage(value: unknown): value is AnswerMessage {
  return (
    isObject(value) &&
    value.type === "answer" &&
    typeof value.text === "string"
  );
}

/** True if `value` is a `plan` message (markdown string, numeric revision). */
export function isPlanMessage(value: unknown): value is PlanMessage {
  return (
    isObject(value) &&
    value.type === "plan" &&
    typeof value.markdown === "string" &&
    typeof value.revision === "number"
  );
}

/** True if `value` is a `changeset` message (request_id + items array). */
export function isChangesetMessage(value: unknown): value is ChangesetMessage {
  return (
    isObject(value) &&
    value.type === "changeset" &&
    typeof value.request_id === "string" &&
    Array.isArray(value.items)
  );
}

/** True if `value` is a `rollback_result` message (ids array + ok bool). */
export function isRollbackResultMessage(
  value: unknown,
): value is RollbackResultMessage {
  return (
    isObject(value) &&
    value.type === "rollback_result" &&
    Array.isArray(value.ids) &&
    typeof value.ok === "boolean"
  );
}

/** True if `value` is a `progress` message (stage is an open string). */
export function isProgressMessage(value: unknown): value is ProgressMessage {
  return (
    isObject(value) &&
    value.type === "progress" &&
    typeof value.stage === "string"
  );
}

/** True if `value` is an `error` message. */
export function isErrorMessage(value: unknown): value is ErrorMessage {
  return (
    isObject(value) &&
    value.type === "error" &&
    typeof value.message === "string"
  );
}

/** True if `value` is a `status` message. */
export function isStatusMessage(value: unknown): value is StatusMessage {
  return (
    isObject(value) &&
    value.type === "status" &&
    typeof value.compiling === "boolean" &&
    typeof value.project === "string"
  );
}

/** True if `value` is a `list` message (files array or error string). */
export function isListMessage(value: unknown): value is ListMessage {
  return (
    isObject(value) &&
    value.type === "list" &&
    (value.files === undefined || Array.isArray(value.files)) &&
    (value.error === undefined || typeof value.error === "string")
  );
}

/**
 * Gate for inbound dispatch: true only for a structurally valid server message
 * of a known type. Anything else is treated as an "unknown type" and surfaced
 * to the log rather than thrown.
 */
export function isServerMessage(value: unknown): value is ServerMessage {
  return (
    isAgentEventMessage(value) ||
    isAnswerMessage(value) ||
    isPlanMessage(value) ||
    isChangesetMessage(value) ||
    isRollbackResultMessage(value) ||
    isProgressMessage(value) ||
    isErrorMessage(value) ||
    isStatusMessage(value) ||
    isListMessage(value)
  );
}
