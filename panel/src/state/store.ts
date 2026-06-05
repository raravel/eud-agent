/**
 * Panel state machine + store v2 (framework-agnostic, plain TS).
 *
 * features/06_changeset-review-panel.md ## State machine (the spec mermaid):
 *
 *   [*] --> connecting
 *   connecting --> ready          : WS open
 *   connecting --> retry          : 2s backoff
 *   retry      --> connecting
 *   ready      --> thinking       : chat sent
 *   thinking   --> ready          : answer (no edits)
 *   thinking   --> plan_review    : plan{}
 *   plan_review --> thinking      : plan_feedback / plan_approve
 *   thinking   --> changeset_review : changeset{}
 *   changeset_review --> ready    : decisions done (accept/reject applied)
 *   changeset_review --> thinking : follow-up chat (undecided auto-accept server-side)
 *
 * features/06 ## Behaviors (encoded here):
 *   - Reconnect during thinking/plan_review resets to ready WITH a notice — the
 *     server cancels the thread turn. The LAST changeset stays reviewable across
 *     a reconnect (the journal is server-persisted), so a reconnect must NOT drop
 *     `changeset`; it only resets an in-flight TURN.
 *   - Send gating v2: `connected && hasProject && !busy` — the settable-target
 *     requirement is GONE (the agent creates files itself). No `canSendSet` /
 *     `canSendNewEps`. `busy` = a turn is in flight (thinking) OR a plan awaits a
 *     decision (plan_review accepts only feedback/approve, not chat) OR the editor
 *     is compiling.
 *   - Plan revision replacement: a later `plan{revision+1}` replaces the active
 *     plan card.
 *   - `rollback_result` flips per-item decision state (rejected / failed).
 *   - Event log capped at 500 — drop oldest.
 *
 * Subscribe/listener pattern — no runtime dependency, no framework coupling.
 */

import type {
  ChangesetItem,
  FileEntry,
  ProgressStage,
} from "@/ws/protocol";

/** Max conversation/event-log entries (features/06 ## Behaviors). */
export const MAX_LOG_ENTRIES = 500;

/** Phases of the v2 panel state machine. */
export type Phase =
  | "connecting"
  | "retry"
  | "ready"
  | "thinking"
  | "plan_review"
  | "changeset_review";

/** Kind of a log line (drives styling in the UI layer). */
export type LogKind =
  | "info"
  | "you"
  | "agent"
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

/** Active plan card (from a `plan` event); replaced by a higher revision. */
export interface PlanState {
  markdown: string;
  revision: number;
}

/** Per-item decision outcome (driven by `rollback_result` + the recorded send). */
export type ItemDecision = "accepted" | "rejected" | "failed";

/**
 * The changeset_decision the store last SENT, recorded so the matching inbound
 * `rollback_result` can be labelled correctly.
 *
 * The server's `rollback_result{ids, ok}` carries NO accept/reject discriminator:
 * engine.py routes BOTH accept and reject through it (accept → `ids:[]`,`ok:true`;
 * reject → the real journal ids,`ok:true|false`). So the inbound message alone
 * cannot tell a KEPT item from a 되돌림 one, and accept-all sends an EMPTY ids
 * array. The store records what it sent (it is the SOLE decision sender, the WS is
 * ordered, and exactly one decision is in flight at a time) and reconciles on the
 * reply. `ids:"all"` is kept verbatim so an empty inbound ids array on accept-all
 * still resolves to "apply to all undecided items".
 *
 * Candidate server-side amendment (later task): have `rollback_result` echo the
 * decision (or the accepted ids) so the panel need not infer it. Until then the
 * recorded-decision approach is correct precisely because the store is the only
 * sender.
 */
export interface PendingDecision {
  decision: "accept" | "reject";
  ids: "all" | string[];
}

/**
 * The active changeset under review. Persisted in the store ACROSS a reconnect
 * (the journal is server-persisted; features/06 line 52) — only an in-flight TURN
 * resets on reconnect, never the changeset. `decisions` maps item id → outcome;
 * an absent id is still undecided.
 */
export interface ChangesetState {
  request_id: string;
  items: ChangesetItem[];
  decisions: Record<string, ItemDecision>;
}

