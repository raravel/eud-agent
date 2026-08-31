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
  startTransition,
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
import { SettingsDialog } from "@/components/SettingsDialog";
import { ConversationLog } from "@/components/ConversationLog";
import { ChangesetView } from "@/components/ChangesetView";
import { HarnessStatusCard } from "@/components/HarnessStatusCard";
import { AskCard } from "@/components/AskCard";
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
  appSettingsGet,
  appSettingsSave,
  attentionNotify,
  compactSession,
  isAgentTurnEndTransition,
  mentionSearch,
  notificationSoundPreview,
  providerApiKeySave,
  providerBaseUrlSave,
  providerSettingsGet,
  providerCredentialImport,
  providerDefaultsSave,
  providerInstall,
  providerLoginStart,
  providerLoginCancel,
  providerLoginStatus,
  providerLogout,
  providerStatusList,
  sessionModelSettingsGet,
  sessionModelSettingsSave,
  setupProviderSelect,
  wikiGet,
  wikiSave,
  workspaceList,
  workspaceRead,
  workspaceSearch,
  type AskAnswer,
  type AppSettings,
  type HarnessJobView,
  type LedgerEntry,
  type MemoryFile,
  type MentionSearchRequest,
  type PanelLog,
  type PanelLogEntry,
  type ProviderId,
  type ProviderModel,
  type ProviderProgressEvent,
  type ProviderStatus,
  type ReasoningSelection,
  type ServerMessage,
  type SessionMeta,
  type SessionModelSettings,
  type SessionRecord,
  type SetupMessage,
  type WorkspaceFileEntry,
  type WorkspaceListResponse,
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
import { PROVIDER_LABELS } from "@/providers/providerCopy";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  discardAttachment,
  stageAttachment,
} from "@/lib/attachments";

const PROVIDER_POLL_MS = 2000;
const PROVIDER_POLL_TIMEOUT_MS = 300000;

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
 * Serialize the live conversation log into the durable {@link PanelLog} subset.
 * Turn progress is transient; durable rows keep `id/kind/text` plus optional
 * `stage`/`tools`/`attachments`/`mentions`. `logSeq` advances restored store
 * counters past every live id, including omitted progress rows.
 */
function serializePanelLog(log: readonly LogEntry[]): PanelLog {
  const entries: PanelLogEntry[] = [];
  for (const entry of log) {
    if (entry.kind === "progress") continue;
    const next: PanelLogEntry = {
      id: entry.id,
      kind: entry.kind,
      text: entry.text,
    };
    if (entry.clientTurnId) next.clientTurnId = entry.clientTurnId;
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
    if (entry.attachments) {
      next.attachments = entry.attachments.map((attachment) => ({
        id: attachment.id,
        name: attachment.name,
        mime: attachment.mime,
        kind: attachment.kind,
        size: attachment.size,
        ...(attachment.kind === "image" &&
        attachment.previewUrl?.startsWith("data:image/") === true
          ? { previewUrl: attachment.previewUrl }
          : {}),
      }));
    }
    if (entry.mentions) {
      next.mentions = entry.mentions.map((mention) => ({ ...mention }));
    }
    entries.push(next);
  }
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
  observedLog: readonly LogEntry[];
  planOpen: boolean;
  changesetOpen: boolean;
  harnessJobs: HarnessJobView[];
}

type PendingAskSnapshot = {
  sessionId: string;
  requestId: string;
  questions: Parameters<PanelStore["askReceived"]>[1];
};


let draftSequence = 0;

function emptyPanelLog(): PanelLog {
  return { schemaVersion: PANEL_LOG_SCHEMA_VERSION, logSeq: 0, log: [] };
}

