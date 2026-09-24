/**
 * Typed Tauri IPC protocol v2, both directions.
 *
 * Mirrors features/05_agent-core.md v2 chat schema and the core emissions,
 * retargeted to in-process Tauri IPC. The panel speaks ONLY this protocol to
 * the local core through `invoke` commands and Tauri `listen` events.
 *
 * v2 replaces the v1 single-shot flow (instruct -> rag -> codex -> code event ->
 * manual apply). The v1 `instruct`/`apply`/`code`/`applied` messages are
 * REMOVED ENTIRELY (no compat shim - features/05 line 58).
 *
 *   panel -> core: chat / plan_feedback / plan_approve /
 *                  cancel / reset / status / list
 *   core -> panel: agent_event / answer / plan / git /
 *                  error / status / progress / list
 *
 * Discriminated unions (on the `type` field) drive narrowing; the runtime type
 * guards below are the gate for inbound dispatch. Anything that fails
 * {@link isServerMessage} is an "unknown type" the client surfaces as a log
 * entry and NEVER throws on.
 */
import {
  PROVIDER_IDS,
  isProviderId,
  isProviderStatus,
  type ProviderId,
  type ProviderBinding,
  type ProviderStatus,
} from "@/providers/types";


// ---- progress stages (core -> panel) ----------------------------------
/**
 * Progress stages the core may report. `rag_warmup` is kept (features/05:
 * "progress{stage,...} (kept: rag_warmup etc.)"); the warmup callback emits a
 * free `stage` string, so the panel treats stage as an open string and only
 * special-cases the known ones for labelling.
 */
export const PROGRESS_STAGES = [
  "rag",
  "rag_warmup",
  "codex",
  "provider",
  "compaction",
  "task_state_warning",
  "large_context_fallback",
  "workspace",
  "lsp",
  "waiting_build",
  "trace_test",
  "bootstrap",
  "euddraft_update",
  "audio_probe",
  "audio_transcode",
  "audio_validate",
  "waiting_map_close",
  "map_sound_write",
  "map_sound_verify",
] as const;
export type KnownProgressStage = (typeof PROGRESS_STAGES)[number];
/** Open string - the core may emit warmup/other stages not in the closed set. */
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

/** One durable document in the project's `.eud-agent/workspace` tree. */
export interface WorkspaceFileEntry {
  path: string;
  size: number;
  /** Parent-owned authoritative acceptance state; absent for unreviewed files. */
  state?: string;
  revision?: number;
}

/** `workspace_list` command output. */
/**
 * `workspace_list` command output: every file below the project root,
 * project-root-relative. Accepted/approved harness documents live under
 * {@link WORKSPACE_DOCUMENT_PREFIX} and carry `state`/`revision`.
 */
export interface WorkspaceListResponse {
  project: string;
  workspaceId: string;
  files: WorkspaceFileEntry[];
}

/** Project-relative prefix of the agent's document tree. */
export const WORKSPACE_DOCUMENT_PREFIX = ".eud-agent/workspace/";

/**
 * `workspace_read` command output. `content` is null when the file is listed
 * but not viewable as text; `unreadable` then says why.
 */
export interface WorkspaceReadResponse {
  workspaceId: string;
  path: string;
  size: number;
  content: string | null;
  unreadable?: "binary" | "too_large";
}

/** `workspace_search` command output. */
export interface WorkspaceSearchResponse {
  workspaceId: string;
  query: string;
  paths: string[];
}

/**
 * Advisory diagnostic from the epscript-lsp gate (rules.md: advisory only).
 * Free-form by design; rendered best-effort and never blocks anything.
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
 * One changeset item (core journal.changeset() shape). Request review is gone —
 * git records a turn instead — but the HARNESS document review still carries a
 * changeset of its own, so the shape survives for {@link HarnessChangeset}. The
 * panel keeps the raw fields so a core-side addition does not break parsing.
 */
export interface ChangesetItem {
  /** "file" | "dat" | tbl/req/btn/settings/plugin/main (flat). */
  category: string;
  /** Stable per-item id (journal entry id). */
  id: string;
  /** Journal sequence (reverse-seq rollback order, render order hint). */
  seq: number;
  /** Remaining core fields (kind/path/diff/dat/objId/properties/old/new/...). */
  [k: string]: unknown;
}

/** Project-memory markdown files editable in the panel memory view. */
export const MEMORY_FILES = [
  "resources",
  "structure",
  "conventions",
  "lessons",
] as const;
export type MemoryFile = (typeof MEMORY_FILES)[number];


// ---- core -> panel messages -------------------------------------------
/**
 * `agent_event {kind, detail}` - streamed turn activity. `detail` is a short
 * string. Known kinds (EUD-063 / features/05 v2 chat schema):
 *   - `thinking` - generic activity (no user-facing text);
 *   - `reasoning` - a reasoning-text DELTA in `detail`
 *     (`item/reasoning/summaryTextDelta` + `item/reasoning/textDelta`); the panel
 *     accumulates it into the dim/collapsible Reasoning surface;
 *   - `delta` - an answer-text DELTA in `detail`; the panel accumulates it into
 *     the prominent Streamdown Message/Response;
 *   - `tool_call` / `tool_result` - a tool call by name -> Tool rows;
 *   - `token_usage` / `turn_done` / `item_started` / `item_completed` / `event` -
 *     internal bookkeeping; the panel surfaces NONE of these raw kind strings as
 *     literal UI text (no-raw-kind-leak contract, decision 06).
 * `kind` is an OPEN string (the core may emit other kinds) - the panel routes
 * the known ones and swallows the rest.
 */
