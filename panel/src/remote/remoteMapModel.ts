/**
 * The phone's Map screen as data: the session list rows, the candidate
 * summary, the whole-map render request, and one Map conversation folded from
 * the same events the desktop Map window listens to (`agent_event`, `answer`,
 * `error`, `ask`, `map_run_started`). A run started on the PC or by an EPS
 * team task is adopted from its transcript, exactly as the Map window does.
 */
import type { ChatAttachment, ContextUsage } from "@/lib/ipc";
import type { AskQuestion, SessionMeta } from "@/lib/protocol";
import type { AgentEventData, PendingSteer, TurnState } from "@/state/store";
import {
  archiveMapTurn,
  conversationFromSession,
  createMapTurn,
  createMapTurnCursor,
  reduceMapTurnEvent,
  type MapTurnCursor,
} from "@/map/mapConversation";
import type { MapConversationEntry } from "@/map/MapAgentPanel";
import type {
  CandidateStateView,
  LayerDiffCount,
  MapBootstrapResponse,
  MapDiff,
  MapLayer,
  MapRunStartedPayload,
  MapRunTranscript,
  MapView,
  RenderCommand,
} from "@/map/mapProtocol";
import {
  runAdoptionEntries,
  runAlreadyAdopted,
  runHasTerminalEvent,
  stampRunRequestId,
} from "@/map/mapRunAdoption";

// ---------------------------------------------------------------- sessions

export interface MapSessionRow {
  id: string;
  name: string;
  /** Set for a team session an EPS conversation's map task runs in. */
  teamBadge: string | null;
  lastConversationAt: number;
}

/** Map sessions newest first; a team session carries its "팀 작업" badge. */
export function mapSessionRows(sessions: SessionMeta[]): MapSessionRow[] {
  return sessions
    .filter((session) => (session.kind ?? "eps") === "map")
    .slice()
    .sort(
      (left, right) =>
        right.lastConversationAt - left.lastConversationAt || right.createdAt - left.createdAt,
    )
    .map((session) => ({
      id: session.id,
      name: session.name,
      teamBadge: session.teamParent ? "팀 작업" : null,
      lastConversationAt: session.lastConversationAt,
    }));
}

// ---------------------------------------------------------------- candidate

export interface CandidateSummary {
  revision: number;
  /** One line per changed layer, e.g. "유닛 추가 2 · 삭제 1". */
  changes: string[];
  warnings: string[];
  errors: string[];
  canApply: boolean;
  canUndo: boolean;
  canDiscard: boolean;
  /** Earlier revisions the candidate can go back to, newest first. */
  revertTargets: number[];
}

const LAYER_NAMES: Array<[keyof Pick<MapDiff, "units" | "buildings" | "doodads" | "sprites" | "locations">, string]> = [
  ["units", "유닛"],
  ["buildings", "건물"],
  ["doodads", "두들"],
  ["sprites", "스프라이트"],
  ["locations", "로케이션"],
];

function countLine(label: string, count: LayerDiffCount): string | null {
  const parts = [
    count.added > 0 ? `추가 ${count.added}` : null,
    count.removed > 0 ? `삭제 ${count.removed}` : null,
    count.moved > 0 ? `이동 ${count.moved}` : null,
    count.changed > 0 ? `변경 ${count.changed}` : null,
  ].filter((part): part is string => part !== null);
  return parts.length > 0 ? `${label} ${parts.join(" · ")}` : null;
}

