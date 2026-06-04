/**
 * Panel state machine + store (framework-agnostic, plain TS).
 *
 * features/03_agent-panel.md ## Flow / state machine (the spec mermaid):
 *
 *   [*] --> connecting
 *   connecting --> ready    : WS open (token ok)
 *   connecting --> retry    : WS error (reconnect 2s backoff)
 *   retry      --> connecting
 *   ready      --> working  : send instruct
 *   working    --> reviewing: code event
 *   working    --> ready    : error event
 *   reviewing  --> applying : Apply click
 *   applying   --> ready    : applied event
 *   applying   --> waiting  : progress waiting_build
 *   waiting    --> ready    : applied or error
 *   reviewing  --> working  : re-instruct (refine)
 *
 * features/03 ## Behaviors (the advisory fixes this store encodes):
 *   - Event log capped at 500 entries — drop oldest.
 *   - Empty-but-open project disables SET Send (no valid target); new-file mode
 *     stays available.
 *   - Panel reconnect during applying/waiting resets to ready (lost `applied`
 *     confirmation is acceptable; no stuck state).
 *   - NEWEPS filename validation: non-empty after trim; no `/` or `\`.
 *
 * Subscribe/listener pattern — no runtime dependency, no framework coupling.
 */

import type {
  Diagnostic,
  FileEntry,
  ProgressStage,
} from "@/ws/protocol";

/** Max conversation/event-log entries (features/03 ## Behaviors → Event log). */
export const MAX_LOG_ENTRIES = 500;

/** Phases of the panel state machine. */
export type Phase =
  | "connecting"
  | "retry"
  | "ready"
  | "working"
  | "reviewing"
  | "applying"
  | "waiting";

/** Kind of a log line (drives styling in the UI layer). */
export type LogKind =
  | "info"
  | "you"
  | "progress"
  | "ok"
  | "warn"
  | "error";

/** One event-log entry. */
export interface LogEntry {
  /** Monotonic id for keyed rendering. */
  id: number;
  kind: LogKind;
  text: string;
  /** Progress stage if this line is a live progress entry (spinner target). */
  stage?: ProgressStage;
}

/** The latest review payload (from a `code` event). */
export interface ReviewState {
  code: string;
  lang: string;
  diff: string;
  diagnostics: Diagnostic[];
}

/** Immutable snapshot the UI renders from. */
export interface PanelState {
  phase: Phase;
  /** Project open (LIST returned files, even if zero). */
  hasProject: boolean;
  files: FileEntry[];
  /** Currently selected SET target path (empty if none). */
  selectedTarget: string;
  /** New-file (NEWEPS) mode toggle. */
  newFileMode: boolean;
  /** Editor project name for the header. */
  project: string;
  /**
   * Editor build-in-progress flag, from the `status` event. Surfaced so the UI
   * can show "editor compiling" (documented status field; architecture.md
   * WebSocket protocol → `status {compiling, project}`).
   */
  compiling: boolean;
  /** Latest review payload (null until a `code` event). */
  review: ReviewState | null;
  /** Capped event log (oldest dropped at {@link MAX_LOG_ENTRIES}). */
  log: LogEntry[];
  // ---- derived selectors (computed on every mutation) ----
  /** Whether the connection is currently open. */
  connected: boolean;
  /** Whether a SET instruct/apply can be sent right now. */
  canSendSet: boolean;
  /** Whether a NEWEPS instruct/apply can be sent (filename still validated separately). */
  canSendNewEps: boolean;
}

/** Result of {@link validateNewEpsName}. */
export type NewEpsValidation =
  | { ok: true; name: string }
  | { ok: false; reason: string };

/**
 * NEWEPS filename validation (features/03 ## Behaviors → Apply):
 * non-empty after trim; no `/` or `\`.
 */
export function validateNewEpsName(name: string): NewEpsValidation {
  const trimmed = (name ?? "").trim();
  if (trimmed.length === 0) {
    return { ok: false, reason: "파일 이름을 입력하세요." };
  }
  if (trimmed.includes("/") || trimmed.includes("\\")) {
    return {
      ok: false,
      reason: "파일 이름에 경로 구분자(/ 또는 \\)를 쓸 수 없습니다.",
    };
  }
  return { ok: true, name: trimmed };
}

/** Listener invoked after every state change. */
export type Listener = (state: PanelState) => void;

/** The store surface (actions + read/subscribe). */
export interface PanelStore {
  getState(): PanelState;
  subscribe(listener: Listener): () => void;

  // ---- connection lifecycle (WS client drives these) ----
  /** WS started connecting (initial or after a retry tick). */
  wsConnecting(): void;
  /** WS opened: ready, and reset any in-flight applying/waiting (reconnect). */
  wsOpen(): void;
  /** WS errored: enter retry. */
  wsError(): void;

  // ---- inbound server events ----
  applyStatus(msg: { compiling: boolean; project: string }): void;
  applyList(msg: { files?: FileEntry[]; error?: string }): void;
  codeReceived(review: ReviewState): void;
  appliedReceived(target: string): void;
  errorReceived(message: string): void;
  progressReceived(stage: ProgressStage): void;

  // ---- user intents (UI drives these after a successful send) ----
  /** An instruct was sent → working (also from reviewing as a refine). */
  instructSent(): void;
  /** An apply was sent → applying. */
  applySent(): void;
  /** Return to ready (Cancel from reviewing). */
  cancelReview(): void;

  // ---- target / mode selection ----
  selectTarget(path: string): void;
  setNewFileMode(on: boolean): void;

  // ---- logging ----
  log(kind: LogKind, text: string, stage?: ProgressStage): void;
}