function draftSession(
  project: string,
  provider: ProviderId,
  model: string,
): SessionRecord {
  draftSequence += 1;
  const now = Date.now();
  return {
    id: `draft-${draftSequence}`,
    name: "새 대화",
    project,
    kind: "eps",
    provider,
    model,
    createdAt: Math.floor(now / 1_000),
    lastConversationAt: now,
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
  const [providerStatuses, setProviderStatuses] = useState<ProviderStatus[]>([]);
  const [providerModels, setProviderModels] = useState<
    Partial<Record<ProviderId, ProviderModel[]>>
  >({});
  const [providerSelectedModels, setProviderSelectedModels] = useState<
    Partial<Record<ProviderId, string>>
  >({});
  const [providerSelectedReasoning, setProviderSelectedReasoning] = useState<
    Partial<Record<ProviderId, ReasoningSelection>>
  >({});
  const [providerVersions, setProviderVersions] = useState<
    Partial<Record<ProviderId, string>>
  >({});
  const [providerChannels, setProviderChannels] = useState<
    Partial<Record<ProviderId, string>>
  >({});
  const [providerBaseUrls, setProviderBaseUrls] = useState<
    Partial<Record<ProviderId, string>>
  >({});
  const [providerHasApiKeys, setProviderHasApiKeys] = useState<
    Partial<Record<ProviderId, boolean>>
  >({});
  const providerAttemptsRef = useRef<
    Partial<Record<ProviderId, string>>
  >({});
  const [providerLoginPending, setProviderLoginPending] = useState<
    Partial<Record<ProviderId, boolean>>
  >({});
  const [providerBusy, setProviderBusy] = useState<ProviderId | undefined>();
  const [providerErrors, setProviderErrors] = useState<
    Partial<Record<ProviderId, string>>
  >({});
  const providerPollsRef = useRef(new Map<ProviderId, number>());
  const draftProviderRef = useRef<{
    provider: ProviderId;
    model: string;
  }>({ provider: "codex", model: "pending" });
  const previewProvider =
    providerStatuses.find((status) => status.selectedAsDefault)?.provider ??
    setup?.defaultProvider ??
    "codex";
  draftProviderRef.current = {
    provider: previewProvider,
    model:
      providerSelectedModels[previewProvider] ??
      providerModels[previewProvider]?.find((candidate) => candidate.isDefault)
        ?.model ??
      "pending",
  };
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
  const [sessionModelSettings, setSessionModelSettings] =
    useState<SessionModelSettings | null>(null);
  const [providerSettingsBusy, setProviderSettingsBusy] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [appSettings, setAppSettings] = useState<AppSettings | null>(null);
  const [appSettingsBusy, setAppSettingsBusy] = useState(false);
  // Message undo/edit flow: the core must finish cancellation/rewind before the
  // input unlocks. `editDraft` is applied by InstructionBox without controlling
  // subsequent typing.
  const [editDraft, setEditDraft] = useState<ChatPayload | null>(null);
  const [messageActionBusy, setMessageActionBusy] = useState(false);
  const [harnessActionJobId, setHarnessActionJobId] = useState<string | null>(null);
  const messageActionBusyRef = useRef(false);

  const bumpSessions = useCallback(() => {
    setSessionRevision((revision) => revision + 1);
  }, []);

  const markConversationStarted = useCallback(
    (slot: SessionSlot) => {
      const newest = Array.from(sessionsRef.current.values()).reduce(
        (latest, candidate) =>
          Math.max(latest, candidate.meta.lastConversationAt),
        0,
      );
      slot.meta = {
        ...slot.meta,
        lastConversationAt: Math.max(Date.now(), newest + 1),
      };
      bumpSessions();
    },
    [bumpSessions],
  );

  const attachSlot = useCallback(
    (slot: SessionSlot) => {
      if (slot.unsubscribe) return;
      slot.unsubscribe = slot.store.subscribe((snapshot) => {
        bumpSessions();
        if (snapshot.log === slot.observedLog) return;
        slot.observedLog = snapshot.log;
        if (!slot.persisted) return;
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

  const syncHarnessJobs = useCallback(
    async (slot: SessionSlot) => {
      try {
        const jobs = await invoke<HarnessJobView[]>("harness_jobs", {
          sessionId: slot.id,
        });
        if (!Array.isArray(jobs)) return;
        const merged = new Map(slot.harnessJobs.map((job) => [job.id, job]));
        for (const job of jobs) {
          const current = merged.get(job.id);
          if (!current || job.updatedAt >= current.updatedAt) merged.set(job.id, job);
        }
        slot.harnessJobs = [...merged.values()];
        bumpSessions();
      } catch {
        // Push events remain authoritative; a failed snapshot does not erase them.
      }
    },
    [bumpSessions],
  );

  const registerSession = useCallback(
    (record: SessionRecord): SessionSlot => {
      const existing = sessionsRef.current.get(record.id);
      if (existing) {
        existing.meta = record;
        if (record.contextUsage !== undefined) {
          existing.store.contextUsageReceived(record.contextUsage);
        }
        existing.persisted = true;
        bumpSessions();
        void syncHarnessJobs(existing);
        return existing;
      }
      const sessionStore = createPanelStore();
      syncProjectState(sessionStore, projectStore);
      sessionStore.hydrate(record.panelLog ?? emptyPanelLog());
      if (record.contextUsage !== undefined) {
        sessionStore.contextUsageReceived(record.contextUsage);
      }
      const slot: SessionSlot = {
        id: record.id,
        meta: record,
        store: sessionStore,
        persisted: true,
        observedLog: sessionStore.getState().log,
        activity: record.pendingRequestIds.length > 0 ? "review" : "idle",
        planOpen: true,
        changesetOpen: true,
        harnessJobs: [],
      };
      sessionsRef.current.set(slot.id, slot);
      attachSlot(slot);
      void syncHarnessJobs(slot);
      toastedLogBySessionRef.current.set(
        slot.id,
        record.panelLog?.logSeq ?? 0,
      );
      bumpSessions();
      return slot;
    },
    [attachSlot, bumpSessions, projectStore, syncHarnessJobs],
  );

  const syncPendingAsk = useCallback(
    async (slot: SessionSlot) => {
      const observedRequestId = slot.store.getState().ask?.requestId;
      try {
        const pending = await invoke<PendingAskSnapshot | null>("ask_pending", {
          sessionId: slot.id,
        });
        if (pending !== null && pending.sessionId === slot.id) {
          slot.activity = "waiting_input";
          slot.store.askReceived(pending.requestId, pending.questions);
          bumpSessions();
          return;
        }
        if (
          observedRequestId !== undefined &&
          slot.store.getState().ask?.requestId === observedRequestId
        ) {
          slot.store.askAnswered();
        }
      } catch {
        // Push events remain the fast path; a failed snapshot must not erase one.
      }
    },
    [bumpSessions],
  );

  const createDraftSlot = useCallback((): SessionSlot => {
    const { provider, model } = draftProviderRef.current;
    const meta = draftSession(projectStore.getState().project, provider, model);
    const sessionStore = createPanelStore();
    syncProjectState(sessionStore, projectStore);
    const slot: SessionSlot = {
      id: meta.id,
      meta,
      store: sessionStore,
      persisted: false,
      observedLog: sessionStore.getState().log,
      activity: "idle",
      planOpen: true,
      changesetOpen: true,
      harnessJobs: [],
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

  const loadProviderCatalog = useCallback(async (provider: ProviderId) => {
    const view = await providerSettingsGet(provider);
    setProviderStatuses((current) =>
      current.map((status) =>
        status.provider === provider ? view.status : status,
      ),
    );
    setProviderModels((current) => ({
      ...current,
      [provider]: view.models,
    }));
    setProviderSelectedModels((current) => ({
      ...current,
      [provider]: view.selectedModel ?? undefined,
    }));
    setProviderSelectedReasoning((current) => ({
      ...current,
      [provider]: view.selectedReasoning ?? undefined,
    }));
    setProviderVersions((current) => ({
      ...current,
      [provider]: view.version ?? undefined,
    }));
    setProviderChannels((current) => ({
      ...current,
      [provider]: view.channel ?? undefined,
    }));
    setProviderBaseUrls((current) => ({
      ...current,
      [provider]: view.baseUrl ?? undefined,
    }));
    setProviderHasApiKeys((current) => ({
      ...current,
      [provider]: view.hasApiKey,
    }));
    return view.models;
  }, []);
  const loadProviders = useCallback(async () => {
    setProviderSettingsBusy(true);
    try {
      const statuses = await providerStatusList();
      setProviderStatuses(statuses);
      await Promise.all(
        statuses
          .filter(
            (status) =>
              status.availability === "ready" || status.provider === "ollama",
          )
          .map((status) => loadProviderCatalog(status.provider)),
      );
    } catch {
      toast.error("AI 제공자 상태를 불러오지 못했습니다.");
    } finally {
      setProviderSettingsBusy(false);
    }
  }, [loadProviderCatalog]);

  useEffect(() => {
    if (editorPollEnabled) void loadProviders();
  }, [editorPollEnabled, loadProviders]);

  useEffect(() => {
    if (!selectedSlot?.persisted) {
      setSessionModelSettings(null);
      return;
    }
    let active = true;
    setProviderSettingsBusy(true);
    void sessionModelSettingsGet(selectedSlot.id)
      .then((settings) => {
        if (active) setSessionModelSettings(settings);
      })
      .catch(() => {
        if (active) setSessionModelSettings(null);
      })
      .finally(() => {
        if (active) setProviderSettingsBusy(false);
      });
    return () => {
      active = false;
    };
  }, [selectedSlot?.id, selectedSlot?.persisted]);

  const handleSessionModelChange = useCallback(
    async (model: string, reasoning: ReasoningSelection | undefined) => {
      setProviderSettingsBusy(true);
      try {
        if (selectedSlot?.persisted) {
          setSessionModelSettings(
            await sessionModelSettingsSave(selectedSlot.id, model, reasoning),
          );
        } else {
          const provider = providerStatuses.find(
            (status) => status.selectedAsDefault,
          )?.provider;
          if (!provider) return;
          const view = await providerDefaultsSave(
            provider,
            model,
            reasoning,
            true,
          );
          setProviderModels((current) => ({
            ...current,
            [provider]: view.models,
          }));
          setProviderSelectedModels((current) => ({
            ...current,
            [provider]: model,
          }));
          setProviderSelectedReasoning((current) => ({
            ...current,
            [provider]: reasoning,
          }));
        }
      } catch {
        toast.error("모델 설정을 저장하지 못했습니다.");
      } finally {
        setProviderSettingsBusy(false);
      }
    },
    [providerStatuses, selectedSlot],
  );

  const loadAppSettings = useCallback(async () => {
    setAppSettingsBusy(true);
    try {
      setAppSettings(await appSettingsGet());
    } catch {
      setAppSettings(null);
      toast.error("앱 설정을 불러오지 못했습니다.");
    } finally {
      setAppSettingsBusy(false);
    }
  }, []);

  useEffect(() => {
    void loadAppSettings();
  }, [loadAppSettings]);

  const handleAppSettingsChange = useCallback(
    async (next: AppSettings) => {
      const previous = appSettings;
      setAppSettings(next);
      setAppSettingsBusy(true);
      try {
        setAppSettings(await appSettingsSave(next));
      } catch {
        setAppSettings(previous);
        toast.error("앱 설정을 저장하지 못했습니다.");
      } finally {
        setAppSettingsBusy(false);
      }
    },
    [appSettings],
  );

  const handleNotificationSoundPreview = useCallback(async () => {
    try {
      await notificationSoundPreview();
    } catch {
      toast.error("알림음을 재생하지 못했습니다.");
    }
  }, []);

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
          setProviderStatuses(msg.providers);
          if (!msg.setupRequired) {
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
        case "agent_event": {
          const target = sessionStore();
          if (target) {
            startTransition(() =>
              target.agentEvent(msg.kind, msg.detail, msg.data),
            );
          }
          break;
        }
        case "context_usage":
          sessionStore()?.contextUsageReceived(msg.tokenUsage);
          break;
        case "answer":
          sessionStore()?.answerReceived(msg.text);
          break;
        case "ask": {
          const targetSlot = sessionsRef.current.get(msg.sessionId);
          if (!targetSlot) break;
          const priorRequestId = targetSlot.store.getState().ask?.requestId;
          if (priorRequestId !== msg.requestId) {
            void attentionNotify(
              "askResponseRequired",
              !document.hasFocus(),
              targetSlot.id,
            ).catch(() => {
              // Delivery is best-effort and must not disturb the pending ASK.
            });
          }
          targetSlot.store.askReceived(msg.requestId, msg.questions);
          break;
        }
        case "plan": {
          const targetSlot = scopedId
            ? sessionsRef.current.get(scopedId)
            : undefined;
          if (!targetSlot) break;
          const target = targetSlot.store;
          const prior = target.getState().plan;
          if (prior === null || prior.revision !== msg.revision) {
            targetSlot.planOpen = true;
            void attentionNotify(
              "planApproval",
              !document.hasFocus(),
              targetSlot.id,
            ).catch(() => {
              // Delivery is best-effort and must not disturb review state.
            });
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
            void attentionNotify(
              "changesetReview",
              !document.hasFocus(),
              targetSlot.id,
              msg.items.length,
            ).catch(() => {
              // Delivery is best-effort and must not disturb review state.
            });
          }
          target.changesetReceived(msg.request_id, msg.items);
          target.log("agent", `변경사항 ${msg.items.length}건을 검토하세요.`);
          break;
        }
        case "harness_job": {
          const slot = sessionsRef.current.get(msg.sessionId);
          if (!slot) break;
          const previous = slot.harnessJobs.find((job) => job.id === msg.id);
          slot.harnessJobs = [
            ...slot.harnessJobs.filter((job) => job.id !== msg.id),
            msg,
          ];
          if (previous?.status !== msg.status) {
            if (msg.status === "waiting_runtime") {
              slot.store.log("warn", "인게임 검증 후 하네스 동기화를 계속합니다.");
            } else if (msg.status === "review") {
              slot.store.log("agent", "하네스 문서 변경사항을 검토하세요.");
              void attentionNotify(
                "changesetReview",
                !document.hasFocus(),
                slot.id,
                msg.changeset?.items.length,
              ).catch(() => {
                // Attention delivery is best-effort.
              });
            } else if (msg.status === "failed") {
              slot.store.log("warn", msg.error ?? "하네스 동기화에 실패했습니다.");
            } else if (msg.status === "completed") {
              slot.store.log("ok", "하네스 문서 동기화 완료");
            }
          }
          bumpSessions();
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
          if (isAgentTurnEndTransition(previous, msg.activity)) {
            void attentionNotify(
              "agentTurnComplete",
              !document.hasFocus(),
              slot.id,
            ).catch(() => {
              // Delivery is best-effort and must not disturb settled turn state.
            });
          }
          bumpSessions();
          break;
        }
        default:
          break;
      }
    },
    [bumpSessions, projectStore],
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
          if (open) {
            slot.store.wsOpen();
            void syncPendingAsk(slot);
          } else {
            slot.store.wsError();
          }
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
  }, [onMessage, projectStore, syncPendingAsk]);

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
              void syncPendingAsk(slot);
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
        for (const slot of slots) void syncPendingAsk(slot);
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
    syncPendingAsk,
  ]);

  // Setup flow, download step: once the editor folder is picked (or was already
  // configured) and assets are still missing, start the bootstrap download.
  // Progress streams in as `progress {stage: "bootstrap"}`; the final "done"
  // re-queries setup_status, which dismisses the SetupScreen.
  useEffect(() => {
    if (!setup?.setupRequired || !setup.editorValid || setup.assetsReady) return;
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
    if (!setup || setup.setupRequired) return;
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
  const handleMentionSearch = useCallback(
    (request: MentionSearchRequest) => mentionSearch(request),
    [],
  );


  // ---- user intents ----
  // Every session invokes immediately. The backend serializes commands only
  // within that session and queues only declared project write transactions.
  const handleSend = useCallback(
    async (payload: ChatPayload) => {
      const compactRequested =
        payload.text.trim() === "/compact" &&
        payload.attachments.length === 0 &&
        payload.mentions.length === 0;
      if (compactRequested) {
        setEditDraft(null);
        const slot = selectedSlot;
        if (!slot?.persisted) {
          toast.error("압축할 대화가 없습니다. 먼저 메시지를 보내 주세요.");
          return;
        }
        const snapshot = slot.store.getState();
        if (
          messageActionBusyRef.current ||
          snapshot.phase === "thinking" ||
          snapshot.phase === "changeset_review" ||
          (slot.activity !== "idle" &&
            slot.activity !== "error" &&
            slot.activity !== "review")
        ) {
          slot.store.log("warn", "현재 작업이 끝난 뒤 대화를 압축해 주세요.");
          return;
        }

        messageActionBusyRef.current = true;
        setMessageActionBusy(true);
        slot.store.log("info", "대화 컨텍스트 압축 중…");
        try {
          await compactSession(slot.id);
          slot.store.log("ok", "대화 컨텍스트를 압축했습니다.");
        } catch (error) {
          slot.store.log(
            "error",
            `대화 컨텍스트를 압축하지 못했습니다: ${String(error)}`,
          );
        } finally {
          messageActionBusyRef.current = false;
          setMessageActionBusy(false);
        }
        return;
      }

      const slot = selectedSlot ?? createDraftSlot();
      setEditDraft(null);
      if (slot.store.getState().phase === "changeset_review") {
        slot.store.log("warn", "변경사항 검토를 완료한 뒤 새 요청을 보내세요.");
        return;
      }
      const clientTurnId = payload.clientTurnId ?? crypto.randomUUID();

      try {
        if (!slot.persisted) {
          const oldId = slot.id;
          const seed =
            payload.text.trim() ||
            (payload.mentions.length > 0 ? "리소스 멘션 요청" : "첨부 파일 분석");
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

        markConversationStarted(slot);

        if (slot.store.getState().phase === "plan_review") {
          slot.store.log(
            "you",
            payload.text,
            undefined,
            payload.attachments,
            payload.mentions,
            clientTurnId,
          );
          slot.store.log("agent", "계획 수정을 요청했습니다.");
          slot.store.planFeedbackSent();
          const sent = await clientRef.current?.send({
            type: "plan_feedback",
            sessionId: slot.id,
            clientTurnId,
            text: payload.text,
            attachments: payload.attachments.map((attachment) => attachment.id),
            mentions: payload.mentions,
          });
          if (!sent) {
            slot.store.errorReceived("계획 수정 요청을 처리하지 못했습니다.");
            setEditDraft({
              text: payload.text,
              attachments: [...payload.attachments],
              mentions: payload.mentions.map((mention) => ({ ...mention })),
              clientTurnId,
            });
          }
          return;
        }

        slot.store.log(
          "you",
          payload.text,
          undefined,
          payload.attachments,
          payload.mentions,
          clientTurnId,
        );
        slot.store.chatSent();
        const sent = await clientRef.current?.send({
          type: "chat",
          sessionId: slot.id,
          clientTurnId,
          text: payload.text,
          attachments: payload.attachments.map((attachment) => attachment.id),
          mentions: payload.mentions,
        });
        if (!sent) {
          slot.store.errorReceived("요청을 처리하지 못했습니다.");
          setEditDraft({
            text: payload.text,
            attachments: [...payload.attachments],
            mentions: payload.mentions.map((mention) => ({ ...mention })),
            clientTurnId,
          });
        }
      } catch (error) {
        setEditDraft({
          text: payload.text,
          attachments: [...payload.attachments],
          mentions: payload.mentions.map((mention) => ({ ...mention })),
          clientTurnId,
        });
        slot.store.errorReceived(String(error));
        slot.store.log("error", `요청을 처리하지 못했습니다: ${String(error)}`);
      }
    },
    [bumpSessions, createDraftSlot, markConversationStarted, selectedSlot],
  );

  const handleCancel = useCallback(async () => {
    const slot = selectedSlot;
    const cancellable =
      slot &&
      (slot.store.getState().phase === "thinking" ||
        slot.activity === "running_read" ||
        slot.activity === "waiting_input" ||
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
            mentions: restored.mentions ?? [],
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
      void handleSend({ text, attachments: [], mentions: [] });
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

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void listen<unknown>("notification_activated", ({ payload }) => {
      const sessionId =
        typeof payload === "object" &&
        payload !== null &&
        "sessionId" in payload &&
        typeof payload.sessionId === "string"
          ? payload.sessionId
          : null;
      if (sessionId) handleSessionSelect(sessionId);
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [handleSessionSelect]);

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
        slot.activity === "waiting_input" ||
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

  const refreshSetup = useCallback(() => {
    void clientRef.current?.send({ type: "setup_status" });
  }, []);

  const stopProviderPoll = useCallback((provider: ProviderId) => {
    const timer = providerPollsRef.current.get(provider);
    if (timer !== undefined) {
      window.clearInterval(timer);
      providerPollsRef.current.delete(provider);
    }
  }, []);
  const clearProviderLogin = useCallback(
    (provider: ProviderId) => {
      stopProviderPoll(provider);
      delete providerAttemptsRef.current[provider];
      setProviderLoginPending((current) => {
        if (current[provider] === undefined) return current;
        const next = { ...current };
        delete next[provider];
        return next;
      });
      setProviderBusy((current) => (current === provider ? undefined : current));
    },
    [stopProviderPoll],
  );


  const setProviderError = useCallback(
    (provider: ProviderId, error: unknown) => {
      setProviderErrors((current) => ({
        ...current,
        [provider]: String(error),
      }));
    },
    [],
  );

  const updateProviderStatus = useCallback((status: ProviderStatus) => {
    setProviderStatuses((current) =>
      current.map((candidate) =>
        candidate.provider === status.provider ? status : candidate,
      ),
    );
  }, []);

  const handleProviderRefresh = useCallback(
    async (provider: ProviderId) => {
      setProviderBusy(provider);
      setProviderErrors((current) => ({ ...current, [provider]: undefined }));
      try {
        const status = await providerLoginStatus(provider);
        updateProviderStatus(status);
        if (status.availability === "ready") {
          await loadProviderCatalog(provider);
        }
        refreshSetup();
      } catch (error) {
        setProviderError(provider, error);
      } finally {
        setProviderBusy(undefined);
      }
    },
    [
      loadProviderCatalog,
      refreshSetup,
      setProviderError,
      updateProviderStatus,
    ],
  );

  const handleProviderInstall = useCallback(
    async (provider: ProviderId) => {
      stopProviderPoll(provider);
      setProviderBusy(provider);
      setProviderErrors((current) => ({ ...current, [provider]: undefined }));
      try {
        updateProviderStatus(await providerInstall(provider));
        refreshSetup();
      } catch (error) {
        setProviderError(provider, error);
      } finally {
        setProviderBusy(undefined);
      }
    },
    [refreshSetup, setProviderError, stopProviderPoll, updateProviderStatus],
  );

  const handleProviderLogin = useCallback(
    async (provider: ProviderId) => {
      clearProviderLogin(provider);
      setProviderBusy(provider);
      setProviderErrors((current) => ({ ...current, [provider]: undefined }));
      try {
        const attemptId = await providerLoginStart(provider);
        providerAttemptsRef.current[provider] = attemptId;
        setProviderLoginPending((current) => ({ ...current, [provider]: true }));
        const startedAt = Date.now();
        const timer = window.setInterval(() => {
          void providerLoginStatus(provider)
            .then(async (status) => {
              updateProviderStatus(status);
              if (status.availability === "ready") {
                clearProviderLogin(provider);
                await loadProviderCatalog(provider);
                refreshSetup();
              } else if (
                Date.now() - startedAt >
                PROVIDER_POLL_TIMEOUT_MS
              ) {
                clearProviderLogin(provider);
                setProviderError(provider, "provider_cancelled");
              }
            })
            .catch((error) => {
              clearProviderLogin(provider);
              setProviderError(provider, error);
            });
        }, PROVIDER_POLL_MS);
        providerPollsRef.current.set(provider, timer);
      } catch (error) {
        clearProviderLogin(provider);
        setProviderError(provider, error);
      }
    },
    [
      clearProviderLogin,
      loadProviderCatalog,
      refreshSetup,
      setProviderError,
      updateProviderStatus,
    ],
  );
  const handleProviderLoginCancel = useCallback(
    async (provider: ProviderId) => {
      const attemptId = providerAttemptsRef.current[provider];
      let failure: unknown;
      if (attemptId !== undefined) {
        try {
          await providerLoginCancel(provider, attemptId);
        } catch (error) {
          if (String(error) !== "provider_cancelled") failure = error;
        }
      }
      clearProviderLogin(provider);
      setProviderError(provider, failure ?? "provider_cancelled");
      try {
        updateProviderStatus(await providerLoginStatus(provider));
      } catch {
        // Cancellation already completed locally; a status refresh remains optional.
      }
      refreshSetup();
    },
    [clearProviderLogin, refreshSetup, setProviderError, updateProviderStatus],
  );


  const handleProviderImport = useCallback(
    async (provider: ProviderId) => {
      setProviderBusy(provider);
      try {
        const status = await providerCredentialImport(provider);
        updateProviderStatus(status);
        if (status.availability === "ready") {
          await loadProviderCatalog(provider);
        }
        refreshSetup();
      } catch (error) {
        setProviderError(provider, error);
      } finally {
        setProviderBusy(undefined);
      }
    },
    [
      loadProviderCatalog,
      refreshSetup,
      setProviderError,
      updateProviderStatus,
    ],
  );

  const handleProviderApiKey = useCallback(
    async (provider: ProviderId, key: string) => {
      setProviderBusy(provider);
      try {
        const status = await providerApiKeySave(provider, key);
        updateProviderStatus(status);
        if (status.availability === "ready") {
          await loadProviderCatalog(provider);
        }
        refreshSetup();
      } catch (error) {
        setProviderError(provider, error);
      } finally {
        setProviderBusy(undefined);
      }
    },
    [
      loadProviderCatalog,
      refreshSetup,
      setProviderError,
      updateProviderStatus,
    ],
  );

  const handleProviderBaseUrl = useCallback(
    async (provider: ProviderId, baseUrl: string) => {
      setProviderBusy(provider);
      setProviderErrors((current) => ({ ...current, [provider]: undefined }));
      try {
        const view = await providerBaseUrlSave(provider, baseUrl);
        updateProviderStatus(view.status);
        setProviderModels((current) => ({
          ...current,
          [provider]: view.models,
        }));
        setProviderSelectedModels((current) => ({
          ...current,
          [provider]: view.selectedModel ?? undefined,
        }));
        setProviderSelectedReasoning((current) => ({
          ...current,
          [provider]: view.selectedReasoning ?? undefined,
        }));
        setProviderBaseUrls((current) => ({
          ...current,
          [provider]: view.baseUrl ?? undefined,
        }));
        setProviderHasApiKeys((current) => ({
          ...current,
          [provider]: view.hasApiKey,
        }));
        refreshSetup();
      } catch (error) {
        setProviderError(provider, error);
      } finally {
        setProviderBusy(undefined);
      }
    },
    [refreshSetup, setProviderError, updateProviderStatus],
  );

  const handleProviderLogout = useCallback(
    async (provider: ProviderId) => {
      setProviderBusy(provider);
      try {
        const status = await providerLogout(provider);
        updateProviderStatus(status);
        setProviderHasApiKeys((current) => ({
          ...current,
          [provider]: false,
        }));
        if (status.availability === "ready") {
          await loadProviderCatalog(provider);
        } else {
          setProviderModels((current) => ({ ...current, [provider]: [] }));
        }
        refreshSetup();
      } catch (error) {
        setProviderError(provider, error);
      } finally {
        setProviderBusy(undefined);
      }
    },
    [loadProviderCatalog, refreshSetup, setProviderError, updateProviderStatus],
  );

  const handleProviderSelect = useCallback(
    async (provider: ProviderId) => {
      try {
        await setupProviderSelect(provider);
        setProviderStatuses((current) =>
          current.map((status) => ({
            ...status,
            selectedAsDefault: status.provider === provider,
          })),
        );
        if (provider === "ollama") {
          await loadProviderCatalog(provider);
        }
        refreshSetup();
      } catch (error) {
        setProviderError(provider, error);
      }
    },
    [loadProviderCatalog, refreshSetup, setProviderError],
  );

  const handleProviderModelChange = useCallback(
    async (
      provider: ProviderId,
      model: string,
      reasoning: ReasoningSelection | undefined,
    ) => {
      setProviderBusy(provider);
      try {
        const selectedAsDefault = providerStatuses.some(
          (status) =>
            status.provider === provider && status.selectedAsDefault,
        );
        const view = await providerDefaultsSave(
          provider,
          model,
          reasoning,
          selectedAsDefault,
        );

        setProviderModels((current) => ({
          ...current,
          [provider]: view.models,
        }));
        setProviderSelectedModels((current) => ({
          ...current,
          [provider]: model,
        }));
        setProviderSelectedReasoning((current) => ({
          ...current,
          [provider]: reasoning,
        }));
        refreshSetup();
      } catch (error) {
        setProviderError(provider, error);
      } finally {
        setProviderBusy(undefined);
      }
    },
    [providerStatuses, refreshSetup, setProviderError],
  );
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void listen<ProviderProgressEvent>("provider_progress", ({ payload }) => {
      const currentAttempt = providerAttemptsRef.current[payload.provider];
      if (currentAttempt !== payload.attemptId) return;
      if (payload.detailCode) {
        clearProviderLogin(payload.provider);
        setProviderError(payload.provider, payload.detailCode);
      }
    }).then((dispose) => {
      unlisten = dispose;
    });
    return () => unlisten?.();
  }, [clearProviderLogin, setProviderError]);

  useEffect(
    () => () => {
      for (const timer of providerPollsRef.current.values()) {
        window.clearInterval(timer);
      }
      providerPollsRef.current.clear();
    },
    [],
  );

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

  const handleAskSubmit = useCallback(
    async (answers: Record<string, AskAnswer>) => {
      const slot = selectedSlot;
      const ask = slot?.store.getState().ask;
      if (!slot || !ask || ask.submitting) return;
      slot.store.askSubmitStarted();
      const sent = await clientRef.current?.send({
        type: "ask_response",
        sessionId: slot.id,
        requestId: ask.requestId,
        answers,
      });
      if (sent) {
        slot.store.askAnswered();
      } else {
        slot.store.askSubmitFailed();
      }
    },
    [selectedSlot],
  );

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

  const runHarnessAction = useCallback(
    async (jobId: string, command: string, args: Record<string, unknown> = {}) => {
      if (harnessActionJobId !== null) return;
      setHarnessActionJobId(jobId);
      try {
        await invoke(command, { jobId, ...args });
      } catch (error) {
        toast.error(`하네스 작업을 처리하지 못했습니다: ${String(error)}`);
      } finally {
        setHarnessActionJobId(null);
      }
    },
    [harnessActionJobId],
  );

  const handleHarnessRuntimeConfirm = useCallback(
    (jobId: string) => {
      void runHarnessAction(jobId, "harness_runtime_confirm");
    },
    [runHarnessAction],
  );

  const handleHarnessSkip = useCallback(
    (jobId: string) => {
      void runHarnessAction(jobId, "harness_skip");
    },
    [runHarnessAction],
  );

  const handleHarnessRetry = useCallback(
    (jobId: string) => {
      void runHarnessAction(jobId, "harness_retry");
    },
    [runHarnessAction],
  );

  const handleHarnessDismiss = useCallback(
    (jobId: string) => {
      void runHarnessAction(jobId, "harness_dismiss");
    },
    [runHarnessAction],
  );

  const handleHarnessDecision = useCallback(
    (jobId: string, decision: "accept" | "reject") => {
      void runHarnessAction(jobId, "harness_decision", { decision });
    },
    [runHarnessAction],
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

  const handleWorkspaceSearch = useCallback(
    async (query: string) => {
      if (!workspaceData) return [];
      const response = await workspaceSearch(workspaceData.workspaceId, query);
      return response.paths;
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
  const handleOpenMapAgent = useCallback(() => {
    void invoke("map_agent_open").catch(() => {
      toast.error("Map Agent 창을 열지 못했습니다.");
    });
  }, []);

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
          return (
            right.meta.lastConversationAt - left.meta.lastConversationAt ||
            right.meta.createdAt - left.meta.createdAt ||
            left.id.localeCompare(right.id)
          );
        })
        .map((slot) => ({
          id: slot.id,
          name: slot.meta.name,
          lastConversationAt: slot.meta.lastConversationAt,
          provider: slot.meta.provider,
          activity: slot.activity,
          persisted: slot.persisted,
        })),
    [sessionRevision],
  );
  const defaultProvider = providerStatuses.find(
    (status) => status.selectedAsDefault,
  )?.provider;
  const draftModels = defaultProvider ? providerModels[defaultProvider] ?? [] : [];
  const draftSelectedModel = defaultProvider
    ? providerSelectedModels[defaultProvider] ??
      draftModels.find((model) => model.isDefault)?.model
    : undefined;
  const promptModelSettings: SessionModelSettings | null =
    selectedSlot?.persisted
      ? sessionModelSettings
      : defaultProvider && draftSelectedModel
        ? {
            provider: defaultProvider,
            models: draftModels,
            selectedModel: draftSelectedModel,
            selectedReasoning: providerSelectedReasoning[defaultProvider],
          }
        : null;
  const selectedActionBusy =
    messageActionBusy ||
    selectedSlot?.activity === "running_write" ||
    state.phase === "changeset_review";

  if (setup?.setupRequired || bootstrap.active) {
    return (
      <SetupScreen
        editorValid={setup?.editorValid ?? true}
        pickError={setup?.error ?? null}
        onPick={handlePickEditorPath}
        view={bootstrap.view}
        error={bootstrap.error}
        onRetry={handleBootstrapRetry}
        assetsReady={setup?.assetsReady ?? false}
        defaultProvider={setup?.defaultProvider ?? undefined}
        providers={setup?.providers ?? providerStatuses}
        models={providerModels}
        selectedModels={providerSelectedModels}
        selectedReasoning={providerSelectedReasoning}
        versions={providerVersions}
        channels={providerChannels}
        baseUrls={providerBaseUrls}
        hasApiKeys={providerHasApiKeys}
        busyProvider={providerBusy}
        loginPending={providerLoginPending}
        providerErrors={providerErrors}
        onSelectProvider={handleProviderSelect}
        onProviderInstall={handleProviderInstall}
        onProviderLogin={handleProviderLogin}
        onProviderLoginCancel={handleProviderLoginCancel}
        onProviderImport={handleProviderImport}
        onProviderApiKey={handleProviderApiKey}
        onProviderBaseUrl={handleProviderBaseUrl}
        onProviderLogout={handleProviderLogout}
        onProviderRefresh={handleProviderRefresh}
        onProviderModelChange={handleProviderModelChange}
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
          onOpenMapAgent={handleOpenMapAgent}
          projectPanelOpen={projectSidebarOpen}
          onProjectPanelToggle={handleProjectPanelToggle}
          onSettingsOpen={() => setSettingsOpen(true)}
        />
        <SettingsDialog
          open={settingsOpen}
          settings={appSettings}
          providers={providerStatuses}
          providerModels={providerModels}
          selectedModels={providerSelectedModels}
          selectedReasoning={providerSelectedReasoning}
          versions={providerVersions}
          channels={providerChannels}
          baseUrls={providerBaseUrls}
          hasApiKeys={providerHasApiKeys}
          providerErrors={providerErrors}
          providerBusy={providerBusy}
          loginPending={providerLoginPending}
          busy={appSettingsBusy || providerSettingsBusy}
          onOpenChange={setSettingsOpen}
          onSettingsChange={handleAppSettingsChange}
          onReload={loadAppSettings}
          onSelectProvider={handleProviderSelect}
          onProviderInstall={handleProviderInstall}
          onProviderLogin={handleProviderLogin}
          onProviderLoginCancel={handleProviderLoginCancel}
          onProviderImport={handleProviderImport}
          onProviderApiKey={handleProviderApiKey}
          onProviderBaseUrl={handleProviderBaseUrl}
          onProviderLogout={handleProviderLogout}
          onProviderRefresh={handleProviderRefresh}
          onProviderModelChange={handleProviderModelChange}
          onPreviewSound={handleNotificationSoundPreview}
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
            <span className="rounded-full border border-border bg-muted/50 px-2 py-0.5 text-[11px] text-muted-foreground">
              {PROVIDER_LABELS[selectedSlot.meta.provider]}
            </span>
            {selectedSlot.activity === "running_read" && (
              <span className="text-primary">분석 중</span>
            )}
            {selectedSlot.activity === "waiting_input" && (
              <span className="text-amber-400">응답 필요</span>
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

        {state.ask && (
          <AskCard
            key={state.ask.requestId}
            requestId={state.ask.requestId}
            questions={state.ask.questions}
            submitting={state.ask.submitting}
            onSubmit={handleAskSubmit}
          />
        )}


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
        {selectedSlot && (
          <HarnessStatusCard
            jobs={selectedSlot.harnessJobs}
            pendingJobId={harnessActionJobId}
            onRuntimeConfirm={handleHarnessRuntimeConfirm}
            onSkip={handleHarnessSkip}
            onRetry={handleHarnessRetry}
            onDismiss={handleHarnessDismiss}
            onDecide={handleHarnessDecision}
          />
        )}

        <InstructionBox
          state={state}
          onSend={handleSend}
          onMentionSearch={handleMentionSearch}
          projectIdentity={projectState.project}
          scopeIdentity={selectedSlot?.id ?? `draft:${projectState.project}`}
          onStageAttachment={stageAttachment}
          onDiscardAttachment={discardAttachment}
          onCancel={handleCancel}
          draft={editDraft}
          actionBusy={selectedActionBusy}
          modelSettings={promptModelSettings}
          modelSettingsBusy={!editorPollEnabled || providerSettingsBusy}
          onModelSettingsChange={handleSessionModelChange}
          onModelSettingsReload={() => {
            if (selectedSlot?.persisted) {
              void sessionModelSettingsGet(selectedSlot.id).then(
                setSessionModelSettings,
              );
            } else if (defaultProvider) {
              void loadProviderCatalog(defaultProvider);
            }
          }}
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
        onWorkspaceSearch={handleWorkspaceSearch}
        onWorkspaceRefresh={handleWorkspaceRefresh}
      />
    </div>
  );
}