interface SessionScopedMessage {
  /** Immutable owner of every conversation event. */
  sessionId: string;
}

interface OptionalSessionScopedMessage {
  /** Turn progress is scoped; bootstrap/RAG progress remains global. */
  sessionId?: string;
}

export interface AgentEventMessage extends SessionScopedMessage {
  type: "agent_event";
  kind: string;
  detail: string;
  /**
   * EUD-068: optional tool payload - `tool_call` carries `args` (the call's
   * argument text, core-truncated); `tool_result` carries `result` (the
   * result/error text) + `status` ("completed" | "failed" | "declined").
   */
  data?: {
    callId?: string;
    args?: string;
    result?: string;
    status?: string;
  };
}

/** Token counts from one Codex model response. */
export interface TokenUsageBreakdown {
  inputTokens: number;
  cachedInputTokens: number;
  cacheWriteInputTokens: number;
  outputTokens: number;
  reasoningOutputTokens: number;
  totalTokens: number;
}

/** Active context and cumulative token usage for one Codex thread. */
export interface ContextUsage {
  last: TokenUsageBreakdown;
  total: TokenUsageBreakdown;
  modelContextWindow: number | null;
}

/** `context_usage` — latest typed usage snapshot for one session. */
export interface ContextUsageMessage extends SessionScopedMessage {
  type: "context_usage";
  turnId: string;
  tokenUsage: ContextUsage;
}

/** `answer {text}` - answer-only turn (no edits). */
export interface AnswerMessage extends SessionScopedMessage {
  type: "answer";
  text: string;
}

/** `plan {markdown, revision}` - propose_plan ended the turn; revision replaces. */
export interface PlanMessage extends SessionScopedMessage {
  type: "plan";
  markdown: string;
  revision: number;
}
export interface AskOption {
  label: string;
  description?: string;
}

export interface AskQuestion {
  id: string;
  header?: string;
  question: string;
  options?: AskOption[];
  multi: boolean;
}

/** `ask` pauses the current tool call until the user submits every answer. */
/** Lifecycle of one `ask` request: answerable, or closed because its bounded wait elapsed. */
export type AskEventStatus = "pending" | "expired";

export interface AskMessage extends SessionScopedMessage {
  type: "ask";
  requestId: string;
  /** Absent means `pending` (older cores). */
  status?: AskEventStatus;
  /** Bounded wait in whole seconds before the request expires. */
  waitSeconds?: number;
  questions: AskQuestion[];
}

export interface AskAnswer {
  answers: string[];
}

/** Map layers a team map task may change. */
export type MapTaskLayer =
  | "terrain"
  | "units"
  | "buildings"
  | "doodads"
  | "sprites"
  | "locations";
export const MAP_TASK_LAYERS: readonly MapTaskLayer[] = [
  "terrain",
  "units",
  "buildings",
  "doodads",
  "sprites",
  "locations",
] as const;

/** Lifecycle of one EPS → Map team task (`{kind, reason?}` on the wire). */
export type TeamTaskStatus =
  | { kind: "queued" }
  | { kind: "running" }
  | { kind: "candidate_ready" }
  | { kind: "applied" }
  | { kind: "discarded" }
  | { kind: "failed"; reason: string }
  | { kind: "cancelled" }
  | { kind: "interrupted" }
  | { kind: "superseded" };
export const TEAM_TASK_STATUS_KINDS = [
  "queued",
  "running",
  "candidate_ready",
  "applied",
  "discarded",
  "failed",
  "cancelled",
  "interrupted",
  "superseded",
] as const;
export type TeamTaskStatusKind = (typeof TEAM_TASK_STATUS_KINDS)[number];

/** The candidate revision a team task produced. */
export interface TeamCandidateSummary {
  revision: number;
  revisionKey: string;
  mapSha256: string;
  summary: string;
  terrainCells: number;
  units: number;
  buildings: number;
  doodads: number;
  sprites: number;
  locations: number;
}

/** One EPS → Map team handoff, as persisted on the EPS session. */
export interface TeamTask {
  id: string;
  parentRequestId: string;
  mapSessionId: string;
  mapRequestId?: string;
  goal: string;
  layers: MapTaskLayer[];
  selectionIds?: string[];
  locationIds?: number[];
  sourceMapSha256AtCreate: string;
  status: TeamTaskStatus;
  candidate?: TeamCandidateSummary;
  appliedSourceSha256?: string;
  /** Who applied the candidate: the user in the Map window or the EPS agent. */
  appliedBy?: TeamApplyActor;
  createdAt: number;
  updatedAt: number;
}

export type TeamApplyActor = "user" | "agent";

/**
 * `team_task` — a team task changed (created, settled, applied, discarded).
 * `continuation` is present when the engine itself started the session's
 * continuation turn on that text (the task settled after `map_task_request`
 * had returned `running`): the panel records the turn as sent.
 */
export interface TeamTaskMessage extends SessionScopedMessage {
  type: "team_task";
  task: TeamTask;
  continuation?: TeamTaskContinuation;
}

export interface TeamTaskContinuation {
  clientTurnId: string;
  text: string;
}

/** One commit the app recorded, as the `git` event reports it. */
export interface CommitRecord {
  sha: string;
  subject: string;
  /** How many paths the commit changed. */
  files: number;
}

/**
 * `git {external?, turn?, warning?}` - what the turn boundary recorded.
 *
 * `turn` is the commit this turn produced. `external` is the separate commit
 * that captured what changed OUTSIDE the app (the user's own SCMDraft or editor
 * edit) before the turn ran, which is why the panel says so out loud. `warning`
 * means the work happened but the record did not.
 */