/** What the candidate changed against the saved map, and what can be done with it. */
export function candidateSummary(state: CandidateStateView): CandidateSummary {
  const current = state.revisions.find((revision) => revision.revision === state.currentRevision);
  const changes: string[] = [];
  const warnings: string[] = [];
  const errors: string[] = [];
  if (current) {
    const diff = current.diff;
    if (diff.terrainCells > 0) changes.push(`지형 ${diff.terrainCells}칸`);
    for (const [key, label] of LAYER_NAMES) {
      const line = countLine(label, diff[key]);
      if (line) changes.push(line);
    }
    if ((diff.properties ?? 0) > 0) changes.push(`맵 속성 ${diff.properties}개`);
    if (diff.outsideTarget > 0) warnings.push(`대상 영역 밖 변경 ${diff.outsideTarget}칸`);
    if (diff.protected > 0) warnings.push(`보호 영역 변경 ${diff.protected}칸`);
    warnings.push(...current.verification.warnings);
    errors.push(...current.verification.errors);
  }
  if (state.sourceDiverged) {
    warnings.unshift(
      "저장된 원본 맵이 바뀌어 후보를 다시 맞추지 못했습니다. 적용하면 저장된 맵을 후보로 덮어씁니다(백업은 남습니다).",
    );
  }
  if (state.stale) {
    warnings.unshift("원본 맵이 바뀌었습니다. 진행 중인 요청이 끝나면 후보가 새 원본을 따라갑니다.");
  }
  const revertTargets = state.revisions
    .map((revision) => revision.revision)
    .filter((revision) => revision < state.currentRevision)
    .sort((left, right) => right - left);
  if (state.currentRevision > 0 && !revertTargets.includes(0)) revertTargets.push(0);
  return {
    revision: state.currentRevision,
    changes,
    warnings,
    errors,
    canApply: state.canApply,
    canUndo: state.canUndo,
    canDiscard: state.currentRevision > 0,
    revertTargets,
  };
}

export const RENDER_LAYERS: MapLayer[] = ["terrain", "doodads", "sprites", "units", "buildings"];

/** The whole map of one view at the minimap's scale (8 px per tile). */
export function wholeMapRender(state: CandidateStateView, view: Exclude<MapView, "diff" | "draft">): RenderCommand {
  return {
    sessionId: state.sessionId,
    view,
    x: 0,
    y: 0,
    width: state.baseline.width,
    height: state.baseline.height,
    scale: 8,
    layers: RENDER_LAYERS,
  };
}

// ---------------------------------------------------------------- conversation

export interface RemoteMapAsk {
  requestId: string;
  questions: AskQuestion[];
  submitting: boolean;
  waitSeconds?: number;
  receivedAt?: number;
}

export interface RemoteMapState {
  conversation: MapConversationEntry[];
  turn: TurnState;
  /** A run of this session is in flight (sent here or adopted). */
  inFlight: boolean;
  ask?: RemoteMapAsk;
  contextUsage: ContextUsage | null;
  /** Messages sent to the running request that it has not received yet. */
  pendingSteers: PendingSteer[];
}

export interface PendingMapAsk {
  sessionId: string;
  requestId: string;
  questions: AskQuestion[];
  waitSeconds?: number;
  candidateRevision?: string;
}

type Payload = Record<string, unknown>;

export interface RemoteMapStore {
  getState(): RemoteMapState;
  subscribe(listener: () => void): () => void;
  /** The log sequence persisted with the conversation. */
  logSequence(): number;
  /** This screen sends a request on candidate `revisionKey`. */
  sendStarted(text: string, attachments: ChatAttachment[], revisionKey: string): void;
  /** The request this screen sent settled; `error` when it failed. */
  sendSettled(error?: string): void;
  /** The user stopped the run. */
  cancelled(): void;
  event(name: string, payload: Payload): void;
  /** `true` when the run was started elsewhere and must be adopted from its transcript. */
  runStarted(payload: MapRunStartedPayload): boolean;
  adoptRun(run: MapRunTranscript): void;
  restoreAsk(pending: PendingMapAsk, revisionKey: string): void;
  askSubmitting(submitting: boolean): void;
  askAnswered(): void;
  note(kind: MapConversationEntry["kind"], text: string): void;
  /** The conversation after a rewind to the rows before `entryId`. */
  rewound(entryId: number): void;
  /** A message was sent to the running request; it waits as "전달 대기". */
  steerSent(steer: PendingSteer): void;
  /** Remove these waiting messages and return them. */
  steerTaken(steerIds: readonly string[]): PendingSteer[];
  /**
   * A `steer` event: received by the run (user rows at this point), started
   * as the next request this screen sent (rows before its run), or handed
   * back; returns the waiting messages that go back to the composer.
   */
  steer(payload: Payload): PendingSteer[];
}

/** The rows a rewind from user message `entryId` keeps. */
export function rewindPrefix(
  conversation: MapConversationEntry[],
  entryId: number,
): MapConversationEntry[] {
  return conversation.filter((entry) => entry.id < entryId);
}

function eventData(raw: unknown): AgentEventData {
  const data = (raw ?? {}) as Payload;
  return {
    ...(typeof data.callId === "string" ? { callId: data.callId } : {}),
    ...(typeof data.args === "string" ? { args: data.args } : {}),
    ...(typeof data.result === "string" ? { result: data.result } : {}),
    ...(typeof data.status === "string" ? { status: data.status } : {}),
  };
}

