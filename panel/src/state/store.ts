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
// itemIds maps an item to its decision-target ids (a dat group has NO
// item-level id — its ids live on each property; see lib/changeset). The store
// MUST use the same id-shape helper as ChangesetView so "fully decided" agrees
// with what the view renders. Value import; the reverse `import type
// { ItemDecision }` in lib/changeset is type-only, so there is no runtime cycle.
import { itemIds } from "@/lib/changeset";

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
  /**
   * Archived tool rows (EUD-069): when a turn ends, its tool rows move from the
   * live per-turn buffer into a compact log entry carrying the rows, so past
   * tool activity stays expandable in the conversation history instead of
   * occupying the live surface into the next phase.
   */
  tools?: AgentTool[];
}

/** Active plan card (from a `plan` event); replaced by a higher revision. */
export interface PlanState {
  markdown: string;
  revision: number;
}

/**
 * One tool-call row in the current turn (EUD-065). `tool_call` opens a `running`
 * row by name; the next `tool_result` flips the latest still-running row to
 * `done` (or `failed` when the server-reported status is not "completed",
 * EUD-068). `args` is the call's argument text (server-truncated JSON) and
 * `detail` the result text — both ride `agent_event.data` and render inside the
 * Tool card. The id is monotonic per turn (for stable React keys).
 */
export interface AgentTool {
  id: string;
  name: string;
  state: "running" | "done" | "failed";
  /** Tool-call argument text (agent_event.data.args, EUD-068). */
  args?: string;
  /** Tool-result text (agent_event.data.result, EUD-068). */
  detail?: string;
}

/** Optional payload on a streamed agent_event (EUD-068 tool args/result). */
export interface AgentEventData {
  args?: string;
  result?: string;
  status?: string;
}

/**
 * Per-turn streaming buffers (EUD-065 / features/06 ## Behaviors → Agent stream).
 * The EUD-063 streamed `agent_event`s accumulate here so the AI-Elements surfaces
 * render live and reset per turn:
 *   - `reasoning` deltas → {@link TurnState.reasoning} (dim/collapsible Reasoning);
 *   - `delta` answer deltas → {@link TurnState.answer} (prominent Streamdown
 *     Message; `answerStarted` flips true on the first delta so the Reasoning
 *     block collapses when the answer begins);
 *   - `tool_call`/`tool_result` → {@link TurnState.tools} (Tool rows by name).
 * Raw internal kinds NEVER reach the log (no-raw-kind-leak contract).
 */
export interface TurnState {
  reasoning: string;
  answer: string;
  answerStarted: boolean;
  tools: AgentTool[];
}

/** A fresh (empty) per-turn buffer. */
function emptyTurn(): TurnState {
  return { reasoning: "", answer: "", answerStarted: false, tools: [] };
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
  /** Per-turn streaming buffers (reasoning / answer / tools); reset per turn. */
  turn: TurnState;
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
  /**
   * A streamed `agent_event` — accumulated into the per-turn {@link TurnState}
   * buffers (reasoning / answer / tools). Raw internal kind identifiers NEVER
   * reach the log (no-raw-kind-leak contract, features/06 / decision 06).
   * `data` is the optional EUD-068 payload (tool args / result / status).
   */
  agentEvent(kind: string, detail: string, data?: AgentEventData): void;
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
  /**
   * `reset{}` was sent ([새 대화]) — clear the client log, plan, changeset, and
   * per-turn buffers, returning to ready (EUD-064/065). The server drops the
   * retained codex thread; the next chat starts a fresh conversation.
   */
  resetSent(): void;

  // ---- logging ----
  log(kind: LogKind, text: string, stage?: ProgressStage): void;
}

/**
 * Phases in which sending is blocked because a turn is in flight.
 * `plan_review` is NOT busy (EUD-074): the MAIN prompt input is the plan
 * feedback channel — typing there sends `plan_feedback{}` (App routes by
 * phase; the PlanView feedback textarea is removed). `changeset_review` is NOT
 * busy — a follow-up chat is allowed (the server auto-accepts undecided
 * items). `compiling` is an orthogonal busy signal layered on top in
 * {@link PanelState.canSend}.
 */
const BUSY_PHASES: ReadonlySet<Phase> = new Set<Phase>(["thinking"]);

/**
 * Contractual no-project marker. The bridge returns `ERROR: no project` when no
 * project is loaded; the server relays it as `error {message}` (there is NO
 * `list {error}` path). Matched as a case-insensitive substring (kept lowercase).
 */
const NO_PROJECT_MARKER = "no project";

/** Notice shown when a reconnect cancels an in-flight turn (features/06 line 52). */
const RECONNECT_TURN_NOTICE = "재연결로 진행 중이던 작업이 취소되었습니다.";