export interface GitMessage extends SessionScopedMessage {
  type: "git";
  external?: CommitRecord;
  turn?: CommitRecord;
  warning?: string;
}

/** Where a project's repository came from, which decides whose history it is. */
export type RepoOrigin = "app" | "preexisting";

/** Whether the app may commit into this repository. */
export type RepoConsent = "pending" | "granted" | "declined";

/** `git_state` / `git_consent_set` result. */
export interface RepoState {
  /** `git` is on PATH. */
  available: boolean;
  /** The project root is inside a git work tree. */
  tracked: boolean;
  /** The work tree's top level is above the project root. */
  nested: boolean;
  origin: RepoOrigin | null;
  consent: RepoConsent;
  /** What the user has to be told once, in Korean, or nothing. */
  warning: string | null;
}

/** One entry of the project's history. */
export interface CommitSummary {
  sha: string;
  subject: string;
  /** Committer date, SECONDS since the epoch (not milliseconds). */
  timestamp: number;
}

/** What happened to one file in a commit. */
export interface CommitFile {
  path: string;
  insertions: number;
  deletions: number;
  /** Git could not diff this file as text (a map, an image, a wheel). */
  binary: boolean;
  /** The unified diff, when it is carried. */
  patch?: string;
  /** Why the patch is absent, in Korean, when it is. */
  omitted?: string;
}

/** One commit as the history view shows it. */
export interface CommitDetail {
  sha: string;
  subject: string;
  /** The rest of the message — the session and request the turn belonged to. */
  body: string;
  timestamp: number;
  files: CommitFile[];
}

export type HarnessJobStatus =
  | "waiting_runtime"
  | "pending"
  | "running"
  | "review"
  | "failed"
  | "completed"
  | "rejected"
  | "skipped";

export type RuntimeVerification = "not_required" | "waiting" | "confirmed" | "skipped";

export interface HarnessChangeset {
  request_id: string;
  items: ChangesetItem[];
}

export interface HarnessJobView {
  id: string;
  sessionId: string;
  sourceRequestId: string;
  status: HarnessJobStatus;
  runtimeVerification: RuntimeVerification;
  attempts: number;
  createdAt: number;
  updatedAt: number;
  summary?: string;
  error?: string;
  memoryFiles: string[];
  changeset?: HarnessChangeset;
  dismissed: boolean;
}

/** Durable post-acceptance harness job update. */
export interface HarnessJobMessage extends SessionScopedMessage, HarnessJobView {
  type: "harness_job";
}

/** `progress {stage, detail?, pct?}` - render as a conversation entry. */
export interface ProgressMessage extends OptionalSessionScopedMessage {
  type: "progress";
  stage: ProgressStage;
  detail?: string;
  provider?: ProviderId;
  model?: string;
  /**
   * Optional percent for bootstrap progress. The Rust bootstrap emitter sends
   * this as a separate field (`progress {stage:"bootstrap", detail, pct}`),
   * matching feature 10 and `src-tauri/src/bootstrap.rs`.
   */
  pct?: number;
}

/** `error {message}` - flow returns to ready. */
export interface ErrorMessage extends SessionScopedMessage {
  type: "error";
  message: string;
}

export type BackendSessionActivity =
  | "idle"
  | "running_read"
  | "running_write"
  | "waiting_input"
  | "review"
  | "error";

/** Backend-authoritative session execution/write-queue activity. */
export interface SessionActivityMessage extends SessionScopedMessage {
  type: "session_activity";
  activity: BackendSessionActivity;
}

export type ExecutionMode = "interactive" | "autonomous";
export type IterationBoundaryReason =
  | "tool_actions"
  | "tool_rounds"
  | "context_pressure"
  | "provider_continuation";
export type AutonomousRunStatus =
  | "running"
  | "pausing"
  | "paused"
  | "paused_after_restart"
  | "waiting_input"
  | "review"
  | "safety_stopped"
  | "cancelled"
  | "failed"
  | "completed";
export type AutonomousPauseReason =
  | "user"
  | "restart"
  | "waiting_input"
  | "review"
  | "unanswered_ask"
  | "team_apply";

export interface AutonomousRunPolicy {
  maxWallTimeMillis?: number | null;
  maxObservedTokens?: number | null;
}

export interface AutonomousBuildStatus {
  inputRevision: string;
  diagnosticsFingerprint: string;
  errorCount: number;
  success: boolean;
  consecutiveNoProgress: number;
}

export interface AutonomousRunProgress {
  elapsedActiveMillis: number;
  observedTokens?: number;
  readActions: number;
  writeActions: number;
  latestBuild?: AutonomousBuildStatus;
  consecutiveNoProgress: number;
  recentFingerprints?: string[];
}

export interface AutonomousRunState {
  schemaVersion: number;
  id: string;
  status: AutonomousRunStatus;
  startedAt: number;
  updatedAt: number;
  iteration: number;
  goal: string;
  requestId: string;
  clientTurnId: string;
  projectId: string;
  projectRevision: string;
  policy: AutonomousRunPolicy;
  progress: AutonomousRunProgress;
  activeStartedAt?: number;
  lastCheckpoint?: unknown;
  boundaryReason?: IterationBoundaryReason;
  pauseReason?: AutonomousPauseReason;
  blocker?: string;
}

export interface AutonomousRunMessage
  extends SessionScopedMessage,
    AutonomousRunState {
  type: "autonomous_run";
}

/** `status {compiling, project}` - editor state for the header. */
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