/** One Map session's conversation, following its live and adopted runs. */
export function createRemoteMapStore(bootstrap: MapBootstrapResponse): RemoteMapStore {
  const sessionId = bootstrap.session.id;
  const restored = conversationFromSession(bootstrap);
  let state: RemoteMapState = {
    conversation: restored,
    turn: createMapTurn(),
    inFlight: false,
    contextUsage: bootstrap.session.contextUsage ?? null,
    pendingSteers: [],
  };
  let sequence = Math.max(0, ...restored.map((entry) => entry.id));
  let cursor: MapTurnCursor = createMapTurnCursor();
  // The candidate revision the current run's events carry.
  let eventRevision = "";
  let ended = false;
  // The run was adopted rather than sent here: its end closes it.
  let recovered = false;
  // The run's streamed prose moved into the conversation at a steer.
  let textArchived = false;
  const listeners = new Set<() => void>();

  const set = (next: Partial<RemoteMapState>) => {
    state = { ...state, ...next };
    for (const listener of [...listeners]) listener();
  };
  const append = (entries: MapConversationEntry[]) => {
    if (entries.length > 0) set({ conversation: [...state.conversation, ...entries] });
  };
  const push = (kind: MapConversationEntry["kind"], text: string) => {
    sequence += 1;
    append([{ id: sequence, kind, text }]);
  };
  const resetTurn = () => {
    cursor = createMapTurnCursor();
    set({ turn: createMapTurn() });
  };
  const archive = (fallback = "") => {
    const archived = archiveMapTurn(state.turn, fallback, sequence);
    sequence = archived.logSequence;
    append(archived.entries);
    resetTurn();
  };
  const finishRecovered = () => {
    if (!recovered) return;
    recovered = false;
    eventRevision = "";
    set({ inFlight: false });
  };
  const current = (payload: Payload) =>
    payload.sessionId === sessionId && payload.candidateRevision === eventRevision;

  const event = (name: string, payload: Payload) => {
    if (!current(payload)) return;
    switch (name) {
      case "agent_event": {
        if (!state.inFlight || ended) return;
        const next = reduceMapTurnEvent(
          state.turn,
          cursor,
          String(payload.kind ?? ""),
          String(payload.detail ?? ""),
          eventData(payload.data),
        );
        if (next.unpairedToolStart !== undefined) {
          const { name: tool, args } = next.unpairedToolStart;
          push("info", args ? `도구 호출 시작 — ${tool}\n${args}` : `도구 호출 시작 — ${tool}`);
        }
        cursor = next.cursor;
        set({ turn: next.turn });
        return;
      }
      case "context_usage":
        set({ contextUsage: (payload.tokenUsage ?? null) as ContextUsage | null });
        return;
      case "answer":
        if (ended) return;
        ended = true;
        archive(textArchived ? "" : String(payload.text ?? ""));
        textArchived = false;
        set({ ask: undefined });
        finishRecovered();
        return;
      case "error":
        if (ended) return;
        ended = true;
        archive();
        push("error", String(payload.message ?? "Map Agent 요청이 실패했습니다."));
        set({ ask: undefined });
        finishRecovered();
        return;
      case "ask": {
        if (!state.inFlight || ended) return;
        const requestId = String(payload.requestId);
        if (payload.status === "expired") {
          if (state.ask?.requestId === requestId) set({ ask: undefined });
          return;
        }
        set({
          ask: {
            requestId,
            questions: (payload.questions ?? []) as AskQuestion[],
            submitting: false,
            ...(typeof payload.waitSeconds === "number"
              ? { waitSeconds: payload.waitSeconds, receivedAt: Date.now() }
              : {}),
          },
        });
        return;
      }
      default:
        return;
    }
  };

  return {
    getState: () => state,
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    logSequence: () => sequence,
    sendStarted(text, attachments, revisionKey) {
      recovered = false;
      textArchived = false;
      eventRevision = revisionKey;
      sequence += 1;
      append([
        {
          id: sequence,
          kind: "you",
          text: text.trim() || "첨부 파일을 분석해 주세요.",
          mapMentions: [],
          attachments,
        },
      ]);
      resetTurn();
      ended = false;
      set({ inFlight: true, ask: undefined });
    },
    sendSettled(error) {
      if (!ended) {
        ended = true;
        archive();
        if (error !== undefined) push("error", error);
      }
      eventRevision = "";
      recovered = false;
      set({ inFlight: false });
    },
    cancelled() {
      ended = true;
      recovered = false;
      eventRevision = "";
      archive();
      push("info", "작업을 중단했습니다.");
      set({ inFlight: false, ask: undefined });
    },
    event,
    runStarted(payload) {
      if (payload.sessionId !== sessionId) return false;
      if (state.inFlight && !recovered) {
        // This screen sent it: follow the revision it actually started on.
        eventRevision = payload.candidateRevision;
        set({ conversation: stampRunRequestId(state.conversation, payload.requestId) });
        return false;
      }
      return !state.inFlight;
    },
    adoptRun(run) {
      if (runAlreadyAdopted(state.conversation, run.requestId)) return;
      const adoption = runAdoptionEntries(run, sequence);
      sequence = adoption.logSequence;
      append(adoption.entries);
      resetTurn();
      eventRevision = run.candidateRevision;
      ended = false;
      recovered = true;
      set({ inFlight: true, ask: undefined });
      for (const recorded of run.events) {
        if (recorded.name === "ask") continue;
        event(recorded.name, recorded.payload);
      }
      if (run.inFlight) return;
      if (!runHasTerminalEvent(run)) {
        ended = true;
        archive();
        finishRecovered();
      }
    },
    restoreAsk(pending, revisionKey) {
      if (pending.sessionId !== sessionId) return;
      const revision = pending.candidateRevision ?? revisionKey;
      if (revision !== revisionKey) return;
      if (!state.inFlight) {
        eventRevision = revision;
        ended = false;
        recovered = true;
      }
      set({
        inFlight: true,
        ask: {
          requestId: pending.requestId,
          questions: pending.questions,
          submitting: false,
          ...(typeof pending.waitSeconds === "number"
            ? { waitSeconds: pending.waitSeconds, receivedAt: Date.now() }
            : {}),
        },
      });
    },
    askSubmitting(submitting) {
      if (state.ask) set({ ask: { ...state.ask, submitting } });
    },
    askAnswered() {
      set({ ask: undefined });
    },
    note: push,
    rewound(entryId) {
      resetTurn();
      set({
        conversation: rewindPrefix(state.conversation, entryId),
        ask: undefined,
        pendingSteers: [],
      });
    },
    steerSent(steer) {
      set({ pendingSteers: [...state.pendingSteers, steer] });
    },
    steerTaken(steerIds) {
      const ids = new Set(steerIds);
      const taken = state.pendingSteers.filter((pending) => ids.has(pending.steerId));
      if (taken.length > 0) {
        set({
          pendingSteers: state.pendingSteers.filter((pending) => !ids.has(pending.steerId)),
        });
      }
      return taken;
    },
    steer(payload) {
      if (payload.sessionId !== sessionId) return [];
      const steers = Array.isArray(payload.steers)
        ? (payload.steers as { steerId: string; text: string }[])
        : [];
      const ids = new Set(steers.map((steer) => steer.steerId));
      const known = state.pendingSteers.filter((pending) => ids.has(pending.steerId));
      if (known.length > 0) {
        set({
          pendingSteers: state.pendingSteers.filter((pending) => !ids.has(pending.steerId)),
        });
      }
      const rows = () =>
        steers.map((steer) => {
          const pending = known.find((candidate) => candidate.steerId === steer.steerId);
          sequence += 1;
          return {
            id: sequence,
            kind: "you" as const,
            text: pending?.text ?? steer.text,
            mapMentions: [],
            ...(pending && pending.attachments.length > 0
              ? { attachments: pending.attachments }
              : {}),
          };
        });
      if (payload.kind === "applied") {
        if (!state.inFlight || ended) return [];
        if (
          state.turn.blocks.some(
            (block) => block.type === "text" && block.text.trim().length > 0,
          )
        ) {
          textArchived = true;
        }
        archive();
        append(rows());
        return [];
      }
      if (payload.kind === "turn") {
        // A run this screen did not send is adopted from its transcript.
        if (!state.inFlight || recovered) return [];
        append(rows());
        resetTurn();
        textArchived = false;
        ended = false;
        return [];
      }
      return known;
    },
  };
}