/** Immutable snapshot the UI renders from. */
export interface PanelState {
  phase: Phase;
  /** Project open (LIST returned files, even if zero). */
  hasProject: boolean;
  files: FileEntry[];
  /** Editor project name for the header. */
  project: string;
  /** Editor build-in-progress flag, from the `status` event. */
  compiling: boolean;
  /** Active plan card (null until a `plan` event; replaced by higher revision). */
  plan: PlanState | null;
  /** Active changeset under review (null until a `changeset`; survives reconnect). */
  changeset: ChangesetState | null;
  /**
   * The changeset_decision in flight (recorded on send, cleared when its
   * `rollback_result` lands). Exposed so the UI can label the result per the
   * accept/reject the user chose — the inbound reply carries no discriminator.
   */
  pendingDecision: PendingDecision | null;
  /** Capped event log (oldest dropped at {@link MAX_LOG_ENTRIES}). */
  log: LogEntry[];
  // ---- derived selectors (computed on every mutation) ----
  /** Whether the connection is currently open. */
  connected: boolean;
  /** Send gating v2: connected && hasProject && !busy (no settable-target gate). */
  canSend: boolean;
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
  /** WS opened: ready; reset any in-flight turn with a notice (reconnect). */
  wsOpen(): void;
  /** WS errored: enter retry. */
  wsError(): void;

  // ---- inbound server events ----
  applyStatus(msg: { compiling: boolean; project: string }): void;
  applyList(msg: { files?: FileEntry[]; error?: string }): void;
  /** A streamed `agent_event` — logged as an agent activity line. */
  agentEvent(kind: string, detail: string): void;
  /** `answer` — answer-only turn; back to ready. */
  answerReceived(text: string): void;
  /** `plan` — enter/refresh plan_review (revision replaces the active card). */
  planReceived(markdown: string, revision: number): void;
  /** `changeset` — enter changeset_review with the journaled items. */
  changesetReceived(requestId: string, items: ChangesetItem[]): void;
  /** `rollback_result` — flip per-item decision state (rejected/failed). */
  rollbackResult(ids: string[], ok: boolean): void;
  /** `error` — return the flow to ready (and detect the no-project signal). */
  errorReceived(message: string): void;
  /** `progress` — logged only; no phase change in v2. */
  progressReceived(stage: ProgressStage): void;

  // ---- user intents (UI drives these after a successful send) ----
  /** A chat was sent → thinking (from ready or changeset_review). */
  chatSent(): void;
  /** plan_feedback was sent → thinking. */
  planFeedbackSent(): void;
  /** plan_approve was sent → thinking. */
  planApproveSent(): void;
  /**
   * A changeset_decision was sent (per-item or bulk) — RECORD it so the matching
   * `rollback_result` can be labelled per the recorded accept/reject (the inbound
   * message carries no discriminator). Awaits `rollback_result`.
   */
  decisionSent(decision: "accept" | "reject", ids: "all" | string[]): void;
  /** cancel was sent — return to ready. */
  cancelSent(): void;

  // ---- logging ----
  log(kind: LogKind, text: string, stage?: ProgressStage): void;
}

/**
 * Phases in which sending a chat is blocked because work is in flight or the
 * panel awaits a plan decision. `changeset_review` is NOT busy — a follow-up
 * chat is allowed (the server auto-accepts undecided items). `compiling` is an
 * orthogonal busy signal layered on top in {@link PanelState.canSend}.
 */
const BUSY_PHASES: ReadonlySet<Phase> = new Set<Phase>([
  "thinking",
  "plan_review",
]);

/**
 * Contractual no-project marker. The bridge returns `ERROR: no project` when no
 * project is loaded; the server relays it as `error {message}` (there is NO
 * `list {error}` path). Matched as a case-insensitive substring (kept lowercase).
 */
const NO_PROJECT_MARKER = "no project";

/** Notice shown when a reconnect cancels an in-flight turn (features/06 line 52). */
const RECONNECT_TURN_NOTICE = "재연결로 진행 중이던 작업이 취소되었습니다.";

/** True when every changeset item has a decision (accepted/rejected/failed). */
function isChangesetFullyDecided(cs: ChangesetState): boolean {
  return (
    cs.items.length > 0 &&
    cs.items.every((it) => cs.decisions[it.id] !== undefined)
  );
}