/** `memory {project, files}` - project memory snapshot. */
export interface MemoryMessage {
  type: "memory";
  project: string;
  files: Record<MemoryFile, string>;
}

/** `memory_saved {file}` - acknowledgement for a saved memory file. */
export interface MemorySavedMessage {
  type: "memory_saved";
  file: MemoryFile;
}

/** Dat-edit wiki table family (the on-disk `table` field). */
export const WIKI_TABLES = ["dat", "xdat", "tbl", "req", "btn"] as const;
export type WikiTable = (typeof WIKI_TABLES)[number];

/**
 * One dat-edit wiki ledger entry — the LAST value the agent applied via the
 * DATA EDITOR (GETDAT/SETDAT) and the user APPROVED. Mirrors the Rust
 * `wiki::LedgerEntry` wire shape EXACTLY (camelCase: `objId`/`itemName`/
 * `appliedAt`/`editedByUser`). The ledger key is `"{table}:{dat}:{objId}:{property}"`.
 * `value` is a number or string (the applied new value); `itemName` is present
 * only for `table=dat` & `dat="units"` (the resolved unit name).
 */
export interface LedgerEntry {
  /** Dat family: "dat" | "xdat" | "tbl" | "req" | "btn" (open string, defensive). */
  table: string;
  /** Dat-family name (e.g. "units","weapons"); empty for tbl/btn. */
  dat: string;
  /** Numeric object index. */
  objId: number;
  /** Property name as used on the bridge wire (e.g. "HP","Armor"). */
  property: string;
  /** The last applied new value (number or string). */
  value: number | string;
  /** Unit name for table=dat & dat="units"; omitted otherwise. */
  itemName?: string;
  /** Unix seconds from the originating journal entry. */
  appliedAt: number;
  /** false when written by the accept-hook, true when corrected via wiki_save. */
  editedByUser: boolean;
}

/**
 * `wiki {version, entries}` - the dat-edit ledger snapshot. Pushed after every
 * turn that records >= 1 dat edit, and also the
 * resolved value of the `wiki_get`/`wiki_save` commands. `entries` is a
 * key -> entry map mirroring the on-disk `ledger.json`.
 */
export interface WikiMessage {
  type: "wiki";
  version: number;
  entries: Record<string, LedgerEntry>;
}

export interface RecentProject {
  name: string;
  path: string;
  lastOpenedAt: number;
  available: boolean;
}

export type HarnessImportIssueScope = "workspace" | "memory" | "sessions";

/** One optional harness item that could not be safely imported. */
export interface HarnessImportIssue {
  readonly id: string;
  readonly scope: HarnessImportIssueScope;
  readonly path: string;
  readonly reason: string;
}

export interface SetupMessage {
  type: "setup";
  projectPath: string;
  projectValid: boolean;
  /** True only when this response came from a successful explicit project open/create/import. */
  projectOpened: boolean;
  euddraftPath: string;
  euddraftValid: boolean;
  assetsReady: boolean;
  defaultProvider?: ProviderId | null;
  providers: ProviderStatus[];
  setupRequired: boolean;
  error?: string | null;
  /** Optional harness items that require explicit review before import can complete. */
  importIssues?: HarnessImportIssue[];
}



// ---- sessions (session restore feature) ------------------------------
/**
 * Saved-session list metadata (the `session_list` result rows and the
 * flattened head of a `SessionRecord`). Field names are camelCase to match the
 * Rust `SessionMeta` (`#[serde(rename_all = "camelCase")]`). `createdAt` is in
 * Unix seconds; `lastConversationAt` is in Unix milliseconds. The list is
 * ordered by the latest submitted user conversation.
 */
export interface SessionMeta {
  id: string;
  name: string;
  /** Native project name captured at session creation. */
  project: string;
  /** Missing only in legacy records; the Rust backend migrates it to `eps`. */
  kind?: "eps" | "map";
  provider: ProviderId;
  model: string;
  createdAt: number;
  lastConversationAt: number;
  /** For a team Map session: the EPS session that owns it. */
  teamParent?: string;
}

export type MentionKind = "map.region" | "map.location";

export interface MapRegionMentionV1 {
  kind: "map.region";
  version: 1;
  projectId: string;
  sourceFileSha256: string;
  mapWidth: number;
  mapHeight: number;
  selectionId: string;
  selectionSnapshotHash: string;
}

export interface MapLocationMentionV1 {
  kind: "map.location";
  version: 1;
  projectId: string;
  sourceFileSha256: string;
  locationId: number;
  locationFingerprint: string;
}

export type MentionSnapshot = MapRegionMentionV1 | MapLocationMentionV1;

export interface MentionInstance {
  id: string;
  label: string;
  detail?: string;
  mention: MentionSnapshot;
  stale?: boolean;
}

export interface MentionSearchRequest {
  query: string;
  kinds?: MentionKind[];
  limit?: number;
}

export interface MentionSuggestion {
  resourceKey: string;
  kind: MentionKind;
  label: string;
  detail?: string;
  mention: MentionSnapshot;
}

export interface MentionSearchResponse {
  schema: "eud-mention-search/1";
  results: MentionSuggestion[];
  truncated: boolean;
}

export type AttachmentKind = "image" | "text" | "audio";

/** App-owned attachment metadata returned by `attachment_stage`. */
export interface AttachmentDescriptor {
  id: string;
  name: string;
  mime: string;
  kind: AttachmentKind;
  size: number;
}

/** Attachment metadata retained in the panel log; only images may carry small preview data URLs. */
export interface ChatAttachment extends AttachmentDescriptor {
  previewUrl?: string;
}

