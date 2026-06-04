/**
 * Typed WebSocket protocol — both directions.
 *
 * Mirrors architecture.md "WebSocket protocol (panel to server)" and
 * features/03_agent-panel.md ## Behaviors. The panel speaks ONLY this protocol
 * to the local server over `ws://${location.host}/ws?token=...`.
 *
 * Discriminated unions (on the `type` field) drive narrowing; the runtime type
 * guards below are the gate for inbound dispatch — anything that fails
 * {@link isServerMessage} is an "unknown type" that the client surfaces as a
 * log entry and NEVER throws on (features/03 ## Behaviors → Event log).
 */

// ---- progress stages (server → client) --------------------------------
/**
 * Progress stages the server may report during an instruct/apply flow.
 * Per architecture.md: rag | rag_warmup | codex | lsp | waiting_build.
 */
export const PROGRESS_STAGES = [
  "rag",
  "rag_warmup",
  "codex",
  "lsp",
  "waiting_build",
] as const;
export type ProgressStage = (typeof PROGRESS_STAGES)[number];

// ---- shared shapes -----------------------------------------------------
/** One entry from the project file list (LIST result). */
export interface FileEntry {
  /** File path within the project. */
  path: string;
  /**
   * EFileType enum NAME (display only; the panel never interprets it). The
   * bridge LIST emits the enum name as a STRING, e.g. "CUIEps" / "GUI"
   * (features/01_lua-bridge.md ## New command: LIST; server bridge_io.list_files
   * parses `path\t<EFileType>`). NOT a numeric tag.
   */
  ftype: string;
  /** Whether SET is allowed (CUI/SCA/RawText true; GUI false). */
  settable: boolean;
}

/**
 * Advisory diagnostic from the epscript-lsp gate. Free-form by design (the
 * server may send strings or structured records); the panel renders best-effort
 * and never blocks Apply on it (rules.md: diagnostics are advisory only).
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

// ---- server → client messages -----------------------------------------
/** `progress {stage, detail?}` — render as a conversation entry with spinner. */
export interface ProgressMessage {
  type: "progress";
  stage: ProgressStage;
  detail?: string;
}

/** `code {code, lang, diff, diagnostics}` — drives the review area. */
export interface CodeMessage {
  type: "code";
  code: string;
  lang: string;
  diff: string;
  diagnostics: Diagnostic[];
}

/** `applied {target}` — apply succeeded. */
export interface AppliedMessage {
  type: "applied";
  target: string;
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
  | ProgressMessage
  | CodeMessage
  | AppliedMessage
  | ErrorMessage
  | StatusMessage
  | ListMessage;

/** All server message `type` discriminants (closed set). */
export const SERVER_MESSAGE_TYPES = [
  "progress",
  "code",
  "applied",
  "error",
  "status",
  "list",
] as const;
export type ServerMessageType = (typeof SERVER_MESSAGE_TYPES)[number];

// ---- client → server messages ------------------------------------------
/** `instruct {instruction, target, useContext}` — run RAG then codex. */
export interface InstructMessage {
  type: "instruct";
  instruction: string;
  target: string;
  useContext: boolean;
}

/** `apply {mode, target, code}` — set an existing file or create a new eps. */
export interface ApplyMessage {
  type: "apply";
  mode: "set" | "neweps";
  target: string;
  code: string;
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
  | InstructMessage
  | ApplyMessage
  | StatusRequest
  | ListRequest;

// ---- runtime type guards (inbound dispatch gate) -----------------------
function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

/** True if `value` is a `progress` message with a known stage. */
export function isProgressMessage(value: unknown): value is ProgressMessage {
  return (
    isObject(value) &&
    value.type === "progress" &&
    typeof value.stage === "string" &&
    (PROGRESS_STAGES as readonly string[]).includes(value.stage)
  );
}

/** True if `value` is a `code` message (code/lang/diff strings, diagnostics array). */
export function isCodeMessage(value: unknown): value is CodeMessage {
  return (
    isObject(value) &&
    value.type === "code" &&
    typeof value.code === "string" &&
    typeof value.lang === "string" &&
    typeof value.diff === "string" &&
    Array.isArray(value.diagnostics)
  );
}

/** True if `value` is an `applied` message. */
export function isAppliedMessage(value: unknown): value is AppliedMessage {
  return (
    isObject(value) &&
    value.type === "applied" &&
    typeof value.target === "string"
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
 * to the log rather than thrown (features/03 ## Behaviors → Event log).
 */
export function isServerMessage(value: unknown): value is ServerMessage {
  return (
    isProgressMessage(value) ||
    isCodeMessage(value) ||
    isAppliedMessage(value) ||
    isErrorMessage(value) ||
    isStatusMessage(value) ||
    isListMessage(value)
  );
}