/** Create a fresh panel store. */
export function createPanelStore(): PanelStore {
  let logSeq = 0;

  // ---- mutable core (selectors are recomputed into the snapshot) ----
  const core = {
    phase: "connecting" as Phase,
    hasProject: false,
    files: [] as FileEntry[],
    project: "",
    compiling: false,
    plan: null as PlanState | null,
    changeset: null as ChangesetState | null,
    log: [] as LogEntry[],
    connected: false,
    // A turn is in flight (chat/plan_feedback/plan_approve sent, no turn-end event
    // yet). Tracked SEPARATELY from `phase` because a reconnect drives
    // wsConnecting() -> wsOpen(): by the time wsOpen runs, `phase` has already left
    // thinking/plan_review for connecting. The server cancels the thread turn on
    // disconnect, so wsOpen uses this flag (not the transient phase) to decide
    // whether to emit the reconnect notice (features/06 line 52).
    turnInFlight: false,
    // The changeset_decision last sent, awaiting its rollback_result. The inbound
    // reply carries no accept/reject discriminator, so this is how the store knows
    // whether to label items 적용 유지 (accepted) or 되돌림 (rejected). Cleared on
    // apply. null when no decision is in flight.
    pendingDecision: null as PendingDecision | null,
  };

  let snapshot: PanelState = computeSnapshot();
  const listeners = new Set<Listener>();

  function computeSnapshot(): PanelState {
    const busy = BUSY_PHASES.has(core.phase) || core.compiling;
    const canSend = core.connected && core.hasProject && !busy;
    return {
      phase: core.phase,
      hasProject: core.hasProject,
      files: core.files,
      project: core.project,
      compiling: core.compiling,
      plan: core.plan,
      changeset: core.changeset,
      pendingDecision: core.pendingDecision,
      log: core.log,
      connected: core.connected,
      canSend,
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
    // Drop oldest beyond the cap (features/06 ## Behaviors).
    const next =
      core.log.length >= MAX_LOG_ENTRIES ? core.log.slice(1) : core.log.slice();
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
      // Reconnect cancels any in-flight TURN (the server cancels the thread on
      // disconnect): a reconnect mid thinking/plan_review resets the turn WITH a
      // notice, dropping the in-flight plan card. `turnInFlight` survives the
      // intervening wsConnecting() so the notice fires even though `phase` already
      // moved to connecting.
      if (core.turnInFlight) {
        core.turnInFlight = false;
        core.plan = null;
        pushLog("warn", RECONNECT_TURN_NOTICE);
      }
      // The last changeset STAYS reviewable across a reconnect (journal is
      // server-persisted; features/06 line 52): if an undecided changeset is
      // present, restore changeset_review even though the intermediate
      // connecting/retry phases passed through. Otherwise land on ready.
      if (core.changeset !== null && !isChangesetFullyDecided(core.changeset)) {
        core.phase = "changeset_review";
      } else {
        core.phase = "ready";
      }
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
      core.compiling = msg.compiling ?? false;
      emit();
    },

    applyList(msg) {
      if (msg.error !== undefined || msg.files === undefined) {
        // No project open (or list error): clear, gate send off.
        core.hasProject = false;
        core.files = [];
      } else {
        core.hasProject = true;
        core.files = msg.files;
      }
      emit();
    },

    agentEvent(kind, detail) {
      // A streamed activity line under the latest user message (features/06).
      const text = detail ? `${kind}: ${detail}` : kind;
      pushLog("agent", text);
      emit();
    },

    answerReceived(_text) {
      // answer-only turn (no edits): thinking --> ready. (The text is logged by
      // the App layer so the bubble carries the right styling.)
      core.turnInFlight = false;
      core.phase = "ready";
      emit();
    },

    planReceived(markdown, revision) {
      // propose_plan ENDS the codex turn (the turn is no longer in flight); the
      // panel now awaits feedback/approve. thinking --> plan_review; a higher
      // revision REPLACES the active card.
      core.turnInFlight = false;
      core.plan = { markdown, revision };
      core.phase = "plan_review";
      emit();
    },

    changesetReceived(requestId, items) {
      // thinking --> changeset_review. Fresh decisions map (no item decided yet).
      core.turnInFlight = false;
      core.changeset = { request_id: requestId, items, decisions: {} };
      core.phase = "changeset_review";
      emit();
    },

    rollbackResult(ids, ok) {
      // Label items per the RECORDED decision — the inbound rollback_result has no
      // accept/reject discriminator (engine.py routes BOTH through it). accept →
      // "accepted" (적용 유지); reject → ok ? "rejected" (되돌림) : "failed".
      if (core.changeset === null) {
        core.pendingDecision = null;
        emit();
        return;
      }
      const pending = core.pendingDecision;
      // Which items did this reply decide?
      //  - bulk accept echoes an EMPTY ids array (the server does not return the
      //    accepted ids), so resolve it against ALL currently-undecided items.
      //  - reject (and per-item accept) carry the real ids.
      let targetIds: string[];
      if (pending?.ids === "all" && ids.length === 0) {
        targetIds = core.changeset.items
          .filter((it) => core.changeset!.decisions[it.id] === undefined)
          .map((it) => it.id);
      } else {
        targetIds = ids;
      }
      const outcome: ItemDecision = pending
        ? pending.decision === "accept"
          ? "accepted"
          : ok
            ? "rejected"
            : "failed"
        : // Defensive fallback: a rollback_result with no recorded decision (the
          // store is normally the sole sender, so this should not happen). Treat
          // it as the legacy reject-shaped reply rather than dropping the update.
          ok
          ? "rejected"
          : "failed";
      const decisions = { ...core.changeset.decisions };
      for (const id of targetIds) {
        decisions[id] = outcome;
      }
      core.changeset = { ...core.changeset, decisions };
      core.pendingDecision = null;
      // When every item is decided, the review is done → ready (features/06:
      // changeset_review --> ready when decisions are applied). Failed items keep
      // the panel open so the user can retry.
      const anyFailed = Object.values(core.changeset.decisions).some(
        (d) => d === "failed",
      );
      if (
        isChangesetFullyDecided(core.changeset) &&
        !anyFailed &&
        core.phase === "changeset_review"
      ) {
        core.phase = "ready";
      }
      emit();
    },

    errorReceived(message) {
      // A turn error returns the flow to ready (thinking/plan_review --> ready).
      // changeset_review keeps its reviewable changeset (an error there is about a
      // failed decision, surfaced via rollback_result/log, not a phase reset).
      if (core.phase !== "changeset_review") {
        core.turnInFlight = false;
        core.phase = "ready";
        core.plan = null;
      }
      // No-project signal: the server has NO list{error} path — the bridge's
      // "ERROR: no project" surfaces as an error{message}. Treat the contractual
      // literal as the project-closed signal so the placeholder + send gating
      // engage. Case-insensitive substring.
      if (
        typeof message === "string" &&
        message.toLowerCase().includes(NO_PROJECT_MARKER)
      ) {
        core.hasProject = false;
        core.files = [];
      }
      emit();
    },

    progressReceived(_stage) {
      // v2 has no progress-driven phase (no waiting/applying). Progress is logged
      // by the App layer via the labeller; this hook is kept for symmetry + future
      // use and intentionally does not change phase.
      emit();
    },

    // ---- user intents ----
    chatSent() {
      // ready --> thinking, and changeset_review --> thinking (follow-up chat;
      // the server auto-accepts undecided items). Starting a new turn clears the
      // prior plan card; the changeset is left intact (server archives it).
      core.turnInFlight = true;
      core.plan = null;
      core.phase = "thinking";
      emit();
    },

    planFeedbackSent() {
      // plan_review --> thinking (iterate; next plan{revision+1} replaces card).
      core.turnInFlight = true;
      core.phase = "thinking";
      emit();
    },

    planApproveSent() {
      // plan_review --> thinking (apply the approved plan).
      core.turnInFlight = true;
      core.phase = "thinking";
      emit();
    },

    decisionSent(decision, ids) {
      // Record the decision so the matching rollback_result can be labelled
      // correctly (the inbound reply carries no accept/reject discriminator, and
      // accept-all echoes an empty ids array). The phase change waits on
      // rollback_result (so a slow bridge does not strand the UI mid-review).
      core.pendingDecision = { decision, ids };
      emit();
    },

    cancelSent() {
      // cancel interrupts the in-flight turn; the journal entries persist, but no
      // changeset has been emitted yet (the turn was cut short) → back to ready.
      core.turnInFlight = false;
      if (core.phase !== "changeset_review") {
        core.phase = "ready";
        core.plan = null;
      }
      emit();
    },

    // ---- logging ----
    log(kind, text, stage) {
      pushLog(kind, text, stage);
      emit();
    },
  };
}