/**
 * The panel-owned durable conversation snapshot persisted inside a session
 * record. Opaque to Rust (stored/returned verbatim); its schema is owned here.
 * `logSeq` is the closure-private monotonic id high-water mark the store must
 * advance past on hydrate so restored ids never collide with fresh ones. Each
 * `log` entry is the DURABLE {@link LogEntry} subset (id/kind/text + optional
 * stage/tools); transient turn/plan state is NOT persisted.
 */
export interface PanelLog {
  schemaVersion: number;
  logSeq: number;
  log: PanelLogEntry[];
}

/** Durable log entry persisted in {@link PanelLog} (subset of the store LogEntry). */
export interface PanelLogEntry {
  id: number;
  kind: string;
  text: string;
  /** Stable user-turn anchor; absent only on hydrated legacy logs. */
  clientTurnId?: string;
  stage?: string;
  tools?: PanelLogTool[];
  attachments?: ChatAttachment[];
  mentions?: MentionInstance[];
  /** Folded supporting text behind the row's "자세히" toggle. */
  detail?: string;
}

/** Durable archived-tool row persisted in a {@link PanelLogEntry} (subset of AgentTool). */
export interface PanelLogTool {
  id: string;
  name: string;
  /** Always terminal (done/failed) for an archived tool. */
  state: string;
  args?: string;
  detail?: string;
}

/**
 * Full saved session. The backend validates that metadata and the strict
 * provider conversation variant agree.
 */
export interface SessionRecord extends SessionMeta {
  providerBinding?: ProviderBinding;
  pendingRequestIds: string[];
  panelLog: PanelLog | null;
  contextUsage?: ContextUsage;
  autonomousRun?: AutonomousRunState;
  /** EPS → Map team tasks (EPS sessions only). */
  teamTasks?: TeamTask[];
}

/** Discriminated union of every documented core -> panel message. */
export type ServerMessage =
  | AgentEventMessage
  | ContextUsageMessage
  | AskMessage
  | TeamTaskMessage
  | AnswerMessage
  | PlanMessage
  | GitMessage
  | HarnessJobMessage
  | ProgressMessage
  | ErrorMessage
  | SessionActivityMessage
  | AutonomousRunMessage
  | StatusMessage
  | ListMessage
  | MemoryMessage
  | MemorySavedMessage
  | WikiMessage
  | SetupMessage;

/** All server message `type` discriminants (closed set). */
export const SERVER_MESSAGE_TYPES = [
  "agent_event",
  "context_usage",
  "answer",
  "plan",
  "ask",
  "team_task",
  "git",
  "harness_job",
  "progress",
  "error",
  "session_activity",
  "autonomous_run",
  "status",
  "list",
  "memory",
  "memory_saved",
  "wiki",
  "setup",
] as const;
export type ServerMessageType = (typeof SERVER_MESSAGE_TYPES)[number];

// ---- panel -> core messages -------------------------------------------
/** Session-scoped command base. Every conversation mutation names its owner. */
interface SessionCommand {
  sessionId: string;
}

/** `chat {sessionId, text, attachments}` - queue/start one session turn. */
export interface ChatMessage extends SessionCommand {
  type: "chat";
  clientTurnId: string;
  text: string;
  attachments: string[];
  mentions?: MentionInstance[];
  executionMode?: ExecutionMode;
  autonomousPolicy?: AutonomousRunPolicy;
}

/** `plan_feedback` iterates the plan owned by one active session. */
export interface PlanFeedbackMessage extends SessionCommand {
  type: "plan_feedback";
  clientTurnId: string;
  text: string;
  attachments: string[];
  mentions?: MentionInstance[];
}

/** `plan_approve` resumes the plan owned by one active session. */
export interface PlanApproveMessage extends SessionCommand {
  type: "plan_approve";
}
/** Resolve one pending ASK tool call without starting a new chat turn. */
export interface AskResponseMessage extends SessionCommand {
  type: "ask_response";
  requestId: string;
  answers: Record<string, AskAnswer>;
}


/** Interrupt the in-flight turn for one session. */
export interface CancelMessage extends SessionCommand {
  type: "cancel";
}

export interface AutonomousPauseMessage extends SessionCommand {
  type: "autonomous_pause";
}

export interface AutonomousResumeMessage extends SessionCommand {
  type: "autonomous_resume";
}

export interface AutonomousStopMessage extends SessionCommand {
  type: "autonomous_stop";
}

/** Replace one session's model-visible history with a panel-log prefix. */
export interface ConversationRewindMessage extends SessionCommand {
  type: "conversation_rewind";
  panelLog: PanelLog;
}


/** `status {}` - request editor state. */
export interface StatusRequest {
  type: "status";
}

/** `list {}` - request the project file tree. */
export interface ListRequest {
  type: "list";
}

/** `memory_get {}` - request the project memory snapshot. */
export interface MemoryGetMessage {
  type: "memory_get";
}

/** `memory_save {file, content}` - save one project memory markdown file. */
export interface MemorySaveMessage {
  type: "memory_save";
  file: MemoryFile;
  content: string;
}

/** `setup_status {}` - request the first-run manifest-check snapshot. */
export interface SetupStatusRequest {
  type: "setup_status";
}

/** Pick and configure an existing native project root. */
export interface SetupPickProjectPathMessage {
  type: "setup_pick_project_path";
  /** When true, use a folder picker; otherwise pick an .eap manifest or legacy project to migrate. */
  directory?: boolean;
}
/** Create a native project from a selected source map and empty destination. */
export interface SetupCreateProjectMessage {
  type: "setup_create_project";
}

