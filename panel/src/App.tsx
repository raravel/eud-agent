/**
 * Panel app shell (v2) - wires the Tauri IPC v2 client + state store to the
 * chat-first review UI (features/06_changeset-review-panel.md).
 *
 * Components: a status-rich Header (connection transitions + RAG state/elapsed),
 * the ConversationLog cards, a live AgentStream under the turn, the PlanView
 * feedback/approve surface, the ChangesetView accept/reject surface, and the
 * regated InstructionBox. Plan cards are archived into the conversation log as
 * agent entries when a plan arrives, is superseded by a higher revision, or is
 * approved.
 *
 * Data flow: IpcClient (Tauri invoke + listen) -> store actions + log entries
 * -> React snapshot via useSyncExternalStore -> components -> user intents call
 * client.send + the matching store action. Two pieces of UI-only state live here
 * (not protocol state): the current turn's agent_event list (for AgentStream)
 * and the RAG warmup state/timing (for the Header pill).
 */
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { toast } from "sonner";
import { Toaster } from "@/components/ui/sonner";
import { Header, type RagState } from "@/components/Header";
import { ConversationLog } from "@/components/ConversationLog";
import { ChangesetView } from "@/components/ChangesetView";
import { PlanView } from "@/components/PlanView";
import { InstructionBox, type ChatPayload } from "@/components/InstructionBox";
import { ConnectionNotice } from "@/components/ConnectionNotice";
import {
  SessionSidebar,
  type SessionActivity,
  type SessionSidebarRow,
} from "@/components/SessionSidebar";
import {
  ProjectSidebar,
  type ProjectPanelTab,
} from "@/components/ProjectSidebar";
import { createPanelStore } from "@/state/store";
import type { LogEntry, PanelStore } from "@/state/store";
import {
  IpcClient,
  codexModelSettingsGet,
  codexModelSettingsSave,
  wikiGet,
  wikiSave,
  workspaceList,
  workspaceRead,
  type LedgerEntry,
  type CodexModelSettings,
  type MemoryFile,
  type PanelLog,
  type PanelLogEntry,
  type ServerMessage,
  type SessionMeta,
  type SessionRecord,
  type WorkspaceFileEntry,
  type WorkspaceListResponse,
  type SetupMessage,
} from "@/lib/ipc";
import { progressLabel } from "@/lib/progress";
import { useProjectIdentityEffect } from "@/lib/projectIdentity";
import {
  bootstrapView,
  type BootstrapView,
} from "@/setup/bootstrap";
import { SetupScreen } from "@/setup/SetupScreen";
import { UpdateNotice } from "@/components/UpdateNotice";
import { createUpdater, type UpdateHandle } from "@/setup/update";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  discardAttachment,
  stageAttachment,
} from "@/lib/attachments";

/** codex login probe result (mirrors the Rust `CodexAuthState`). */
interface CodexAuthState {
  resolved: boolean;
  authed: boolean;
  detail: string;
}

/** OAuth poll cadence + ceiling: codex's browser flow rarely exceeds a minute. */
const CODEX_POLL_MS = 2000;
const CODEX_POLL_TIMEOUT_MS = 180000;

/**
 * Editor-liveness poll cadence. The bridge writes heartbeat.txt every ~1s and
 * the core treats a >3s-old heartbeat as stale, so a 2s probe recovers a downed
 * editor within ~1 cycle without churning the file IPC (the probe is the cheap
 * status-only path; the heavier `list` round-trip runs only on edges).
 */
const EDITOR_POLL_MS = 2000;

interface BootstrapState {
  active: boolean;
  view: BootstrapView;
  error: string | null;
}

/** panelLog schema version (features/sessions.md ## panelLog schema). */
const PANEL_LOG_SCHEMA_VERSION = 2;

/**
 * Serialize the live conversation log into the durable {@link PanelLog} subset
 * pushed via `session_update_log`: `id/kind/text` plus optional
 * `stage`/`tools`/`attachments` survive; transient turn/plan/changeset/wiki state
 * is dropped. `logSeq` advances restored store counters past existing ids.
 */
function serializePanelLog(log: readonly LogEntry[]): PanelLog {
  const entries: PanelLogEntry[] = log.map((entry) => {
    const next: PanelLogEntry = {
      id: entry.id,
      kind: entry.kind,
      text: entry.text,
    };
    if (entry.stage) next.stage = entry.stage;
    if (entry.tools) {
      next.tools = entry.tools.map((tool) => ({
        id: tool.id,
        name: tool.name,
        state: tool.state,
        ...(tool.args !== undefined ? { args: tool.args } : {}),
        ...(tool.detail !== undefined ? { detail: tool.detail } : {}),
      }));
    }
    if (entry.attachments) next.attachments = entry.attachments;
    return next;
  });
  const logSeq = log.reduce((max, entry) => (entry.id > max ? entry.id : max), 0);
  return { schemaVersion: PANEL_LOG_SCHEMA_VERSION, logSeq, log: entries };
}

interface SessionSlot {
  id: string;
  meta: SessionRecord;
  store: PanelStore;
  persisted: boolean;
  activity: SessionActivity;
  unsubscribe?: () => void;
  saveTimer?: number;
  planOpen: boolean;
  changesetOpen: boolean;
}


let draftSequence = 0;

function emptyPanelLog(): PanelLog {
  return { schemaVersion: PANEL_LOG_SCHEMA_VERSION, logSeq: 0, log: [] };
}

function draftSession(project: string): SessionRecord {
  draftSequence += 1;
  const now = Math.floor(Date.now() / 1000);
  return {
    id: `draft-${draftSequence}`,
    name: "새 대화",
    project,
    createdAt: now,
    updatedAt: now,
    threadId: null,
    pendingRequestIds: [],
    panelLog: emptyPanelLog(),
  };
}

function syncProjectState(target: PanelStore, source: PanelStore): void {
  const state = source.getState();
  if (state.connected) target.wsOpen();
  else target.wsConnecting();
  target.applyStatus({ compiling: state.compiling, project: state.project });
  target.applyList({ files: state.hasProject ? state.files : undefined });
  target.editorConnectionChanged(state.editorConnected);
  if (state.rag !== "unknown") target.ragWarmupChanged(state.rag);
  if (state.memory) {
    target.memoryReceived(state.memory.project, state.memory.files);
  }
  if (state.wikiData) {
    target.wikiReceived(state.wikiData.version, state.wikiData.entries);
  }
}