/**
 * True when every changeset item has a decision (accepted/rejected/failed).
 *
 * An item is decided when ALL of its decision-target ids are decided — derived
 * from {@link itemIds}, the SAME id-shape helper ChangesetView uses. This is
 * load-bearing for dat groups: a dat group carries NO item-level `id` (the ids
 * live on each property), so the old `decisions[it.id]` test was permanently
 * undefined for any dat group and a changeset containing one could NEVER reach
 * "fully decided" — stranding changeset_review and re-opening it on reconnect.
 * An item with zero ids (defensive) is treated as already decided so it never
 * blocks completion.
 */
function isChangesetFullyDecided(cs: ChangesetState): boolean {
  return (
    cs.items.length > 0 &&
    cs.items.every((it) =>
      itemIds(it).every((id) => cs.decisions[id] !== undefined),
    )
  );
}

/** Create a fresh panel store. */
export function createPanelStore(): PanelStore {
  let logSeq = 0;
  let toolSeq = 0;

  // ---- mutable core (selectors are recomputed into the snapshot) ----
  const core = {
    phase: "connecting" as Phase,
    hasProject: false,
    files: [] as FileEntry[],
    project: "",
    compiling: false,
    plan: null as PlanState | null,
    changeset: null as ChangesetState | null,
    // Per-turn streaming buffers (reasoning / answer / tools). Reset whenever a
    // new turn starts (chat / plan_feedback / plan_approve / reset).
    turn: emptyTurn(),
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
      turn: core.turn,
      log: core.log,
      connected: core.connected,
      canSend,
    };
  }

  function emit(): void {
    snapshot = computeSnapshot();
    for (const listener of listeners) listener(snapshot);
  }

  function pushLog(
    kind: LogKind,
    text: string,
    stage?: ProgressStage,
    tools?: AgentTool[],
  ): void {
    logSeq += 1;
    const entry: LogEntry = { id: logSeq, kind, text };
    if (stage) entry.stage = stage;
    if (tools) entry.tools = tools;
    // Drop oldest beyond the cap (features/06 ## Behaviors).
    const next =
      core.log.length >= MAX_LOG_ENTRIES ? core.log.slice(1) : core.log.slice();
    next.push(entry);
    core.log = next;
  }

  /**
   * EUD-069: archive the live tool rows as a compact log entry (carrying the
   * rows for expandable history) and clear the buffer when a turn ends. Without
   * this, leftover rows occupy the live surface into the next phase — the
   * live-E2E layout crush (14 stale rows squeezed the plan card to 33px).
   * Called BEFORE archiveTurnAnswer so the order reads tools → prose.
   */
  function archiveTurnTools(): void {
    const tools = core.turn.tools;
    if (tools.length === 0) return;
    const counts = new Map<string, number>();
    for (const t of tools) counts.set(t.name, (counts.get(t.name) ?? 0) + 1);
    const parts = [...counts].map(([name, c]) =>
      c > 1 ? `${name}×${c}` : name,
    );
    pushLog("info", `도구 호출 ${tools.length}건 — ${parts.join(", ")}`,
      undefined, tools);
    core.turn = { ...core.turn, tools: [] };
  }

  /**
   * F2: archive the live streamed-answer buffer (`turn.answer`) as a prominent
   * agent log entry when a turn ends WITHOUT an `answer{}` (plan / changeset /
   * error). The live AgentAnswer bubble renders `turn.answer` only while the
   * panel is `thinking`, so prose streamed via `delta` before a plan/changeset/
   * error would otherwise be shown live then silently discarded at the
   * transition. The `answer{}` path is authoritative (the server final text) and
   * supersedes the buffer — `answerReceived` does NOT call this, so there is no
   * double-log. Empty/whitespace buffer = no-op. The buffer is cleared after
   * archiving so a later transition in the same turn cannot re-archive it.
   */
  function archiveTurnAnswer(): void {
    if (core.turn.answer.trim().length > 0) {
      pushLog("agent", core.turn.answer);
      core.turn = { ...core.turn, answer: "", answerStarted: false };
    }
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

    agentEvent(kind, detail, data) {
      // Accumulate the streamed agent_event into the per-turn buffers (EUD-065 /
      // decision 06). Raw internal kind identifiers (delta/answer/token_usage/
      // turn_done/item_started/item_completed/event) MUST NEVER reach the log —
      // they drive the Reasoning / Response / Tool surfaces, not a text line.
      switch (kind) {
        case "reasoning":
          core.turn = {
            ...core.turn,
            reasoning: core.turn.reasoning + detail,
          };
          break;
        case "delta":
          // The first answer delta marks the answer as started (so the Reasoning
          // block collapses); subsequent deltas grow the live answer text.
          core.turn = {
            ...core.turn,
            answer: core.turn.answer + detail,
            answerStarted: true,
          };
          break;
        case "tool_call": {
          // Open a running Tool row by name; carry the call args (EUD-068).
          toolSeq += 1;
          const tool: AgentTool = {
            id: `tool-${toolSeq}`,
            name: detail || "tool",
            state: "running",
          };
          if (data?.args) tool.args = data.args;
          core.turn = { ...core.turn, tools: [...core.turn.tools, tool] };
          break;
        }
        case "tool_result": {
          // Flip the latest still-running Tool row to done/failed; attach the
          // result text (EUD-068). A non-"completed" server status (failed /
          // declined) flags the row; absence of data keeps the legacy done flip.
          const failed =
            data?.status !== undefined && data.status !== "completed";
          const tools = core.turn.tools.slice();
          for (let i = tools.length - 1; i >= 0; i -= 1) {
            if (tools[i].state === "running") {
              tools[i] = {
                ...tools[i],
                state: failed ? "failed" : "done",
                ...(data?.result ? { detail: data.result } : {}),
              };
              break;
            }
          }
          core.turn = { ...core.turn, tools };
          break;
        }
        default:
          // thinking / answer / token_usage / turn_done / item_* / event and any
          // other kind: no user-facing text. Swallow (no log leak).
          break;
      }
      emit();
    },

    answerReceived(_text) {
      // answer-only turn (no edits): thinking --> ready. (The text is logged by
      // the App layer so the bubble carries the right styling.) EUD-069: the
      // tool rows archive BEFORE the App logs the answer text, so the history
      // reads tools → answer.
      archiveTurnTools();
      core.turnInFlight = false;
      core.phase = "ready";
      emit();
    },

    planReceived(markdown, revision) {
      // propose_plan ENDS the codex turn (the turn is no longer in flight); the
      // panel now awaits feedback/approve. thinking --> plan_review; a higher
      // revision REPLACES the active card.
      // EUD-069: archive the tool rows, THEN (F2) any prose streamed before the
      // plan turn-end — history order tools → prose.
      archiveTurnTools();
      archiveTurnAnswer();
      core.turnInFlight = false;
      core.plan = { markdown, revision };
      core.phase = "plan_review";
      emit();
    },

    changesetReceived(requestId, items) {
      // thinking --> changeset_review. Fresh decisions map (no item decided yet).
      // EUD-069: archive the tool rows, THEN (F2) any prose streamed before the
      // changeset turn-end.
      archiveTurnTools();
      archiveTurnAnswer();
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
        // Resolve against ALL currently-undecided ids. Derive ids per item via
        // itemIds (a dat group's ids live on its properties, NOT on it.id), then
        // keep only the still-undecided ones.
        targetIds = core.changeset.items
          .flatMap((it) => itemIds(it))
          .filter((id) => core.changeset!.decisions[id] === undefined);
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
      // EUD-069: archive the tool rows; F2: archive any prose streamed before
      // the turn errored out.
      archiveTurnTools();
      archiveTurnAnswer();
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
      // prior plan card and the per-turn streaming buffers; the changeset is left
      // intact (server archives it).
      core.turnInFlight = true;
      core.plan = null;
      core.turn = emptyTurn();
      core.phase = "thinking";
      emit();
    },

    planFeedbackSent() {
      // plan_review --> thinking (iterate; next plan{revision+1} replaces card).
      // A new turn — reset the per-turn streaming buffers.
      core.turnInFlight = true;
      core.turn = emptyTurn();
      core.phase = "thinking";
      emit();
    },

    planApproveSent() {
      // plan_review --> thinking (apply the approved plan). A new turn — reset the
      // per-turn streaming buffers.
      core.turnInFlight = true;
      core.turn = emptyTurn();
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

    resetSent() {
      // [새 대화]: the server drops the retained codex thread (EUD-064); the client
      // clears the conversation log, plan, changeset, pending decision, and the
      // per-turn streaming buffers, returning to ready for a fresh conversation.
      // F3: if an undecided changeset is discarded, the server default-accepts its
      // undecided items (features/05). Surface that as the FIRST entry of the fresh
      // log so the discard is not silent.
      const discardedUndecided =
        core.changeset !== null && !isChangesetFullyDecided(core.changeset);
      core.turnInFlight = false;
      core.plan = null;
      core.changeset = null;
      core.pendingDecision = null;
      core.turn = emptyTurn();
      core.log = [];
      if (discardedUndecided) {
        pushLog("warn", "미결정 변경사항은 자동 적용 처리되었습니다.");
      }
      core.phase = "ready";
      emit();
    },

    // ---- logging ----
    log(kind, text, stage) {
      pushLog(kind, text, stage);
      emit();
    },
  };
}