/** Import a legacy `.e3s` into a selected empty native destination. */
export interface SetupImportE3sMessage {
  type: "setup_import_e3s";
  sourceE3s: string;
  destination: string;
  /** Issue ids explicitly reviewed and authorized for omission. */
  excludedImportItems?: readonly string[];
}
/** Pick an existing euddraft launcher (`euddraft.exe` / macOS `euddraft`) or `euddraft.py`, or its containing folder. */
export interface SetupPickEuddraftPathMessage {
  type: "setup_pick_euddraft_path";
  /** When true, open a folder picker and resolve euddraft within that folder. */
  directory?: boolean;
}

/** Install the latest managed euddraft release into the app-local directory. */
export interface SetupInstallEuddraftMessage {
  type: "setup_install_euddraft";
}

/**
 * `bootstrap_run {}` - run the first-run asset download (also the setup
 * screen's retry action). Progress streams as `progress {stage: "bootstrap"}`.
 */
export interface BootstrapRunMessage {
  type: "bootstrap_run";
}

/** Discriminated union of every documented panel -> core message. */
export type ClientMessage =
  | ChatMessage
  | PlanFeedbackMessage
  | PlanApproveMessage
  | AskResponseMessage
  | CancelMessage
  | AutonomousPauseMessage
  | AutonomousResumeMessage
  | AutonomousStopMessage
  | ConversationRewindMessage
  | StatusRequest
  | ListRequest
  | MemoryGetMessage
  | MemorySaveMessage
  | SetupStatusRequest
  | SetupPickProjectPathMessage
  | SetupCreateProjectMessage
  | SetupImportE3sMessage
  | SetupPickEuddraftPathMessage
  | SetupInstallEuddraftMessage
  | BootstrapRunMessage;

/** All client message `type` discriminants (closed set). */
export const CLIENT_MESSAGE_TYPES = [
  "chat",
  "plan_feedback",
  "plan_approve",
  "ask_response",
  "cancel",
  "autonomous_pause",
  "autonomous_resume",
  "autonomous_stop",
  "conversation_rewind",
  "status",
  "list",
  "memory_get",
  "memory_save",
  "setup_status",
  "setup_pick_project_path",
  "setup_create_project",
  "setup_import_e3s",
  "setup_pick_euddraft_path",
  "setup_install_euddraft",
  "bootstrap_run",
] as const;
export type ClientMessageType = (typeof CLIENT_MESSAGE_TYPES)[number];

// ---- runtime type guards (inbound dispatch gate) -----------------------
function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

const HARNESS_IMPORT_ISSUE_SCOPES: readonly HarnessImportIssueScope[] = [
  "workspace",
  "memory",
  "sessions",
];

function isHarnessImportIssueScope(value: unknown): value is HarnessImportIssueScope {
  return (
    typeof value === "string" &&
    HARNESS_IMPORT_ISSUE_SCOPES.includes(value as HarnessImportIssueScope)
  );
}

export function isHarnessImportIssue(value: unknown): value is HarnessImportIssue {
  return (
    isObject(value) &&
    typeof value.id === "string" &&
    value.id.trim() !== "" &&
    isHarnessImportIssueScope(value.scope) &&
    typeof value.path === "string" &&
    value.path.trim() !== "" &&
    typeof value.reason === "string" &&
    value.reason.trim() !== ""
  );
}

function isMemoryFile(value: unknown): value is MemoryFile {
  return typeof value === "string" && MEMORY_FILES.includes(value as MemoryFile);
}


function isMemoryFiles(value: unknown): value is Record<MemoryFile, string> {
  return (
    isObject(value) &&
    MEMORY_FILES.every((file) => typeof value[file] === "string")
  );
}

function hasSessionId(value: Record<string, unknown>): boolean {
  return typeof value.sessionId === "string" && value.sessionId.length > 0;
}

/** True if `value` is an `agent_event` message. */
export function isAgentEventMessage(value: unknown): value is AgentEventMessage {
  return (
    isObject(value) &&
    value.type === "agent_event" &&
    hasSessionId(value) &&
    typeof value.kind === "string" &&
    typeof value.detail === "string"
  );
}

function isTokenUsageBreakdown(value: unknown): value is TokenUsageBreakdown {
  return (
    isObject(value) &&
    typeof value.inputTokens === "number" &&
    typeof value.cachedInputTokens === "number" &&
    typeof value.cacheWriteInputTokens === "number" &&
    typeof value.outputTokens === "number" &&
    typeof value.reasoningOutputTokens === "number" &&
    typeof value.totalTokens === "number"
  );
}

function isContextUsage(value: unknown): value is ContextUsage {
  return (
    isObject(value) &&
    isTokenUsageBreakdown(value.last) &&
    isTokenUsageBreakdown(value.total) &&
    (value.modelContextWindow === null ||
      typeof value.modelContextWindow === "number")
  );
}

/** True if `value` is a typed per-session context usage update. */
export function isContextUsageMessage(
  value: unknown,
): value is ContextUsageMessage {
  return (
    isObject(value) &&
    value.type === "context_usage" &&
    hasSessionId(value) &&
    typeof value.turnId === "string" &&
    isContextUsage(value.tokenUsage)
  );
}

/** True if `value` is an `answer` message. */
export function isAnswerMessage(value: unknown): value is AnswerMessage {
  return (
    isObject(value) &&
    value.type === "answer" &&
    hasSessionId(value) &&
    typeof value.text === "string"
  );
}