/** Phases in which Send/Apply are blocked because work is in flight. */
const BUSY_PHASES: ReadonlySet<Phase> = new Set<Phase>([
  "working",
  "applying",
  "waiting",
]);

/**
 * Contractual no-project marker. The bridge returns `ERROR: no project` when no
 * project is loaded (features/01_lua-bridge.md ## New command: LIST); the server
 * relays it as `error {message}` (there is NO `list {error}` path). Matched as a
 * case-insensitive substring of the error message. Kept lowercase for the
 * comparison.
 */
const NO_PROJECT_MARKER = "no project";

/** Create a fresh panel store. */
export function createPanelStore(): PanelStore {
  let logSeq = 0;

  // ---- mutable core (selectors are recomputed into the snapshot) ----
  const core = {
    phase: "connecting" as Phase,
    hasProject: false,
    files: [] as FileEntry[],
    selectedTarget: "",
    newFileMode: false,
    project: "",
    compiling: false,
    review: null as ReviewState | null,
    log: [] as LogEntry[],
    connected: false,
  };

  let snapshot: PanelState = computeSnapshot();
  const listeners = new Set<Listener>();

  function computeSnapshot(): PanelState {
    const busy = BUSY_PHASES.has(core.phase);
    // A SET target is valid when the selected path maps to a settable file.
    const selected = core.files.find((f) => f.path === core.selectedTarget);
    const hasSettableTarget = selected !== undefined && selected.settable;
    // Empty-but-open project: hasProject true but no settable target → SET off
    // (vanilla advisory fix). New-file mode only needs a connected open project.
    const canSendSet =
      core.connected && core.hasProject && hasSettableTarget && !busy;
    const canSendNewEps = core.connected && core.hasProject && !busy;
    return {
      phase: core.phase,
      hasProject: core.hasProject,
      files: core.files,
      selectedTarget: core.selectedTarget,
      newFileMode: core.newFileMode,
      project: core.project,
      compiling: core.compiling,
      review: core.review,
      log: core.log,
      connected: core.connected,
      canSendSet,
      canSendNewEps,
    };
  }

  function emit(): void {
    snapshot = computeSnapshot();
    for (const listener of listeners) listener(snapshot);
  }

  function pushLog(kind: LogKind, text: string, stage?: ProgressStage): void {
    logSeq += 1;
    const entry: LogEntry = stage
      ? { id: logSeq, kind, text, stage }
      : { id: logSeq, kind, text };
    // Drop oldest beyond the cap (features/03 ## Behaviors → Event log).
    const next = core.log.length >= MAX_LOG_ENTRIES ? core.log.slice(1) : core.log.slice();
    next.push(entry);
    core.log = next;
  }

  return {
    getState() {
      return snapshot;
    },

    subscribe(listener) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },

    // ---- connection lifecycle ----
    wsConnecting() {
      core.phase = "connecting";
      core.connected = false;
      emit();
    },

    wsOpen() {
      core.connected = true;
      // Reconnect-during-applying/waiting resets to ready (no stuck state); any
      // other phase also lands on ready on a fresh open.
      core.phase = "ready";
      emit();
    },

    wsError() {
      core.phase = "retry";
      core.connected = false;
      emit();
    },

    // ---- inbound server events ----
    applyStatus(msg) {
      core.project = msg.project ?? "";
      // Documented status field: editor build-in-progress (architecture.md
      // WebSocket protocol → status {compiling, project}).
      core.compiling = msg.compiling ?? false;
      emit();
    },

    applyList(msg) {
      if (msg.error !== undefined || msg.files === undefined) {
        // No project open (or list error): clear, gate SET off.
        core.hasProject = false;
        core.files = [];
        core.selectedTarget = "";
      } else {
        core.hasProject = true;
        core.files = msg.files;
        // Keep the current selection only if it still exists; otherwise clear.
        if (!core.files.some((f) => f.path === core.selectedTarget)) {
          core.selectedTarget = "";
        }
      }
      emit();
    },

    codeReceived(review) {
      core.review = review;
      // working --> reviewing (a code event always lands in reviewing).
      core.phase = "reviewing";
      emit();
    },

    appliedReceived(_target) {
      // applying/waiting --> ready.
      core.phase = "ready";
      emit();
    },

    errorReceived(message) {
      // working/applying/waiting --> ready (error returns the flow to ready).
      core.phase = "ready";
      // No-project signal: the server has NO list{error} path — the bridge's
      // "ERROR: no project" surfaces as an error{message}. Treat the contractual
      // literal (features/01 LIST: "Project not loaded returns ERROR: no
      // project") as the project-closed signal so the "프로젝트를 열어주세요"
      // placeholder + SET-Send gating engage. Case-insensitive substring: the
      // literal is contractual.
      if (typeof message === "string" && message.toLowerCase().includes(NO_PROJECT_MARKER)) {
        core.hasProject = false;
        core.files = [];
        core.selectedTarget = "";
      }
      emit();
    },

    progressReceived(stage) {
      // Only the waiting_build stage changes phase (applying --> waiting).
      if (stage === "waiting_build" && core.phase === "applying") {
        core.phase = "waiting";
        emit();
      }
    },

    // ---- user intents ----
    instructSent() {
      // ready --> working, and reviewing --> working (refine).
      core.phase = "working";
      emit();
    },

    applySent() {
      // reviewing --> applying.
      core.phase = "applying";
      emit();
    },

    cancelReview() {
      core.phase = "ready";
      emit();
    },

    // ---- target / mode ----
    selectTarget(path) {
      core.selectedTarget = path;
      emit();
    },

    setNewFileMode(on) {
      core.newFileMode = on;
      emit();
    },

    // ---- logging ----
    log(kind, text, stage) {
      pushLog(kind, text, stage);
      emit();
    },
  };
}