export default function App() {
  const projectStore = useMemo(() => createPanelStore(), []);
  const sessionsRef = useRef(new Map<string, SessionSlot>());
  const [sessionRevision, setSessionRevision] = useState(0);
  const toastedLogBySessionRef = useRef(new Map<string, number>());
  const [selectedSessionId, setSelectedSessionId] = useState<string | null>(null);
  const selectedSessionIdRef = useRef<string | null>(null);
  const loadedProjectRef = useRef<string | null>(null);
  const clientRef = useRef<IpcClient | null>(null);
  const updater = useMemo(() => createUpdater(), []);

  const selectedSlot = useMemo(
    () =>
      (selectedSessionId
        ? sessionsRef.current.get(selectedSessionId)
        : undefined) ?? null,
    [selectedSessionId, sessionRevision],
  );
  const store = selectedSlot?.store ?? projectStore;
  const state = store.getState();
  const projectState = useSyncExternalStore(
    projectStore.subscribe,
    projectStore.getState,
    projectStore.getState,
  );

  // ---- UI-only state (not protocol state) ----
  // The per-turn streaming buffers (reasoning / answer / tools) live in the STORE
  // (state.turn) now — the AgentStream + live AgentAnswer render from there, and
  // the store resets them per turn. No App-local agent_event list is needed.
  // RAG warmup visibility for the Header pill. `startedAt` drives the elapsed
  // counter while loading; a 1s tick re-renders so the seconds advance.
  const [ragState, setRagState] = useState<RagState>("idle");
  const ragStartRef = useRef<number | null>(null);
  const [ragElapsedSec, setRagElapsedSec] = useState(0);
  const [bootstrap, setBootstrap] = useState<BootstrapState>(() => ({
    active: false,
    view: bootstrapView(null, undefined),
    error: null,
  }));
  const bootstrapActiveRef = useRef(false);
  // First-run manifest check (EUD-132). null until the first `setup` snapshot
  // arrives; setup_required routes the whole panel to the SetupScreen.
  const [setup, setSetup] = useState<SetupMessage | null>(null);
  const bootstrapRunningRef = useRef(false);
  // Editor-liveness poll gate. Flips true once first-run setup is satisfied (or
  // a failed setup_status falls back to "assume configured"); the poll effect
  // then probes the editor every EDITOR_POLL_MS so a stale heartbeat at boot or
  // a mid-session editor restart recovers automatically.
  const [editorPollEnabled, setEditorPollEnabled] = useState(false);
  // codex login step (setup screen step 3). The OAuth path spawns the browser
  // flow in the backend and polls codex_login_status until it flips.
  const [codexBusy, setCodexBusy] = useState(false);
  const [codexError, setCodexError] = useState<string | null>(null);
  const codexPollRef = useRef<number | null>(null);
  // "에디터 켜기": true while the launch_editor command is in flight. The button
  // re-enables once the editor connects (editorConnected) or the spawn resolves/fails.
  const [launchPending, setLaunchPending] = useState(false);
  const [sessionSidebarCollapsed, setSessionSidebarCollapsed] = useState(false);
  const [projectSidebarOpen, setProjectSidebarOpen] = useState(true);
  const [projectPanelTab, setProjectPanelTab] =
    useState<ProjectPanelTab>("wiki");
  const [workspaceData, setWorkspaceData] =
    useState<WorkspaceListResponse | null>(null);
  const [workspacePath, setWorkspacePath] = useState<string | null>(null);
  const [workspaceContent, setWorkspaceContent] = useState<string | null>(null);
  const [workspaceLoading, setWorkspaceLoading] = useState(false);
  const [workspaceError, setWorkspaceError] = useState<string | null>(null);
  // Self-update banner state: the pending update (null until found) and a
  // session-scoped "나중에" dismissal. The check fires once (guarded by the ref).
  const [update, setUpdate] = useState<UpdateHandle | null>(null);
  const [updateDismissed, setUpdateDismissed] = useState(false);
  const updateCheckedRef = useRef(false);
  // Authenticated Codex model catalog + persisted global selection. The catalog
  // comes from app-server `model/list`; changing either select applies to the
  // next eud-agent turn and is saved by the Rust core.
  const [codexSettings, setCodexSettings] =
    useState<CodexModelSettings | null>(null);
  const [codexSettingsBusy, setCodexSettingsBusy] = useState(false);
  // Message undo/edit flow: the core must finish cancellation/rewind before the
  // input unlocks. `editDraft` is applied by InstructionBox without controlling
  // subsequent typing.
  const [editDraft, setEditDraft] = useState<ChatPayload | null>(null);
  const [messageActionBusy, setMessageActionBusy] = useState(false);
  const messageActionBusyRef = useRef(false);

  const bumpSessions = useCallback(() => {
    setSessionRevision((revision) => revision + 1);
  }, []);

  const attachSlot = useCallback(
    (slot: SessionSlot) => {
      if (slot.unsubscribe) return;
      slot.unsubscribe = slot.store.subscribe((snapshot) => {
        bumpSessions();
        if (!slot.persisted || snapshot.log.length === 0) return;
        if (slot.saveTimer !== undefined) window.clearTimeout(slot.saveTimer);
        slot.saveTimer = window.setTimeout(() => {
          void invoke("session_update_log", {
            id: slot.id,
            panelLog: serializePanelLog(slot.store.getState().log),
          }).catch(() => {
            // Session autosave is best-effort; the durable store reports failures.
          });
        }, 500);
      });
    },
    [bumpSessions],
  );

  const registerSession = useCallback(
    (record: SessionRecord): SessionSlot => {
      const existing = sessionsRef.current.get(record.id);
      if (existing) {
        existing.meta = record;
        existing.persisted = true;
        bumpSessions();
        return existing;
      }
      const sessionStore = createPanelStore();
      syncProjectState(sessionStore, projectStore);
      sessionStore.hydrate(record.panelLog ?? emptyPanelLog());
      const slot: SessionSlot = {
        id: record.id,
        meta: record,
        store: sessionStore,
        persisted: true,
        activity: record.pendingRequestIds.length > 0 ? "review" : "idle",
        planOpen: true,
        changesetOpen: true,
      };
      sessionsRef.current.set(slot.id, slot);
      attachSlot(slot);
      toastedLogBySessionRef.current.set(
        slot.id,
        record.panelLog?.logSeq ?? 0,
      );
      bumpSessions();
      return slot;
    },
    [attachSlot, bumpSessions, projectStore],
  );

  const createDraftSlot = useCallback((): SessionSlot => {
    const meta = draftSession(projectStore.getState().project);
    const sessionStore = createPanelStore();
    syncProjectState(sessionStore, projectStore);
    const slot: SessionSlot = {
      id: meta.id,
      meta,
      store: sessionStore,
      persisted: false,
      activity: "idle",
      planOpen: true,
      changesetOpen: true,
    };
    sessionsRef.current.set(slot.id, slot);
    attachSlot(slot);
    setSelectedSessionId(slot.id);
    selectedSessionIdRef.current = slot.id;
    bumpSessions();
    return slot;
  }, [attachSlot, bumpSessions, projectStore]);

  useEffect(() => {
    selectedSessionIdRef.current = selectedSessionId;
  }, [selectedSessionId]);


  useEffect(
    () => () => {
      for (const slot of sessionsRef.current.values()) {
        slot.unsubscribe?.();
        if (slot.saveTimer !== undefined) window.clearTimeout(slot.saveTimer);
      }
    },
    [],
  );

  const loadCodexModelSettings = useCallback(async () => {
    setCodexSettingsBusy(true);
    try {
      setCodexSettings(await codexModelSettingsGet());
    } catch {
      setCodexSettings(null);
      toast.error("Codex 모델 목록을 불러오지 못했습니다.");
    } finally {
      setCodexSettingsBusy(false);
    }
  }, []);

  useEffect(() => {
    if (editorPollEnabled) void loadCodexModelSettings();
  }, [editorPollEnabled, loadCodexModelSettings]);

  const handleCodexSettingsChange = useCallback(
    async (model: string, reasoningEffort: string) => {
      setCodexSettingsBusy(true);
      try {
        setCodexSettings(
          await codexModelSettingsSave(model, reasoningEffort),
        );
      } catch {
        toast.error("Codex 모델 설정을 저장하지 못했습니다.");
      } finally {
        setCodexSettingsBusy(false);
      }
    },
    [],
  );

  useEffect(() => {
    bootstrapActiveRef.current = bootstrap.active;
  }, [bootstrap.active]);

  // Tick the RAG elapsed counter once a second while loading.
  useEffect(() => {
    if (ragState !== "loading") return;
    const id = setInterval(() => {
      if (ragStartRef.current !== null) {
        setRagElapsedSec((Date.now() - ragStartRef.current) / 1000);
      }
    }, 1000);
    return () => clearInterval(id);
  }, [ragState]);

  // Every error/warn log entry ALSO pops a toast so a problem is noticeable even
  // when the conversation is scrolled away or the user is on the input. The log
  // Toast high-water marks are session-scoped so selecting an old conversation
  // neither replays its historical alerts nor suppresses a newer session's ids.
  useEffect(() => {
    const sessionId = selectedSlot?.id;
    if (!sessionId) return;
    let highWater = toastedLogBySessionRef.current.get(sessionId) ?? 0;
    for (const entry of state.log) {
      if (entry.id <= highWater) continue;
      if (entry.kind === "error") toast.error(entry.text);
      else if (entry.kind === "warn") toast.warning(entry.text);
      highWater = Math.max(highWater, entry.id);
    }
    toastedLogBySessionRef.current.set(sessionId, highWater);
  }, [selectedSlot?.id, state.log]);

  // Global editor/project events fan out to every session store. Turn events
  // route to the backend execution owner, not whichever sidebar row is visible.
  const onMessage = useCallback(
    (msg: ServerMessage) => {
      const scopedId =
        "sessionId" in msg && typeof msg.sessionId === "string"
          ? msg.sessionId
          : null;
      const sessionStore = () =>
        scopedId ? (sessionsRef.current.get(scopedId)?.store ?? null) : null;
      const forEveryStore = (apply: (target: PanelStore) => void) => {
        apply(projectStore);
        for (const slot of sessionsRef.current.values()) apply(slot.store);
      };

      switch (msg.type) {
        case "status":
          forEveryStore((target) =>
            target.applyStatus({ compiling: msg.compiling, project: msg.project }),
          );
          break;
        case "list":
          forEveryStore((target) =>
            target.applyList({ files: msg.files, error: msg.error }),
          );
          break;
        case "memory":
          forEveryStore((target) => target.memoryReceived(msg.project, msg.files));
          break;
        case "memory_saved":
          forEveryStore((target) => target.memorySaved(msg.file));
          projectStore.log("ok", "메모리를 저장했습니다.");
          break;
        case "wiki":
          forEveryStore((target) =>
            target.wikiReceived(msg.version, msg.entries),
          );
          break;
        case "setup":
          setSetup(msg);
          if (!msg.setup_required) {
            bootstrapActiveRef.current = false;
            setBootstrap((prev) =>
              prev.active ? { ...prev, active: false } : prev,
            );
            setEditorPollEnabled(true);
          }
          break;
        case "progress": {
          if (msg.stage === "bootstrap") {
            if (msg.detail === "done") {
              bootstrapActiveRef.current = false;
              setBootstrap((prev) => ({ ...prev, active: false, error: null }));
              void clientRef.current?.send({ type: "setup_status" });
              break;
            }
            const view = bootstrapView(msg.pct, msg.detail);
            bootstrapActiveRef.current = true;
            setBootstrap({
              active: true,
              view,
              error: view.phase === "error" ? view.label : null,
            });
            break;
          }
          if (bootstrapActiveRef.current) {
            bootstrapActiveRef.current = false;
            setBootstrap((prev) => ({ ...prev, active: false }));
          }
          if (msg.stage === "rag_warmup") {
            const previous = projectStore.getState().rag;
            const next =
              msg.detail === "done"
                ? "ready"
                : msg.detail?.startsWith("error")
                  ? "unavailable"
                  : "loading";
            forEveryStore((target) => target.ragWarmupChanged(next));
            if (next !== previous && next === "unavailable") {
              const { kind, text } = progressLabel(msg.stage, msg.detail);
              projectStore.log(kind, text, msg.stage);
            }
            if (next === "loading") {
              ragStartRef.current = Date.now();
              setRagElapsedSec(0);
            }
            setRagState(next);
            break;
          }
          const target = sessionStore();
          if (target) {
            target.progressReceived(msg.stage);
            const { kind, text } = progressLabel(msg.stage, msg.detail);
            target.log(kind, text, msg.stage);
          }
          break;
        }
        case "agent_event":
          sessionStore()?.agentEvent(msg.kind, msg.detail, msg.data);
          break;
        case "answer":
          sessionStore()?.answerReceived(msg.text);
          break;
        case "plan": {
          const targetSlot = scopedId
            ? sessionsRef.current.get(scopedId)
            : undefined;
          if (!targetSlot) break;
          const target = targetSlot.store;
          const prior = target.getState().plan;
          if (prior === null || prior.revision !== msg.revision) {
            targetSlot.planOpen = true;
          }
          if (prior !== null && prior.revision !== msg.revision) {
            target.log("agent", `계획안(rev ${prior.revision})이 갱신되었습니다.`);
          }
          target.planReceived(msg.markdown, msg.revision);
          target.log("agent", `계획안(rev ${msg.revision})이 도착했습니다.`);
          break;
        }
        case "changeset": {
          const targetSlot = scopedId
            ? sessionsRef.current.get(scopedId)
            : undefined;
          if (!targetSlot) break;
          const target = targetSlot.store;
          const prior = target.getState().changeset;
          if (prior === null || prior.request_id !== msg.request_id) {
            targetSlot.changesetOpen = true;
          }
          target.changesetReceived(msg.request_id, msg.items);
          target.log("agent", `변경사항 ${msg.items.length}건을 검토하세요.`);
          break;
        }
        case "rollback_result": {
          const target = sessionStore();
          if (!target) break;
          const decision = target.getState().pendingDecision?.decision;
          const count = msg.ids.length;
          target.rollbackResult(msg.ids, msg.ok);
          if (!msg.ok) {
            const label = decision === "accept" ? "적용 실패" : "되돌리기 실패";
            target.log("warn", msg.error ? `${label}: ${msg.error}` : `${label} (${count}건)`);
          } else if (decision === "accept") {
            target.log("ok", count > 0 ? `적용 유지 (${count}건)` : "적용 유지");
          } else {
            target.log("ok", `되돌림 (${count}건)`);
          }
          break;
        }
        case "error": {
          const target = sessionStore();
          if (!target) break;
          target.errorReceived(msg.message);
          target.log("error", `오류: ${msg.message}`);
          break;
        }
        case "session_activity": {
          const slot = sessionsRef.current.get(msg.sessionId);
          if (!slot) break;
          const previous = slot.activity;
          slot.activity = msg.activity;
          if (previous !== msg.activity && msg.activity === "running_write") {
            slot.store.log("ok", "격리 워크스페이스에서 변경을 시작합니다.");
          }
          bumpSessions();
          break;
        }
        default:
          break;
      }
    },
    [projectStore],
  );

  // Boot the IPC client once. Project lifecycle state fans out to all session
  // stores; switching the visible row never reconnects the transport.
  useEffect(() => {
    projectStore.wsConnecting();
    const client = new IpcClient({
      onMessage,
      onLog: (kind, text) => {
        const id = selectedSessionIdRef.current;
        const target =
          (id ? sessionsRef.current.get(id)?.store : undefined) ?? projectStore;
        if (kind === "info") target.log("info", text);
        else target.log("warn", text);
      },
      onOpenChange: (open) => {
        if (open) projectStore.wsOpen();
        else projectStore.wsError();
        for (const slot of sessionsRef.current.values()) {
          if (open) slot.store.wsOpen();
          else slot.store.wsError();
        }
      },
      onEditorChange: (connected) => {
        projectStore.editorConnectionChanged(connected);
        for (const slot of sessionsRef.current.values()) {
          slot.store.editorConnectionChanged(connected);
        }
      },
    });
    clientRef.current = client;
    void client.connect().then(() =>
      client.send({ type: "setup_status" }).then((ok) => {
        if (!ok) setEditorPollEnabled(true);
      }),
    );
    return () => {
      client.stop();
      clientRef.current = null;
    };
  }, [onMessage, projectStore]);

  // `session_loaded` is a SIGNAL only (features/sessions.md): the core emits it
  // after a session_open reconnect completes. Its payload carries nothing
  // rendered raw (rules.md forbids raw kind identifiers as user text); the panel
  // already hydrated from the session_open return value, so this just pulls a
  // fresh editor snapshot to settle live state. Registered outside the IpcClient
  // because it is not part of the closed ServerMessage push set.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void listen("session_loaded", () => {
      void clientRef.current?.refresh();
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);


  // Editor-liveness poll. Once armed (first-run setup satisfied), probe the
  // editor every EDITOR_POLL_MS. The transport stays open throughout, so this
  // only drives editorConnected (send gate + ConnectionNotice) and recovers a
  // stale-heartbeat-at-boot or a mid-session editor restart with no user action.
  useEffect(() => {
    if (!editorPollEnabled) return;
    const client = clientRef.current;
    if (!client) return;
    let cancelled = false;
    const probe = () => {
      if (!cancelled) void client.refresh();
    };
    probe(); // immediate — no EDITOR_POLL_MS dead window before the first probe
    const id = window.setInterval(probe, EDITOR_POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, [editorPollEnabled]);

  // Populate the persistent left sidebar with sessions owned by the current
  // editor project. Loading a row is read-only and never steals the execution lane.
  useEffect(() => {
    const project = projectState.project.trim();
    if (!projectState.hasProject || !project || loadedProjectRef.current === project) {
      return;
    }
    loadedProjectRef.current = project;
    let cancelled = false;
    void invoke<SessionMeta[]>("session_list")
      .then((rows) =>
        Promise.all(
          rows
            .filter((row) => row.project === project)
            .map((row) => invoke<SessionRecord>("session_load", { id: row.id })),
        ),
      )
      .then((records) => {
        if (cancelled) return;
        for (const slot of sessionsRef.current.values()) {
          slot.unsubscribe?.();
          if (slot.saveTimer !== undefined) window.clearTimeout(slot.saveTimer);
        }
        sessionsRef.current.clear();
        const slots = records.map(registerSession);
        for (const slot of slots) {
          if (slot.meta.pendingRequestIds.length === 0) continue;
          slot.activity = "review";
          void invoke<SessionRecord>("session_open", { id: slot.id })
            .then((record) => {
              slot.meta = record;
              bumpSessions();
            })
            .catch((error) => {
              slot.activity = "error";
              slot.store.log(
                "error",
                `변경사항을 복구하지 못했습니다: ${String(error)}`,
              );
              bumpSessions();
            });
        }
        const first = slots[0] ?? createDraftSlot();
        setSelectedSessionId(first.id);
        selectedSessionIdRef.current = first.id;
        bumpSessions();
      })
      .catch((error) => {
        loadedProjectRef.current = null;
        const fallback =
          sessionsRef.current.get(selectedSessionIdRef.current ?? "") ??
          createDraftSlot();
        fallback.store.log(
          "error",
          `세션을 불러오지 못했습니다: ${String(error)}`,
        );
      });
    return () => {
      cancelled = true;
    };
  }, [
    bumpSessions,
    createDraftSlot,
    projectState.hasProject,
    projectState.project,
    registerSession,
  ]);

  // Setup flow, download step: once the editor folder is picked (or was already
  // configured) and assets are still missing, start the bootstrap download.
  // Progress streams in as `progress {stage: "bootstrap"}`; the final "done"
  // re-queries setup_status, which dismisses the SetupScreen.
  useEffect(() => {
    if (!setup?.setup_required || !setup.editor_valid || setup.assets_ready) return;
    if (bootstrapRunningRef.current) return;
    bootstrapRunningRef.current = true;
    void clientRef.current?.send({ type: "bootstrap_run" }).then(() => {
      bootstrapRunningRef.current = false;
    });
  }, [setup]);

  // Once first-run setup is satisfied, check for an app self-update exactly once.
  // Non-blocking: an updater error (offline, no release yet) just leaves the banner
  // hidden — it never gates the panel.
  useEffect(() => {
    if (!setup || setup.setup_required) return;
    if (updateCheckedRef.current) return;
    updateCheckedRef.current = true;
    void updater
      .check()
      .then((found) => {
        if (found) setUpdate(found);
      })
      .catch(() => {
        /* no release / offline — no banner */
      });
  }, [setup, updater]);

  // ---- user intents ----
  // Every session invokes immediately. The backend serializes commands only
  // within that session and queues only declared project write transactions.
  const handleSend = useCallback(
    async (payload: ChatPayload) => {
      const slot = selectedSlot ?? createDraftSlot();
      setEditDraft(null);
      if (slot.store.getState().phase === "changeset_review") {
        slot.store.log("warn", "변경사항 검토를 완료한 뒤 새 요청을 보내세요.");
        return;
      }

      try {
        if (!slot.persisted) {
          const oldId = slot.id;
          const seed = payload.text.trim() || "첨부 파일 분석";
          const record = await invoke<SessionRecord>("session_create", {
            firstText: seed,
          });
          sessionsRef.current.delete(oldId);
          slot.id = record.id;
          slot.meta = record;
          slot.persisted = true;
          sessionsRef.current.set(slot.id, slot);
          const toasted = toastedLogBySessionRef.current.get(oldId) ?? 0;
          toastedLogBySessionRef.current.delete(oldId);
          toastedLogBySessionRef.current.set(slot.id, toasted);
          if (selectedSessionIdRef.current === oldId) {
            selectedSessionIdRef.current = slot.id;
            setSelectedSessionId(slot.id);
          }
          bumpSessions();
        }

        if (slot.store.getState().phase === "plan_review") {
          slot.store.log("you", payload.text, undefined, payload.attachments);
          slot.store.log("agent", "계획 수정을 요청했습니다.");
          slot.store.planFeedbackSent();
          const sent = await clientRef.current?.send({
            type: "plan_feedback",
            sessionId: slot.id,
            text: payload.text,
            attachments: payload.attachments.map((attachment) => attachment.id),
          });
          if (!sent) {
            slot.store.errorReceived("계획 수정 요청을 처리하지 못했습니다.");
          }
          return;
        }

        slot.store.log("you", payload.text, undefined, payload.attachments);
        slot.store.chatSent();
        const sent = await clientRef.current?.send({
          type: "chat",
          sessionId: slot.id,
          text: payload.text,
          attachments: payload.attachments.map((attachment) => attachment.id),
        });
        if (!sent) {
          slot.store.errorReceived("요청을 처리하지 못했습니다.");
        }
      } catch (error) {
        slot.store.errorReceived(String(error));
        slot.store.log("error", `요청을 처리하지 못했습니다: ${String(error)}`);
      }
    },
    [bumpSessions, createDraftSlot, selectedSlot],
  );

  const handleCancel = useCallback(async () => {
    const slot = selectedSlot;
    const cancellable =
      slot &&
      (slot.store.getState().phase === "thinking" ||
        slot.activity === "running_read" ||
        slot.activity === "running_write");
    if (!slot || !cancellable || messageActionBusyRef.current) {
      return;
    }
    messageActionBusyRef.current = true;
    setMessageActionBusy(true);
    try {
      const sent = await clientRef.current?.send({
        type: "cancel",
        sessionId: slot.id,
      });
      if (sent) {
        slot.store.cancelSent();
      } else {
        slot.store.errorReceived("작업을 중단하지 못했습니다.");
        slot.store.log("error", "작업을 중단하지 못했습니다.");
      }
    } finally {
      messageActionBusyRef.current = false;
      setMessageActionBusy(false);
    }
  }, [selectedSlot]);

  const handleEditMessage = useCallback(
    async (entry: LogEntry) => {
      const slot = selectedSlot;
      if (
        !slot ||
        (slot.activity !== "idle" && slot.activity !== "error") ||
        entry.kind !== "you" ||
        messageActionBusyRef.current
      ) {
        return;
      }
      messageActionBusyRef.current = true;
      setMessageActionBusy(true);
      try {
        if (slot.store.getState().phase !== "ready") return;

        const currentLog = slot.store.getState().log;
        const selected = currentLog.find(
          (candidate) => candidate.id === entry.id && candidate.kind === "you",
        );
        if (selected === undefined) return;
        const prefix = currentLog.filter((candidate) => candidate.id < entry.id);
        const rewound = await clientRef.current?.send({
          type: "conversation_rewind",
          sessionId: slot.id,
          panelLog: serializePanelLog(prefix),
        });
        if (!rewound) {
          slot.store.log("error", "메시지 수정 지점으로 대화를 되돌리지 못했습니다.");
          return;
        }

        const restored = slot.store.rewindTo(entry.id);
        if (restored !== null) {
          setEditDraft({
            text: restored.text,
            attachments: restored.attachments ?? [],
          });
        }
      } finally {
        messageActionBusyRef.current = false;
        setMessageActionBusy(false);
      }
    },
    [selectedSlot],
  );

  // Empty-conversation suggestion chip → the same chat path as the
  // InstructionBox (the chips render only in the ready phase, so this never
  // routes to plan_feedback). Guarded by canSend in case gating flipped
  // between render and click.
  const handleSuggestion = useCallback(
    (text: string) => {
      if (!store.getState().canSend) return;
      void handleSend({ text, attachments: [] });
    },
    [store, handleSend],
  );

  const handleNewSession = useCallback(() => {
    createDraftSlot();
    setEditDraft(null);
  }, [createDraftSlot]);

  const handleSessionSelect = useCallback(
    (id: string) => {
      const slot = sessionsRef.current.get(id);
      if (!slot) return;
      setSelectedSessionId(id);
      selectedSessionIdRef.current = id;
      setEditDraft(null);
    },
    [],
  );

  const handleSessionRename = useCallback(
    (id: string, name: string) => {
      const slot = sessionsRef.current.get(id);
      if (!slot) return;
      const previous = slot.meta.name;
      slot.meta = { ...slot.meta, name };
      bumpSessions();
      if (!slot.persisted) return;
      void invoke<void>("session_rename", { id, name }).catch((error) => {
        slot.meta = { ...slot.meta, name: previous };
        slot.store.log("error", `이름 변경에 실패했습니다: ${String(error)}`);
        bumpSessions();
      });
    },
    [bumpSessions],
  );

  const handleSessionDelete = useCallback(
    (id: string) => {
      const slot = sessionsRef.current.get(id);
      if (
        !slot ||
        slot.activity === "running_read" ||
        slot.activity === "running_write" ||
        slot.activity === "review"
      )
        return;
      const remove = () => {
        slot.unsubscribe?.();
        if (slot.saveTimer !== undefined) window.clearTimeout(slot.saveTimer);
        sessionsRef.current.delete(id);
        if (selectedSessionIdRef.current === id) {
          const next = sessionsRef.current.values().next().value as
            | SessionSlot
            | undefined;
          if (next) {
            setSelectedSessionId(next.id);
            selectedSessionIdRef.current = next.id;
          } else {
            createDraftSlot();
          }
        }
        bumpSessions();
      };
      if (!slot.persisted) {
        remove();
        return;
      }
      void invoke<void>("session_delete", { id })
        .then(remove)
        .catch((error) => {
          slot.store.log("warn", `대화 삭제에 실패했습니다: ${String(error)}`);
        });
    },
    [bumpSessions, createDraftSlot],
  );


  // Retry re-runs the backend download command (it re-fetches the release
  // manifest and skips already-verified assets), replacing the old full-reload
  // fallback from before bootstrap_run existed.
  const handleBootstrapRetry = useCallback(() => {
    if (bootstrapRunningRef.current) return;
    setBootstrap((prev) => ({
      ...prev,
      error: null,
      view: bootstrapView(null, undefined),
    }));
    bootstrapRunningRef.current = true;
    void clientRef.current?.send({ type: "bootstrap_run" }).then(() => {
      bootstrapRunningRef.current = false;
    });
  }, []);

  const handlePickEditorPath = useCallback(() => {
    void clientRef.current?.send({ type: "setup_pick_editor_path" });
  }, []);

  // Re-query the setup gate after a login attempt so codex_authed (and thus
  // setup_required) refreshes and the SetupScreen dismisses on success.
  const refreshSetup = useCallback(() => {
    void clientRef.current?.send({ type: "setup_status" });
  }, []);

  const stopCodexPoll = useCallback(() => {
    if (codexPollRef.current !== null) {
      window.clearInterval(codexPollRef.current);
      codexPollRef.current = null;
    }
  }, []);

  // Install: the backend downloads the standalone codex binary and places it
  // where resolve_codex_cmd finds it; refreshing the gate flips codex_resolved
  // and the login controls take over.
  const handleCodexInstall = useCallback(() => {
    stopCodexPoll();
    setCodexError(null);
    setCodexBusy(true);
    void invoke<CodexAuthState>("codex_install")
      .then((state) => {
        setCodexBusy(false);
        refreshSetup();
        if (!state.resolved) {
          setCodexError("codex 설치 후에도 실행 파일을 찾지 못했습니다.");
        }
      })
      .catch((error) => {
        setCodexBusy(false);
        setCodexError(String(error));
      });
  }, [refreshSetup, stopCodexPoll]);

  // OAuth: the backend launches `codex login` (opens the browser); we poll
  // codex_login_status until it reports authed, then refresh the gate.
  const handleCodexOAuth = useCallback(() => {
    stopCodexPoll();
    setCodexError(null);
    setCodexBusy(true);
    void invoke("codex_login_start")
      .then(() => {
        const startedAt = Date.now();
        codexPollRef.current = window.setInterval(() => {
          void invoke<CodexAuthState>("codex_login_status")
            .then((state) => {
              if (state.authed) {
                stopCodexPoll();
                setCodexBusy(false);
                refreshSetup();
              } else if (Date.now() - startedAt > CODEX_POLL_TIMEOUT_MS) {
                stopCodexPoll();
                setCodexBusy(false);
                setCodexError(
                  "로그인이 완료되지 않았습니다. 브라우저에서 인증을 마친 뒤 다시 시도해 주세요.",
                );
              }
            })
            .catch((error) => {
              stopCodexPoll();
              setCodexBusy(false);
              setCodexError(String(error));
            });
        }, CODEX_POLL_MS);
      })
      .catch((error) => {
        setCodexBusy(false);
        setCodexError(String(error));
      });
  }, [refreshSetup, stopCodexPoll]);

  // API key: piped to the backend (stdin), awaited; success refreshes the gate.
  const handleCodexApiKey = useCallback(
    (key: string) => {
      stopCodexPoll();
      setCodexError(null);
      setCodexBusy(true);
      void invoke<CodexAuthState>("codex_login_with_api_key", { key })
        .then((state) => {
          setCodexBusy(false);
          if (state.authed) refreshSetup();
          else setCodexError(state.detail || "API 키 로그인에 실패했습니다.");
        })
        .catch((error) => {
          setCodexBusy(false);
          setCodexError(String(error));
        });
    },
    [refreshSetup, stopCodexPoll],
  );

  // Stop any in-flight OAuth poll on unmount.
  useEffect(() => stopCodexPoll, [stopCodexPoll]);

  const handlePlanOpenChange = useCallback(
    (open: boolean) => {
      const slot = selectedSlot;
      if (!slot || slot.planOpen === open) return;
      slot.planOpen = open;
      bumpSessions();
    },
    [bumpSessions, selectedSlot],
  );

  const handleChangesetOpenChange = useCallback(
    (open: boolean) => {
      const slot = selectedSlot;
      if (!slot || slot.changesetOpen === open) return;
      slot.changesetOpen = open;
      bumpSessions();
    },
    [bumpSessions, selectedSlot],
  );

  const handlePlanApprove = useCallback(async () => {
    const slot = selectedSlot;
    if (!slot || slot.store.getState().phase !== "plan_review") return;
    const rev = slot.store.getState().plan?.revision;
    slot.store.log(
      "agent",
      rev !== undefined ? `계획안(rev ${rev})을 승인했습니다.` : "계획을 승인했습니다.",
    );
    slot.store.planApproveSent();
    const sent = await clientRef.current?.send({
      type: "plan_approve",
      sessionId: slot.id,
    });
    if (!sent) {
      slot.store.errorReceived("계획 승인 요청을 처리하지 못했습니다.");
    }
  }, [selectedSlot]);

  const handleDecide = useCallback(
    async (decision: "accept" | "reject", ids: "all" | string[]) => {
      const slot = selectedSlot;
      if (!slot || slot.store.getState().phase !== "changeset_review") return;
      slot.store.decisionSent(decision, ids);
      const sent = await clientRef.current?.send({
        type: "changeset_decision",
        sessionId: slot.id,
        decision,
        ids,
      });
      if (!sent) slot.store.decisionFailed();
    },
    [selectedSlot],
  );

  const handleWorkspaceSelect = useCallback(
    async (file: WorkspaceFileEntry, data = workspaceData) => {
      if (!data) return;
      setWorkspacePath(file.path);
      setWorkspaceContent(null);
      setWorkspaceError(null);
      setWorkspaceLoading(true);
      try {
        const response = await workspaceRead(data.workspaceId, file.path);
        setWorkspaceContent(response.content);
      } catch (error) {
        setWorkspaceError(`파일을 열지 못했습니다: ${String(error)}`);
      } finally {
        setWorkspaceLoading(false);
      }
    },
    [workspaceData],
  );

  const handleWorkspaceRefresh = useCallback(async () => {
    setWorkspaceLoading(true);
    setWorkspaceError(null);
    try {
      const data = await workspaceList();
      setWorkspaceData(data);
      const selected =
        data.files.find((file) => file.path === workspacePath) ??
        data.files.find((file) => file.path === "specs/index.md") ??
        data.files.find((file) => !file.source && file.path.toLowerCase().endsWith(".md")) ??
        data.files[0] ??
        null;
      if (selected) {
        setWorkspacePath(selected.path);
        const response = await workspaceRead(data.workspaceId, selected.path);
        setWorkspaceContent(response.content);
      } else {
        setWorkspacePath(null);
        setWorkspaceContent(null);
      }
    } catch (error) {
      setWorkspaceError(`워크스페이스를 불러오지 못했습니다: ${String(error)}`);
    } finally {
      setWorkspaceLoading(false);
    }
  }, [workspacePath]);

  const handleProjectPanelTab = useCallback(
    async (tab: ProjectPanelTab) => {
      setProjectSidebarOpen(true);
      setProjectPanelTab(tab);
      if (tab === "workspace") {
        await handleWorkspaceRefresh();
        return;
      }
      if (tab === "memory") {
        projectStore.memoryOpened();
        await clientRef.current?.send({ type: "memory_get" });
        return;
      }
      try {
        const msg = await wikiGet();
        projectStore.wikiReceived(msg.version, msg.entries);
        for (const slot of sessionsRef.current.values()) {
          slot.store.wikiReceived(msg.version, msg.entries);
        }
      } catch (error) {
        store.log("warn", `위키를 불러오지 못했습니다: ${String(error)}`);
      }
    },
    [handleWorkspaceRefresh, projectStore, store],
  );

  const handleProjectPanelToggle = useCallback(() => {
    setProjectSidebarOpen((open) => !open);
  }, []);

  const handleWikiSave = useCallback(
    async (entries: Record<string, LedgerEntry>) => {
      try {
        const msg = await wikiSave(entries);
        projectStore.wikiReceived(msg.version, msg.entries);
        for (const slot of sessionsRef.current.values()) {
          slot.store.wikiReceived(msg.version, msg.entries);
        }
        store.log("ok", "위키를 저장했습니다.");
      } catch (error) {
        store.log("error", `위키 저장에 실패했습니다: ${String(error)}`);
      }
    },
    [projectStore, store],
  );

  useProjectIdentityEffect(
    projectState.hasProject && projectState.project ? projectState.project : null,
    (projectIdentity) => {
      setWorkspaceData(null);
      setWorkspacePath(null);
      setWorkspaceContent(null);
      setWorkspaceError(null);
      if (projectIdentity && projectPanelTab === "wiki") {
        void handleProjectPanelTab("wiki");
      }
    },
  );

  useEffect(() => {
    const media = window.matchMedia("(max-width: 1040px)");
    const adaptSessionSidebar = () => {
      setSessionSidebarCollapsed(media.matches);
    };
    adaptSessionSidebar();
    media.addEventListener("change", adaptSessionSidebar);
    return () => media.removeEventListener("change", adaptSessionSidebar);
  }, []);

  // Launch the configured EUD Editor 3 (Header button). The backend spawns the exe;
  // the existing editor-heartbeat poll flips editorConnected once the bridge is up, so
  // success needs no extra signal here. Stable error codes map to Korean (raw codes are
  // never shown). The pending flag clears on resolve/reject; if the spawn succeeds the
  // button stays disabled anyway once editorConnected turns true.
  const handleLaunchEditor = useCallback(() => {
    setLaunchPending(true);
    void invoke("launch_editor")
      .then(() => {
        setLaunchPending(false);
      })
      .catch((error) => {
        setLaunchPending(false);
        const code = String(error);
        const message =
          code === "editor path not configured"
            ? "에디터 경로가 설정되지 않았습니다. 설정에서 에디터 폴더를 먼저 지정해 주세요."
            : code === "editor executable not found"
              ? "에디터 실행 파일을 찾지 못했습니다. 에디터 폴더 경로를 확인해 주세요."
              : "에디터를 실행하지 못했습니다.";
        store.log("error", message);
      });
  }, [store]);

  const handleMemorySave = useCallback(
    async ({ file, content }: { file: MemoryFile; content: string }) => {
      const sent = await clientRef.current?.send({
        type: "memory_save",
        file,
        content,
      });
      if (sent) {
        projectStore.memorySaveSent(file);
        for (const slot of sessionsRef.current.values()) {
          slot.store.memorySaveSent(file);
        }
      }
    },
    [projectStore],
  );

  const rag = ragState === "idle" ? undefined : { state: ragState, elapsedSec: ragElapsedSec };

  const sessionRows = useMemo<SessionSidebarRow[]>(
    () =>
      Array.from(sessionsRef.current.values())
        .sort((left, right) => {
          if (left.persisted !== right.persisted) return left.persisted ? 1 : -1;
          return right.meta.updatedAt - left.meta.updatedAt;
        })
        .map((slot) => ({
          id: slot.id,
          name: slot.meta.name,
          updatedAt: slot.meta.updatedAt,
          activity: slot.activity,
          persisted: slot.persisted,
        })),
    [sessionRevision],
  );
  const selectedActionBusy =
    messageActionBusy ||
    selectedSlot?.activity === "running_write" ||
    state.phase === "changeset_review";

  if (setup?.setup_required || bootstrap.active) {
    return (
      <SetupScreen
        editorValid={setup?.editor_valid ?? true}
        pickError={setup?.error ?? null}
        onPick={handlePickEditorPath}
        view={bootstrap.view}
        error={bootstrap.error}
        onRetry={handleBootstrapRetry}
        assetsReady={setup?.assets_ready ?? false}
        codexResolved={setup?.codex_resolved ?? true}
        codexAuthed={setup?.codex_authed ?? true}
        codexBusy={codexBusy}
        codexError={codexError}
        onCodexInstall={handleCodexInstall}
        onCodexOAuth={handleCodexOAuth}
        onCodexApiKey={handleCodexApiKey}
      />
    );
  }

  return (
    <div className="flex h-screen min-w-0 overflow-hidden bg-background text-foreground">
      <Toaster position="bottom-right" richColors closeButton />
      <SessionSidebar
        project={projectState.project}
        rows={sessionRows}
        selectedId={selectedSessionId}
        collapsed={sessionSidebarCollapsed}
        onCollapsedChange={setSessionSidebarCollapsed}
        onNew={handleNewSession}
        onSelect={handleSessionSelect}
        onRename={handleSessionRename}
        onDelete={handleSessionDelete}
      />

      <main className="flex min-w-[32rem] flex-1 flex-col overflow-hidden">
        <Header
          project={projectState.project}
          connected={projectState.connected}
          phase={state.phase}
          rag={rag}
          editorConnected={projectState.editorConnected}
          hasProject={projectState.hasProject}
          launchPending={launchPending}
          onLaunchEditor={handleLaunchEditor}
          projectPanelOpen={projectSidebarOpen}
          onProjectPanelToggle={handleProjectPanelToggle}
        />

        {update && !updateDismissed && (
          <UpdateNotice
            update={update}
            relaunch={updater.relaunch}
            onLater={() => setUpdateDismissed(true)}
          />
        )}

        {!projectState.editorConnected && <ConnectionNotice />}

        {selectedSlot && (
          <div className="flex min-h-10 items-center gap-2 border-b border-border bg-card/20 px-4 text-xs">
            <span className="min-w-0 flex-1 truncate font-medium text-foreground">
              {selectedSlot.meta.name}
            </span>
            {selectedSlot.activity === "running_read" && (
              <span className="text-primary">분석 중</span>
            )}
            {selectedSlot.activity === "running_write" && (
              <span className="text-primary">변경 중 · 격리 워크스페이스</span>
            )}
            {selectedSlot.activity === "review" && (
              <span className="text-amber-400">검토 필요</span>
            )}
          </div>
        )}

        <ConversationLog
          key={selectedSessionId ?? "no-session"}
          log={state.log}
          phase={state.phase}
          turn={state.turn}
          ragLoading={state.rag === "loading"}
          onSuggestion={handleSuggestion}
          suggestionsEnabled={state.canSend && !selectedActionBusy}
          onEditMessage={handleEditMessage}
          editDisabled={
            messageActionBusy ||
            !selectedSlot ||
            (selectedSlot.activity !== "idle" &&
              selectedSlot.activity !== "error")
          }
        />

        {state.plan &&
          (state.phase === "plan_review" || state.phase === "thinking") && (
            <PlanView
              plan={state.plan}
              open={selectedSlot?.planOpen ?? true}
              onOpenChange={handlePlanOpenChange}
              pending={state.phase !== "plan_review"}
              onApprove={handlePlanApprove}
            />
          )}

        {state.changeset && state.phase === "changeset_review" && (
          <ChangesetView
            changeset={state.changeset}
            open={selectedSlot?.changesetOpen ?? true}
            onOpenChange={handleChangesetOpenChange}
            pending={state.pendingDecision !== null}
            onDecide={handleDecide}
          />
        )}

        <InstructionBox
          state={state}
          onSend={handleSend}
          onStageAttachment={stageAttachment}
          onDiscardAttachment={discardAttachment}
          onCancel={handleCancel}
          draft={editDraft}
          actionBusy={selectedActionBusy}
          codexSettings={codexSettings}
          codexSettingsBusy={!editorPollEnabled || codexSettingsBusy}
          onCodexSettingsChange={handleCodexSettingsChange}
          onCodexSettingsReload={loadCodexModelSettings}
        />
      </main>

      <ProjectSidebar
        open={projectSidebarOpen}
        project={projectState.project}
        activeTab={projectPanelTab}
        wiki={projectState.wikiData ?? { version: 1, entries: {} }}
        memory={projectState.memory}
        workspace={workspaceData}
        workspacePath={workspacePath}
        workspaceContent={workspaceContent}
        workspaceLoading={workspaceLoading}
        workspaceError={workspaceError}
        onTabChange={(tab) => void handleProjectPanelTab(tab)}
        onClose={() => setProjectSidebarOpen(false)}
        onWikiSave={handleWikiSave}
        onMemoryTabSelected={projectStore.memoryTabSelected}
        onMemoryEdited={projectStore.memoryEdited}
        onMemorySave={handleMemorySave}
        onWorkspaceSelect={handleWorkspaceSelect}
        onWorkspaceRefresh={handleWorkspaceRefresh}
      />
    </div>
  );
}