/** True if `value` is a `plan` message (markdown string, numeric revision). */
export function isPlanMessage(value: unknown): value is PlanMessage {
  return (
    isObject(value) &&
    value.type === "plan" &&
    hasSessionId(value) &&
    typeof value.markdown === "string" &&
    typeof value.revision === "number"
  );
}

function isAskOption(value: unknown): value is AskOption {
  return (
    isObject(value) &&
    typeof value.label === "string" &&
    (value.description === undefined || typeof value.description === "string")
  );
}

function isAskQuestion(value: unknown): value is AskQuestion {
  return (
    isObject(value) &&
    typeof value.id === "string" &&
    value.id.length > 0 &&
    (value.header === undefined || typeof value.header === "string") &&
    typeof value.question === "string" &&
    value.question.length > 0 &&
    typeof value.multi === "boolean" &&
    (value.options === undefined ||
      (Array.isArray(value.options) && value.options.every(isAskOption)))
  );
}

/** True if `value` is a session-scoped pending ASK request. */
export function isAskMessage(value: unknown): value is AskMessage {
  return (
    isObject(value) &&
    value.type === "ask" &&
    hasSessionId(value) &&
    typeof value.requestId === "string" &&
    value.requestId.length > 0 &&
    (value.status === undefined ||
      value.status === "pending" ||
      value.status === "expired") &&
    (value.waitSeconds === undefined ||
      (typeof value.waitSeconds === "number" && value.waitSeconds >= 0)) &&
    Array.isArray(value.questions) &&
    value.questions.length > 0 &&
    value.questions.every(isAskQuestion)
  );
}

function isTeamTaskStatus(value: unknown): value is TeamTaskStatus {
  return (
    isObject(value) &&
    TEAM_TASK_STATUS_KINDS.includes(value.kind as TeamTaskStatusKind) &&
    (value.kind !== "failed" || typeof value.reason === "string")
  );
}

function isTeamCandidateSummary(value: unknown): value is TeamCandidateSummary {
  return (
    isObject(value) &&
    typeof value.revision === "number" &&
    typeof value.revisionKey === "string" &&
    typeof value.mapSha256 === "string" &&
    typeof value.summary === "string" &&
    ["terrainCells", "units", "buildings", "doodads", "sprites", "locations"].every(
      (key) => typeof value[key] === "number",
    )
  );
}

/** True if `value` is a persisted team task. */
export function isTeamTask(value: unknown): value is TeamTask {
  return (
    isObject(value) &&
    typeof value.id === "string" &&
    value.id.length > 0 &&
    typeof value.parentRequestId === "string" &&
    typeof value.mapSessionId === "string" &&
    (value.mapRequestId === undefined || typeof value.mapRequestId === "string") &&
    typeof value.goal === "string" &&
    Array.isArray(value.layers) &&
    value.layers.every((layer) => MAP_TASK_LAYERS.includes(layer as MapTaskLayer)) &&
    (value.selectionIds === undefined || isStringArray(value.selectionIds)) &&
    (value.locationIds === undefined ||
      (Array.isArray(value.locationIds) &&
        value.locationIds.every((id) => typeof id === "number"))) &&
    typeof value.sourceMapSha256AtCreate === "string" &&
    isTeamTaskStatus(value.status) &&
    (value.candidate === undefined || isTeamCandidateSummary(value.candidate)) &&
    (value.appliedSourceSha256 === undefined ||
      typeof value.appliedSourceSha256 === "string") &&
    (value.appliedBy === undefined ||
      value.appliedBy === "user" ||
      value.appliedBy === "agent") &&
    typeof value.createdAt === "number" &&
    typeof value.updatedAt === "number"
  );
}

/** True if `value` is a session-scoped `team_task` event. */
export function isTeamTaskMessage(value: unknown): value is TeamTaskMessage {
  return (
    isObject(value) &&
    value.type === "team_task" &&
    hasSessionId(value) &&
    isTeamTask(value.task) &&
    (value.continuation === undefined ||
      (isObject(value.continuation) &&
        typeof value.continuation.clientTurnId === "string" &&
        typeof value.continuation.text === "string"))
  );
}

function isCommitRecord(value: unknown): value is CommitRecord {
  return (
    isObject(value) &&
    typeof value.sha === "string" &&
    typeof value.subject === "string" &&
    typeof value.files === "number"
  );
}

/** True if `value` is a `git` message. Every field is optional core-side. */
export function isGitMessage(value: unknown): value is GitMessage {
  return (
    isObject(value) &&
    value.type === "git" &&
    hasSessionId(value) &&
    (value.external === undefined || isCommitRecord(value.external)) &&
    (value.turn === undefined || isCommitRecord(value.turn)) &&
    (value.warning === undefined || typeof value.warning === "string")
  );
}


function isStringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((item) => typeof item === "string");
}

export function isHarnessJobMessage(value: unknown): value is HarnessJobMessage {
  return (
    isObject(value) &&
    value.type === "harness_job" &&
    hasSessionId(value) &&
    typeof value.id === "string" &&
    typeof value.sourceRequestId === "string" &&
    typeof value.status === "string" &&
    typeof value.runtimeVerification === "string" &&
    typeof value.attempts === "number" &&
    typeof value.dismissed === "boolean" &&
    Array.isArray(value.memoryFiles) &&
    (value.summary === undefined || typeof value.summary === "string") &&
    (value.error === undefined || typeof value.error === "string") &&
    (value.changeset === undefined ||
      (isObject(value.changeset) &&
        typeof value.changeset.request_id === "string" &&
        Array.isArray(value.changeset.items)))
  );
}

/** True if `value` is a `progress` message (stage is an open string). */
export function isProgressMessage(value: unknown): value is ProgressMessage {
  return (
    isObject(value) &&
    value.type === "progress" &&
    typeof value.stage === "string" &&
    (value.provider === undefined || isProviderId(value.provider)) &&
    (value.model === undefined || typeof value.model === "string")
  );
}

/** True if `value` is an `error` message. */
export function isErrorMessage(value: unknown): value is ErrorMessage {
  return (
    isObject(value) &&
    value.type === "error" &&
    hasSessionId(value) &&
    typeof value.message === "string"
  );
}

export function isSessionActivityMessage(
  value: unknown,
): value is SessionActivityMessage {
  return (
    isObject(value) &&
    value.type === "session_activity" &&
    hasSessionId(value) &&
    [
      "idle",
      "running_read",
      "waiting_input",
      "running_write",
      "review",
      "error",
    ].includes(String(value.activity))
  );
}

const AUTONOMOUS_STATUSES: readonly AutonomousRunStatus[] = [
  "running",
  "pausing",
  "paused",
  "paused_after_restart",
  "waiting_input",
  "review",
  "safety_stopped",
  "cancelled",
  "failed",
  "completed",
];

export function isAutonomousRunMessage(
  value: unknown,
): value is AutonomousRunMessage {
  return (
    isObject(value) &&
    value.type === "autonomous_run" &&
    hasSessionId(value) &&
    typeof value.schemaVersion === "number" &&
    typeof value.id === "string" &&
    AUTONOMOUS_STATUSES.includes(value.status as AutonomousRunStatus) &&
    typeof value.startedAt === "number" &&
    typeof value.updatedAt === "number" &&
    typeof value.iteration === "number" &&
    typeof value.goal === "string" &&
    typeof value.requestId === "string" &&
    typeof value.clientTurnId === "string" &&
    typeof value.projectId === "string" &&
    typeof value.projectRevision === "string" &&
    isObject(value.policy) &&
    isObject(value.progress) &&
    typeof value.progress.elapsedActiveMillis === "number" &&
    typeof value.progress.readActions === "number" &&
    typeof value.progress.writeActions === "number" &&
    typeof value.progress.consecutiveNoProgress === "number"
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

/** True if `value` is a `memory` message. */
export function isMemoryMessage(value: unknown): value is MemoryMessage {
  return (
    isObject(value) &&
    value.type === "memory" &&
    typeof value.project === "string" &&
    isMemoryFiles(value.files)
  );
}

/** True if `value` is a `memory_saved` message. */
export function isMemorySavedMessage(
  value: unknown,
): value is MemorySavedMessage {
  return (
    isObject(value) &&
    value.type === "memory_saved" &&
    isMemoryFile(value.file)
  );
}

function isLedgerEntry(value: unknown): value is LedgerEntry {
  return (
    isObject(value) &&
    typeof value.table === "string" &&
    typeof value.dat === "string" &&
    typeof value.objId === "number" &&
    typeof value.property === "string" &&
    (typeof value.value === "number" || typeof value.value === "string") &&
    (value.itemName === undefined || typeof value.itemName === "string") &&
    typeof value.appliedAt === "number" &&
    typeof value.editedByUser === "boolean"
  );
}

/** True if `value` is a `wiki` message (version + entry map of ledger entries). */
export function isWikiMessage(value: unknown): value is WikiMessage {
  return (
    isObject(value) &&
    value.type === "wiki" &&
    typeof value.version === "number" &&
    isObject(value.entries) &&
    Object.values(value.entries).every(isLedgerEntry)
  );
}

/** True if `value` is a `setup` message. */
export function isSetupMessage(value: unknown): value is SetupMessage {
  return (
    isObject(value) &&
    value.type === "setup" &&
    typeof value.projectPath === "string" &&
    typeof value.projectValid === "boolean" &&
    typeof value.projectOpened === "boolean" &&
    typeof value.euddraftPath === "string" &&
    typeof value.euddraftValid === "boolean" &&
    typeof value.assetsReady === "boolean" &&
    (value.defaultProvider === undefined ||
      value.defaultProvider === null ||
      isProviderId(value.defaultProvider)) &&
    Array.isArray(value.providers) &&
    value.providers.length === PROVIDER_IDS.length &&
    value.providers.every(isProviderStatus) &&
    new Set(
      (value.providers as ProviderStatus[]).map((status) => status.provider),
    ).size === PROVIDER_IDS.length &&
    PROVIDER_IDS.every((provider) =>
      (value.providers as ProviderStatus[]).some(
        (status) => status.provider === provider,
      ),
    ) &&
    typeof value.setupRequired === "boolean" &&
    (value.error === undefined ||
      value.error === null ||
      typeof value.error === "string") &&
    (value.importIssues === undefined ||
      (Array.isArray(value.importIssues) &&
        value.importIssues.every(isHarnessImportIssue)))
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
    isContextUsageMessage(value) ||
    isAnswerMessage(value) ||
    isPlanMessage(value) ||
    isAskMessage(value) ||
    isTeamTaskMessage(value) ||
    isGitMessage(value) ||
    isHarnessJobMessage(value) ||
    isProgressMessage(value) ||
    isErrorMessage(value) ||
    isSessionActivityMessage(value) ||
    isAutonomousRunMessage(value) ||
    isStatusMessage(value) ||
    isListMessage(value) ||
    isMemoryMessage(value) ||
    isMemorySavedMessage(value) ||
    isWikiMessage(value) ||
    isSetupMessage(value)
  );
}
