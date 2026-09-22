/**
 * Panel app shell (v2) - wires the Tauri IPC v2 client + state store to the
 * chat-first review UI (features/06_changeset-review-panel.md).
 *
 * Components: a status-rich Header (connection transitions + RAG state/elapsed),
 * the ConversationLog cards, a live AgentStream under the turn, the ChangesetView
 * accept/reject surface, and the regated InstructionBox shared under every
 * center tab. Staged-workflow artifacts leave the conversation column: research
 * and verify reports open as workspace document tabs the moment the workflow
 * snapshot announces them, and the selected session's plan is a virtual
 * "계획 (rev N)" tab hosting the PlanView approve surface. Plan cards are
 * archived into the conversation log as agent entries when a plan arrives, is
 * superseded by a higher revision, or is approved.
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
import { SettingsDialog, type SettingsCategory } from "@/components/SettingsDialog";
import { E3sImportDialog } from "@/setup/E3sImportDialog";
import { NewMapWizard } from "@/setup/NewMapWizard";
import { ConversationLog } from "@/components/ConversationLog";
import { ChangesetView } from "@/components/ChangesetView";
import { HarnessStatusCard } from "@/components/HarnessStatusCard";
import { AskCard } from "@/components/AskCard";
import { PlanView } from "@/components/PlanView";
import { WorkflowStrip } from "@/components/WorkflowStrip";
import { InstructionBox, type ChatPayload } from "@/components/InstructionBox";
import { ConnectionNotice } from "@/components/ConnectionNotice";
import {
  DocumentTabStrip,
  type DocumentTab,
  type DocumentTabId,
} from "@/components/DocumentTabStrip";
import { WorkspaceDocument } from "@/components/WorkspaceDocument";
import {
  SessionSidebar,
  type SessionActivity,
  type SessionSidebarRow,
} from "@/components/SessionSidebar";
import {
  ProjectSidebar,
  type ProjectPanelTab,
} from "@/components/ProjectSidebar";
import { createPanelStore, isBusyPhase } from "@/state/store";
import type { LogEntry, PanelStore, PlanState } from "@/state/store";
import {
  IpcClient,
  appSettingsGet,
  appSettingsSave,
  euddraftCheckUpdate,
  euddraftSettingsGet,
  euddraftUpdate,
  attentionNotify,
  openScmdraft,
  pickScmdraftPath,
  compactSession,
  isAgentTurnEndTransition,
  mentionSearch,
  notificationSoundPreview,
  projectRecentList,
  projectRecentRemove,
  projectTakeLaunchRequest,
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
  setupCreateProject,
  setupPickProjectPath,
  setupProjectOpen,
  setupProviderSelect,
  projectExportE3s,
  wikiGet,
  wikiSave,
  workspaceList,
  workspaceRead,
  workspaceSearch,
  WORKSPACE_DOCUMENT_PREFIX,
  type AskAnswer,
  type AutonomousRunState,
  type AppSettings,
  type EuddraftSettings,
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
  type RecentProject,
  type ServerMessage,
  type SessionMeta,
  type SessionModelSettings,
  type SessionRecord,
  type SetupMessage,
  type WorkspaceFileEntry,
  type WorkspaceListResponse,
} from "@/lib/ipc";
import {
  formatImportIssueReason,
  importE3sProject,
  pickE3sImportDestination,
  pickE3sSource,
} from "@/lib/projectImport";
import {
  createBlankProject,
  mapNewBrushes,
  mapNewOptions,
  pickStarcraftPath,
} from "@/lib/mapNew";
import { progressLabel } from "@/lib/progress";
import { useProjectIdentityEffect } from "@/lib/projectIdentity";
import { formatPathForDisplay } from "@/lib/utils";
import {
  bootstrapView,
  type BootstrapView,
} from "@/setup/bootstrap";
import { SetupScreen } from "@/setup/SetupScreen";
import { ProjectLauncher } from "@/setup/ProjectLauncher";
import { UpdateNotice } from "@/components/UpdateNotice";
import { createUpdater, type UpdateHandle } from "@/setup/update";
import { PROVIDER_LABELS } from "@/providers/providerCopy";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  ClipboardCheck,
  ClipboardList,
  Search,
  type LucideIcon,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  discardAttachment,
  formatAttachmentSize,
  stageAttachment,
} from "@/lib/attachments";

const PROVIDER_POLL_MS = 2000;
const PROVIDER_POLL_TIMEOUT_MS = 300000;

/**
 * Native project refresh cadence. Status is local filesystem state; the source
 * list is reloaded only after recovery or a project identity change.
 */
const PROJECT_REFRESH_MS = 2000;

/** Maximum simultaneously open workspace document tabs in the center column. */
const MAX_DOCUMENT_TABS = 8;

/**
 * Virtual center tab id for the selected session's active plan. Workspace
 * paths are confined relative paths and never contain `:`, so it cannot
 * collide with a document tab.
 */
const PLAN_TAB_ID = "workflow:plan";

/** Staged-workflow artifact families that open as center tabs. */
type StageArtifactKind = "research" | "plan" | "verify";

/**
 * Stage artifact directories (engine-rendered `research/`, `plans/`,
 * `verify/` markdown under the workspace document tree) and their tab
 * presentation. `dir` is the project-relative form used by the file tree and
 * document tabs; workflow events name artifacts workspace-relative.
 */
const STAGE_TABS: Record<
  StageArtifactKind,
  { dir: string; label: string; icon: LucideIcon }
> = {
  research: {
    dir: `${WORKSPACE_DOCUMENT_PREFIX}research/`,
    label: "조사 보고",
    icon: Search,
  },
  plan: {
    dir: `${WORKSPACE_DOCUMENT_PREFIX}plans/`,
    label: "계획",
    icon: ClipboardList,
  },
  verify: {
    dir: `${WORKSPACE_DOCUMENT_PREFIX}verify/`,
    label: "검증 보고",
    icon: ClipboardCheck,
  },
};

/** Project-relative tab path of a workspace-relative stage artifact path. */
function stageArtifactTabPath(workspacePath: string): string {
  return `${WORKSPACE_DOCUMENT_PREFIX}${workspacePath}`;
}

function stageArtifactKind(path: string): StageArtifactKind | null {
  for (const kind of Object.keys(STAGE_TABS) as StageArtifactKind[]) {
    if (path.startsWith(STAGE_TABS[kind].dir)) return kind;
  }
  return null;
}

/** Tab label/icon for a workspace document: stage reports get a Korean name. */
function documentTabPresentation(path: string): Pick<DocumentTab, "label" | "icon"> {
  const kind = stageArtifactKind(path);
  if (kind !== null) {
    return { label: STAGE_TABS[kind].label, icon: STAGE_TABS[kind].icon };
  }
  return { label: path.slice(path.lastIndexOf("/") + 1) };
}

/** Per-tab fetch state; contents persist across tab switches. */
interface DocumentTabState {
  content: string | null;
  loading: boolean;
  error: string | null;
  /** Why a listed file stays closed (binary or oversized); not an error. */
  notice: string | null;
}

/** Korean explanation for a file the viewer cannot show as text. */
function unreadableNotice(
  reason: "binary" | "too_large",
  size: number,
): string {
  const formatted = formatAttachmentSize(size);
  return reason === "too_large"
    ? `파일이 너무 커서 표시하지 않습니다 (1 MB 초과 · ${formatted}). 다른 프로그램으로 여세요.`
    : `텍스트로 표시할 수 없는 파일입니다 (바이너리 · ${formatted}). 맵은 Map 창이나 SCMDraft 2로 여세요.`;
}

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
    if (entry.detail) next.detail = entry.detail;
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
  autonomousRun: AutonomousRunState | null;
  unsubscribe?: () => void;
  saveTimer?: number;
  observedLog: readonly LogEntry[];
  changesetOpen: boolean;
  harnessJobs: HarnessJobView[];
}

type PendingAskSnapshot = {
  sessionId: string;
  requestId: string;
  questions: Parameters<PanelStore["askReceived"]>[1];
  /** Remaining bounded wait reported by the core at restore time. */
  waitSeconds?: number;
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
  target.projectAvailabilityChanged(state.projectAvailable);
  if (state.rag !== "unknown") target.ragWarmupChanged(state.rag);
  if (state.memory) {
    target.memoryReceived(state.memory.project, state.memory.files);
  }
  if (state.wikiData) {
    target.wikiReceived(state.wikiData.version, state.wikiData.entries);
  }
}


function projectOpenError(error: unknown): string {
  const detail = String(error).replace(/^Error:\s*/, "").replace(/^[a-z_]+:\s*/, "");
  return /[가-힣]/.test(detail)
    ? detail
    : `프로젝트를 열지 못했습니다. 프로젝트 파일과 원본 맵의 위치를 확인해 주세요. (${detail})`;
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
  const [launcherVisible, setLauncherVisible] = useState(true);
  const [recentProjects, setRecentProjects] = useState<RecentProject[]>([]);
  const [recentLoading, setRecentLoading] = useState(true);
  const [launcherBusy, setLauncherBusy] = useState(false);
  const [launcherError, setLauncherError] = useState<string | null>(null);
  const [activeProjectPath, setActiveProjectPath] = useState<string | null>(null);
  const [pendingLaunchPath, setPendingLaunchPath] = useState<string | null>(null);
  const [launchInitializing, setLaunchInitializing] = useState(true);
  const launchQueueRef = useRef<string[]>([]);
  const launchBusyRef = useRef(false);
  const [ragElapsedSec, setRagElapsedSec] = useState(0);
  const [bootstrap, setBootstrap] = useState<BootstrapState>(() => ({
    active: false,
    view: bootstrapView(null, undefined),
    error: null,
  }));
  const bootstrapActiveRef = useRef(false);
  const [setup, setSetup] = useState<SetupMessage | null>(null);
  const bootstrapRunningRef = useRef(false);
  const euddraftActionRef = useRef(false);
  const [euddraftAction, setEuddraftAction] = useState<
    "file" | "folder" | "install" | null
  >(null);
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
  // Armed after setup completes. A cheap periodic native status refresh detects
  // external project removal and recovery without affecting the IPC transport.
  const [projectPollEnabled, setProjectPollEnabled] = useState(false);
  const projectSetupBusyRef = useRef(false);
  const [projectSetupAction, setProjectSetupAction] = useState<
    "open" | "create" | "import" | "export" | null
  >(null);
  const [e3sImportOpen, setE3sImportOpen] = useState(false);
  const [newMapWizardOpen, setNewMapWizardOpen] = useState(false);
  const projectDialogOpenRef = useRef(false);
  projectDialogOpenRef.current = e3sImportOpen || newMapWizardOpen;
  const [sessionSidebarCollapsed, setSessionSidebarCollapsed] = useState(false);
  const [projectSidebarOpen, setProjectSidebarOpen] = useState(true);
  const [projectPanelTab, setProjectPanelTab] =
    useState<ProjectPanelTab>("workspace");
  const [workspaceData, setWorkspaceData] =
    useState<WorkspaceListResponse | null>(null);
  const [workspaceLoading, setWorkspaceLoading] = useState(false);
  const [workspaceError, setWorkspaceError] = useState<string | null>(null);
  // Center-column document tabs (Orca-style: tree right, document center). The
  // tab list and per-path contents live here so switching tabs never refetches
  // and each tab keeps its own loading/error state.
  const [openDocumentTabs, setOpenDocumentTabs] = useState<string[]>([]);
  const [activeCenterTab, setActiveCenterTab] = useState<DocumentTabId>("chat");
  const [documentStates, setDocumentStates] = useState<
    Record<string, DocumentTabState>
  >({});
  // Sessions whose virtual plan tab the user closed; a new plan revision
  // reopens it (see the plan-arrival effect below).
  const [planTabHidden, setPlanTabHidden] = useState<ReadonlySet<string>>(
    () => new Set(),
  );
  // The document tab auto-opened for each stage kind; the next artifact of
  // the same kind replaces it in place so pipeline requests do not pile up
  // 조사 보고/검증 보고 tabs. Cleared with the project.
  const autoOpenedStageTabs = useRef(new Map<StageArtifactKind, string>());
  // A read that completes after its tab was closed/replaced reinserts an
  // invisible entry; prune those so per-path contents cannot accumulate.
  useEffect(() => {
    setDocumentStates((current) => {
      const open = new Set(openDocumentTabs);
      const orphans = Object.keys(current).filter((path) => !open.has(path));
      if (orphans.length === 0) return current;
      const next = { ...current };
      for (const path of orphans) delete next[path];
      return next;
    });
  }, [documentStates, openDocumentTabs]);
  // Self-update banner state: the pending update (null until found) and a
  // session-scoped "나중에" dismissal. The check fires once (guarded by the ref).
  const [update, setUpdate] = useState<UpdateHandle | null>(null);
  const [updateDismissed, setUpdateDismissed] = useState(false);
  const updateCheckedRef = useRef(false);
  const [sessionModelSettings, setSessionModelSettings] =
    useState<SessionModelSettings | null>(null);
  const [providerSettingsBusy, setProviderSettingsBusy] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  // Category a task asked the settings dialog to open on; cleared on close so the
  // gear button keeps the dialog's own last category.
  const [settingsCategory, setSettingsCategory] = useState<SettingsCategory>();
  useEffect(() => {
    if (!settingsOpen) setSettingsCategory(undefined);
  }, [settingsOpen]);
  const [appSettings, setAppSettings] = useState<AppSettings | null>(null);
  const [appSettingsBusy, setAppSettingsBusy] = useState(false);
  const [euddraftSettings, setEuddraftSettings] =
    useState<EuddraftSettings | null>(null);
  const [euddraftSettingsBusy, setEuddraftSettingsBusy] = useState<
    "load" | "check" | "update" | null
  >(null);
  const [euddraftSettingsError, setEuddraftSettingsError] = useState<string>();
  const [scmdraftPickBusy, setScmdraftPickBusy] = useState(false);
  const [scmdraftPickError, setScmdraftPickError] = useState<string>();
  // Message undo/edit flow: the core must finish cancellation/rewind before the
  // input unlocks. `editDraft` is applied by InstructionBox without controlling
  // subsequent typing.
  const [editDraft, setEditDraft] = useState<ChatPayload | null>(null);
  const [messageActionBusy, setMessageActionBusy] = useState(false);
  const [harnessActionJobId, setHarnessActionJobId] = useState<string | null>(null);
  const messageActionBusyRef = useRef(false);
  const projectCompilingRef = useRef(false);
  const selectedPhaseRef = useRef(state.phase);
  projectCompilingRef.current = projectState.compiling;
  selectedPhaseRef.current = state.phase;

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
        existing.autonomousRun = record.autonomousRun ?? null;
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
        autonomousRun: record.autonomousRun ?? null,
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
          slot.store.askReceived(
            pending.requestId,
            pending.questions,
            pending.waitSeconds,
          );
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
      autonomousRun: null,
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
  const clearProjectSessions = useCallback(() => {
    for (const slot of sessionsRef.current.values()) {
      slot.unsubscribe?.();
      if (slot.saveTimer !== undefined) {
        window.clearTimeout(slot.saveTimer);
        if (slot.persisted) {
          void invoke("session_update_log", {
            id: slot.id,
            panelLog: serializePanelLog(slot.store.getState().log),
          }).catch(() => toast.error("이전 프로젝트의 대화 저장에 실패했습니다."));
        }
      }
    }
    sessionsRef.current.clear();
    loadedProjectRef.current = null;
    setPlanTabHidden(new Set());
    seenStageArtifacts.current.clear();
    setSelectedSessionId(null);
    selectedSessionIdRef.current = null;
    projectStore.applyStatus({ compiling: false, project: "" });
    projectStore.applyList({ files: undefined });
    projectStore.projectAvailabilityChanged(false);
    setWorkspaceData(null);
    setWorkspaceError(null);
    setOpenDocumentTabs([]);
    setDocumentStates({});
    setActiveCenterTab("chat");
    setEditDraft(null);
    bumpSessions();
  }, [bumpSessions, projectStore]);

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
    if (projectPollEnabled) void loadProviders();
  }, [projectPollEnabled, loadProviders]);

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

  const loadEuddraftSettings = useCallback(async () => {
    setEuddraftSettingsBusy("load");
    setEuddraftSettingsError(undefined);
    try {
      setEuddraftSettings(await euddraftSettingsGet());
    } catch {
      setEuddraftSettings(null);
      setEuddraftSettingsError(
        "euddraft 설정을 불러오지 못했습니다. 다시 시도해 주세요.",
      );
    } finally {
      setEuddraftSettingsBusy(null);
    }
  }, []);

  useEffect(() => {
    if (settingsOpen) void loadEuddraftSettings();
  }, [loadEuddraftSettings, settingsOpen]);

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

  const handleOpenScmdraft = useCallback(async () => {
    try {
      const launch = await openScmdraft();
      if (launch.kind === "unconfigured") {
        setSettingsCategory("compile");
        setSettingsOpen(true);
        toast.info("SCMDraft 2 실행 파일을 지정한 뒤 다시 \"SCMDraft 2로 열기\"를 눌러 주세요.");
      }
    } catch (reason) {
      toast.error(String(reason));
    }
  }, []);

  const handleScmdraftPick = useCallback(async () => {
    setScmdraftPickBusy(true);
    setScmdraftPickError(undefined);
    try {
      const picked = await pickScmdraftPath();
      if (picked !== null) {
        setAppSettings((current) =>
          current ? { ...current, scmdraftPath: picked } : current,
        );
      }
    } catch {
      setScmdraftPickError(
        "SCMDraft 2 실행 파일을 설정하지 못했습니다. ScmDraft 2.exe를 다시 선택해 주세요.",
      );
    } finally {
      setScmdraftPickBusy(false);
    }
  }, []);

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
  // when the conversation is scrolled away or the user is on the input, unless
  // the row is `silent` (its folded detail would overrun the viewport). The log
  // Toast high-water marks are session-scoped so selecting an old conversation
  // neither replays its historical alerts nor suppresses a newer session's ids.
  useEffect(() => {
    const sessionId = selectedSlot?.id;
    if (!sessionId) return;
    let highWater = toastedLogBySessionRef.current.get(sessionId) ?? 0;
    for (const entry of state.log) {
      if (entry.id <= highWater) continue;
      if (!entry.silent) {
        if (entry.kind === "error") toast.error(entry.text);
        else if (entry.kind === "warn") toast.warning(entry.text);
      }
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
          if (msg.error?.startsWith("project_create_failed:")) {
            toast.error("Native 프로젝트를 만들지 못했습니다.");
          } else if (msg.error?.startsWith("e3s_import_failed:")) {
            toast.error("E3S 프로젝트를 가져오지 못했습니다.");
          }
          if (msg.projectOpened) {
            if (msg.importIssues?.length) {
              toast.warning(`하네스 자료 ${msg.importIssues.length}개를 옮기지 못했습니다.`, {
                description: (
                  <div className="max-h-60 overflow-y-auto break-all">
                    <p>가져올 수 있는 자료는 프로젝트 안으로 복사했고 원본은 보존했습니다.</p>
                    <ul className="mt-2 space-y-2">
                      {msg.importIssues.map((issue) => (
                        <li key={issue.id}>
                          <div className="font-mono text-xs">{issue.path}</div>
                          <div>{formatImportIssueReason(issue.reason)}</div>
                          <details>
                            <summary className="cursor-pointer">제외 사유 상세</summary>
                            <p className="whitespace-pre-wrap text-xs">{issue.reason}</p>
                          </details>
                        </li>
                      ))}
                    </ul>
                  </div>
                ),
                duration: Infinity,
                closeButton: true,
              });
            }
            clearProjectSessions();
            setActiveProjectPath(msg.projectPath);
            setLauncherVisible(false);
            setLauncherError(null);
            setSettingsOpen(false);
            const client = clientRef.current;
            client?.invalidateProject();
            setProjectPollEnabled(false);
            void projectRecentList()
              .then(setRecentProjects)
              .catch(() => {
                // Recent history is advisory; an open project remains usable.
              });
          }
          if (!msg.setupRequired) {
            bootstrapActiveRef.current = false;
            setBootstrap((prev) =>
              prev.active ? { ...prev, active: false } : prev,
            );
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
            if (msg.detail === "euddraft configured") {
              // The installer announces readiness before its caller persists
              // the selected path. Refresh only after this post-save marker.
              bootstrapActiveRef.current = false;
              setBootstrap((prev) => ({ ...prev, active: false, error: null }));
              void clientRef.current?.send({ type: "setup_status" });
              break;
            }
            const view = bootstrapView(msg.pct, msg.detail);
            bootstrapActiveRef.current = view.phase !== "error";
            setBootstrap({
              active: view.phase !== "error",
              view,
              error: view.phase === "error" ? view.label : null,
            });
            break;
          }
          if (msg.stage === "euddraft_update") {
            // Settings owns this invoke lifecycle and renders its progress
            // in-place; it must not enter setup or pollute a chat session.
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
        case "autonomous_run": {
          const targetSlot = sessionsRef.current.get(msg.sessionId);
          if (!targetSlot) break;
          const { type: _type, sessionId: _sessionId, ...run } = msg;
          targetSlot.autonomousRun = run;
          bumpSessions();
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
          if (msg.status === "expired") {
            targetSlot.store.askExpired(msg.requestId);
            break;
          }
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
          targetSlot.store.askReceived(msg.requestId, msg.questions, msg.waitSeconds);
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
        case "workflow": {
          const target = sessionStore();
          if (!target) break;
          const { type: _type, sessionId: _sessionId, ...event } = msg;
          target.workflowReceived(event);
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
    [bumpSessions, clearProjectSessions, projectStore],
  );

  const projectSwitchBlocked = useCallback(() => {
    if (projectCompilingRef.current) return "빌드 또는 맵 작업이 진행 중입니다. 작업이 끝난 뒤 프로젝트를 전환해 주세요.";
    if (isBusyPhase(selectedPhaseRef.current) || selectedPhaseRef.current === "plan_review" || selectedPhaseRef.current === "changeset_review") {
      return "현재 세션이 작업 중이거나 검토를 기다리고 있습니다. 세션을 마친 뒤 프로젝트를 전환해 주세요.";
    }
    for (const slot of sessionsRef.current.values()) {
      if (slot.activity !== "idle" && slot.activity !== "error") {
        return "진행 중인 세션 작업이 있습니다. 작업이 끝난 뒤 프로젝트를 전환해 주세요.";
      }
      if (slot.harnessJobs.some((job) =>
        job.status === "waiting_runtime" ||
        job.status === "pending" ||
        job.status === "running" ||
        job.status === "review"
      )) {
        return "맵 또는 하네스 작업이 진행 중입니다. 작업이 끝난 뒤 프로젝트를 전환해 주세요.";
      }
    }
    return null;
  }, []);

  const processLaunchQueue = useCallback(async () => {
    if (launchBusyRef.current) return;
    if (projectSetupBusyRef.current || projectDialogOpenRef.current) {
      setLauncherError("프로젝트 선택 또는 가져오기를 마친 뒤 대기 중인 열기 요청을 다시 시도해 주세요.");
      return;
    }
    launchBusyRef.current = true;
    setLauncherBusy(true);
    try {
      while (launchQueueRef.current.length > 0) {
        const path = launchQueueRef.current[0];
        setPendingLaunchPath(path);
        const blocked = projectSwitchBlocked();
        if (blocked) {
          setLauncherError(blocked);
          setLauncherVisible(true);
          break;
        }
        try {
          const response = await setupProjectOpen(path);
          if (!response.projectOpened) {
            throw new Error(response.error ?? "프로젝트 파일과 원본 맵의 위치를 확인해 주세요.");
          }
          onMessage(response);
          launchQueueRef.current.shift();
          setPendingLaunchPath(launchQueueRef.current[0] ?? null);
          setLauncherError(null);
        } catch (error) {
          setLauncherError(projectOpenError(error));
          setLauncherVisible(true);
          break;
        }
      }
    } finally {
      launchBusyRef.current = false;
      setLauncherBusy(false);
    }
  }, [onMessage, projectSwitchBlocked]);

  const enqueueLaunch = useCallback(
    (path: string) => {
      const trimmed = path.trim();
      if (!trimmed) return;
      if (!launchQueueRef.current.includes(trimmed)) launchQueueRef.current.push(trimmed);
      setPendingLaunchPath(launchQueueRef.current[0]);
      setLauncherVisible(true);
      void processLaunchQueue();
    },
    [processLaunchQueue],
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
      onProjectAvailabilityChange: (available) => {
        projectStore.projectAvailabilityChanged(available);
        for (const slot of sessionsRef.current.values()) {
          slot.store.projectAvailabilityChanged(available);
        }
      },
    });
    clientRef.current = client;
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    let draining = false;
    let drainRequested = false;
    const drainLaunchRequest = async () => {
      drainRequested = true;
      if (draining) return;
      draining = true;
      try {
        do {
          drainRequested = false;
          while (!cancelled) {
            const path = await projectTakeLaunchRequest();
            if (!path) break;
            enqueueLaunch(path);
          }
        } while (drainRequested && !cancelled);
      } catch (error) {
        if (!cancelled) setLauncherError(`프로젝트 열기 요청을 확인하지 못했습니다: ${String(error)}`);
      } finally {
        draining = false;
      }
    };
    void client.connect().then(async () => {
      if (cancelled) return;
      try {
        unlisten = await listen("project-open-requested", drainLaunchRequest);
        if (cancelled) {
          unlisten();
          return;
        }
        await client.send({ type: "setup_status" });
        await drainLaunchRequest();
      } catch (error) {
        if (!cancelled) setLauncherError(`프로젝트 시작 정보를 불러오지 못했습니다: ${String(error)}`);
      } finally {
        if (!cancelled) setLaunchInitializing(false);
      }
    });
    return () => {
      cancelled = true;
      unlisten?.();
      client.stop();
      clientRef.current = null;
    };
  }, [enqueueLaunch, onMessage, projectStore, syncPendingAsk]);
  useEffect(() => {
    let active = true;
    void projectRecentList()
      .then((projects) => {
        if (active) setRecentProjects(projects);
      })
      .catch(() => {
        if (active) setLauncherError("최근 프로젝트를 불러오지 못했습니다. 프로젝트 파일을 직접 열거나 앱을 다시 시작해 주세요.");
      })
      .finally(() => {
        if (active) setRecentLoading(false);
      });
    return () => {
      active = false;
    };
  }, []);

  // after a session_open reconnect completes. The panel already hydrated from
  // the command result, so this only pulls a fresh native project snapshot.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void listen("session_loaded", () => {
      if (activeProjectPath) void clientRef.current?.refresh();
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [activeProjectPath]);

  useEffect(() => {
    setProjectPollEnabled(activeProjectPath !== null && setup !== null && !setup.setupRequired);
  }, [activeProjectPath, setup]);

  // Refresh native project status after setup. The in-process transport remains
  // open if the project path disappears, and availability recovers automatically.
  useEffect(() => {
    if (!projectPollEnabled) return;
    const client = clientRef.current;
    if (!client) return;
    let cancelled = false;
    const probe = () => {
      if (!cancelled) void client.refresh();
    };
    probe();
    const id = window.setInterval(probe, PROJECT_REFRESH_MS);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, [projectPollEnabled, activeProjectPath]);

  // Populate the persistent left sidebar with sessions owned by the current
  // native project. Loading a row is read-only and never steals the execution lane.
  useEffect(() => {
    const project = projectState.project.trim();
    if (!projectPollEnabled || !projectState.hasProject || !project || loadedProjectRef.current === activeProjectPath) {
      return;
    }
    loadedProjectRef.current = activeProjectPath;
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
    activeProjectPath,
    projectPollEnabled,
    bumpSessions,
    createDraftSlot,
    projectState.hasProject,
    projectState.project,
    registerSession,
    syncPendingAsk,
  ]);

  // A blank euddraft path opts into the managed latest-release installer. A
  // non-empty invalid path is an explicit user choice and must never be
  // overwritten by this effect.
  useEffect(() => {
    if (!activeProjectPath || !setup?.setupRequired || !setup.projectValid) return;
    const euddraftMissing = setup.euddraftPath.trim().length === 0;
    if (!euddraftMissing && (!setup.euddraftValid || setup.assetsReady)) return;
    if (bootstrapRunningRef.current) return;
    const client = clientRef.current;
    if (!client) return;
    bootstrapRunningRef.current = true;
    void client
      .send({ type: "bootstrap_run" })
      .then((ok) => {
        if (!ok && !bootstrapActiveRef.current) {
          const view = bootstrapView(null, "error: euddraft 자동 설치 명령이 실패했습니다.");
          setBootstrap({ active: false, view, error: view.label });
        }
      })
      .finally(() => {
        bootstrapRunningRef.current = false;
      });
  }, [
    activeProjectPath,
    setup?.setupRequired,
    setup?.projectValid,
    setup?.euddraftPath,
    setup?.euddraftValid,
    setup?.assetsReady,
  ]);

  // Once first-run setup is satisfied, check for an app self-update exactly once.
  // Non-blocking: an updater error (offline, no release yet) just leaves the banner
  // hidden — it never gates the panel.
  useEffect(() => {
    if (!setup || setup.setupRequired || launcherVisible) return;
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
  }, [setup, updater, launcherVisible]);
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
          isBusyPhase(snapshot.phase) ||
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
              executionMode: payload.executionMode,
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
          executionMode: payload.executionMode,
        });
        if (!sent) {
          slot.store.errorReceived("요청을 처리하지 못했습니다.");
          setEditDraft({
            text: payload.text,
            attachments: [...payload.attachments],
            mentions: payload.mentions.map((mention) => ({ ...mention })),
            executionMode: payload.executionMode,
            clientTurnId,
          });
        }
      } catch (error) {
        setEditDraft({
          text: payload.text,
          attachments: [...payload.attachments],
          mentions: payload.mentions.map((mention) => ({ ...mention })),
          executionMode: payload.executionMode,
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
      (isBusyPhase(slot.store.getState().phase) ||
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

  const sendWorkflowControl = useCallback(
    async (type: "workflow_resume" | "workflow_restart") => {
      const slot = selectedSlot;
      const snapshot = slot?.store.getState();
      // Resume applies to an interrupted stage only; restart also accepts a
      // cancelled (or failed) request whose artifacts were retained.
      const restartable =
        snapshot?.workflow?.stage === "interrupted" ||
        snapshot?.workflow?.stage === "cancelled" ||
        snapshot?.workflow?.stage === "failed";
      const allowed =
        type === "workflow_resume"
          ? snapshot?.phase === "interrupted"
          : restartable && !isBusyPhase(snapshot.phase);
      if (!slot?.persisted || !allowed || messageActionBusyRef.current) {
        return;
      }
      messageActionBusyRef.current = true;
      setMessageActionBusy(true);
      try {
        const sent = await clientRef.current?.send({ type, sessionId: slot.id });
        if (sent) {
          if (type === "workflow_resume") slot.store.workflowResumeSent();
          else slot.store.workflowRestartSent();
          markConversationStarted(slot);
        } else {
          slot.store.log(
            "error",
            type === "workflow_resume"
              ? "중단된 작업을 이어서 진행하지 못했습니다."
              : "작업을 처음부터 다시 시작하지 못했습니다.",
          );
        }
      } finally {
        messageActionBusyRef.current = false;
        setMessageActionBusy(false);
      }
    },
    [markConversationStarted, selectedSlot],
  );

  const handleWorkflowResume = useCallback(
    () => void sendWorkflowControl("workflow_resume"),
    [sendWorkflowControl],
  );
  const handleWorkflowRestart = useCallback(
    () => void sendWorkflowControl("workflow_restart"),
    [sendWorkflowControl],
  );

  const sendAutonomousControl = useCallback(
    (type: "autonomous_pause" | "autonomous_resume" | "autonomous_stop") => {
      const slot = selectedSlot;
      if (!slot?.persisted) return;
      void clientRef.current
        ?.send({ type, sessionId: slot.id })
        .then((sent) => {
          if (!sent) {
            slot.store.log(
              "error",
              type === "autonomous_pause"
                ? "장시간 작업을 일시 중지하지 못했습니다."
                : type === "autonomous_resume"
                  ? "장시간 작업을 계속하지 못했습니다."
                  : "장시간 작업을 중단하지 못했습니다.",
            );
          }
        });
    },
    [selectedSlot],
  );

  const handleAutonomousPause = useCallback(
    () => sendAutonomousControl("autonomous_pause"),
    [sendAutonomousControl],
  );
  const handleAutonomousResume = useCallback(
    () => sendAutonomousControl("autonomous_resume"),
    [sendAutonomousControl],
  );
  const handleAutonomousStop = useCallback(
    () => sendAutonomousControl("autonomous_stop"),
    [sendAutonomousControl],
  );

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
            executionMode: "interactive",
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
      void handleSend({
        text,
        attachments: [],
        mentions: [],
        executionMode: "interactive",
      });
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

  const handleBootstrapRetry = useCallback(() => {
    if (bootstrapRunningRef.current) return;
    setBootstrap((prev) => ({
      ...prev,
      error: null,
      view: bootstrapView(null, undefined),
    }));
    const client = clientRef.current;
    if (!client) return;
    bootstrapRunningRef.current = true;
    void client
      .send({ type: "bootstrap_run" })
      .then((ok) => {
        if (!ok) {
          const view = bootstrapView(null, "error: 설치 명령이 실패했습니다.");
          setBootstrap({ active: false, view, error: view.label });
        }
      })
      .finally(() => {
        bootstrapRunningRef.current = false;
      });
  }, []);


  const runProjectSetupAction = useCallback(
    (
      action: "open" | "create",
      command: "setup_pick_project_path" | "setup_create_project",
      directory = false,
    ) => {
      const blocked = projectSwitchBlocked();
      if (blocked) {
        toast.error(blocked);
        return;
      }
      if (projectSetupBusyRef.current || launchBusyRef.current) return;
      launchQueueRef.current = [];
      setPendingLaunchPath(null);
      setLauncherError(null);
      projectSetupBusyRef.current = true;
      setProjectSetupAction(action);
      const request =
        command === "setup_pick_project_path"
          ? setupPickProjectPath(directory)
          : setupCreateProject();
      void request
        .then((response) => {
          onMessage(response);
          if (!response.projectOpened && response.error) {
            setLauncherError(projectOpenError(response.error));
            setLauncherVisible(true);
          }
        })
        .catch((error) => {
          setLauncherError(projectOpenError(error));
          setLauncherVisible(true);
        })
        .finally(() => {
          projectSetupBusyRef.current = false;
          setProjectSetupAction(null);
          if (launchQueueRef.current.length > 0) void processLaunchQueue();
        });
    },
    [onMessage, processLaunchQueue, projectSwitchBlocked],
  );

  const openE3sImport = useCallback(() => {
    if (projectSetupBusyRef.current || launchBusyRef.current) return;
    const blocked = projectSwitchBlocked();
    if (blocked) {
      toast.error(blocked);
      return;
    }
    setSettingsOpen(false);
    setE3sImportOpen(true);
  }, [projectSwitchBlocked]);

  const openNewMapWizard = useCallback(() => {
    if (projectSetupBusyRef.current || launchBusyRef.current) return;
    const blocked = projectSwitchBlocked();
    if (blocked) {
      toast.error(blocked);
      return;
    }
    setLauncherError(null);
    setNewMapWizardOpen(true);
  }, [projectSwitchBlocked]);

  const handleBlankProjectCreated = useCallback(
    (nextSetup: SetupMessage) => {
      if (!nextSetup.projectOpened) {
        setLauncherError(projectOpenError(nextSetup.error ?? "새 맵 프로젝트 생성이 취소되었습니다."));
        return;
      }
      onMessage(nextSetup);
    },
    [onMessage],
  );

  const handleE3sImported = useCallback(
    (nextSetup: SetupMessage) => {
      if (!nextSetup.projectOpened) {
        setLauncherError(projectOpenError(nextSetup.error ?? "프로젝트 가져오기가 취소되었습니다."));

        return;
      }
      onMessage(nextSetup);
    },
    [onMessage],
  );
  const handleProjectSwitch = useCallback(() => {
    const blocked = projectSwitchBlocked();
    if (blocked) {
      toast.error(blocked);
      return;
    }
    setLauncherError(null);
    setLauncherVisible(true);
    setRecentLoading(true);
    void projectRecentList()
      .then(setRecentProjects)
      .catch(() => setLauncherError("최근 프로젝트를 불러오지 못했습니다. 프로젝트 파일을 직접 열어 주세요."))
      .finally(() => setRecentLoading(false));
  }, [projectSwitchBlocked]);

  const handleProjectExport = useCallback(() => {
    if (projectSetupBusyRef.current) return;
    projectSetupBusyRef.current = true;
    setProjectSetupAction("export");
    void projectExportE3s()
      .then((result) => {
        if (result) toast.success(`E3S를 내보냈습니다: ${result.path}`);
      })
      .catch((error) => {
        const detail = String(error);
        toast.error(
          detail.includes("compatibility base")
            ? "E3S에서 가져온 프로젝트만 E3S로 내보낼 수 있습니다."
            : "E3S를 내보내지 못했습니다. 프로젝트 파일과 대상 경로를 확인해 주세요.",
        );
      })
      .finally(() => {
        projectSetupBusyRef.current = false;
        setProjectSetupAction(null);
      });
  }, []);

  const handlePickEuddraftPath = useCallback((directory = false) => {
    if (euddraftActionRef.current) return;
    const client = clientRef.current;
    if (!client) return;
    euddraftActionRef.current = true;
    setEuddraftAction(directory ? "folder" : "file");
    void client
      .send({ type: "setup_pick_euddraft_path", directory })
      .finally(() => {
        euddraftActionRef.current = false;
        setEuddraftAction(null);
      });
  }, []);

  const handleInstallEuddraft = useCallback(() => {
    if (euddraftActionRef.current || bootstrapRunningRef.current) return;
    const client = clientRef.current;
    if (!client) return;
    euddraftActionRef.current = true;
    setEuddraftAction("install");
    setBootstrap({ active: true, view: bootstrapView(null), error: null });
    void client
      .send({ type: "setup_install_euddraft" })
      .then((ok) => {
        if (!ok && !bootstrapActiveRef.current) {
          const view = bootstrapView(null, "error: 최신 euddraft 설치 명령이 실패했습니다.");
          setBootstrap({ active: false, view, error: view.label });
        }
      })
      .finally(() => {
        euddraftActionRef.current = false;
        setEuddraftAction(null);
      });

  }, []);
  const refreshSetup = useCallback(() => {
    void clientRef.current?.send({ type: "setup_status" });
  }, []);
  const handleEuddraftCheck = useCallback(async () => {
    setEuddraftSettingsBusy("check");
    setEuddraftSettingsError(undefined);
    try {
      setEuddraftSettings(await euddraftCheckUpdate());
    } catch {
      setEuddraftSettingsError(
        "최신 euddraft 버전을 확인하지 못했습니다. 네트워크 연결을 확인하고 다시 시도해 주세요.",
      );
    } finally {
      setEuddraftSettingsBusy(null);
    }
  }, []);

  const handleEuddraftUpdate = useCallback(async () => {
    setEuddraftSettingsBusy("update");
    setEuddraftSettingsError(undefined);
    try {
      const updated = await euddraftUpdate();
      setEuddraftSettings(updated);
      refreshSetup();
      toast.success(`euddraft ${updated.installedVersion ?? ""} 업데이트를 완료했습니다.`);
    } catch {
      setEuddraftSettingsError(
        "euddraft를 업데이트하지 못했습니다. 네트워크 연결과 설치 경로를 확인한 뒤 다시 시도해 주세요.",
      );
    } finally {
      setEuddraftSettingsBusy(null);
    }
  }, [refreshSetup]);

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

  const loadDocumentTab = useCallback(
    async (workspaceId: string, path: string) => {
      setDocumentStates((current) => ({
        ...current,
        [path]: { content: null, loading: true, error: null, notice: null },
      }));
      try {
        const response = await workspaceRead(workspaceId, path);
        setDocumentStates((current) => ({
          ...current,
          [path]: {
            content: response.content,
            loading: false,
            error: null,
            notice:
              response.unreadable === undefined
                ? null
                : unreadableNotice(response.unreadable, response.size),
          },
        }));
      } catch (error) {
        setDocumentStates((current) => ({
          ...current,
          [path]: {
            content: null,
            loading: false,
            error: `파일을 열지 못했습니다: ${String(error)}`,
            notice: null,
          },
        }));
      }
    },
    [],
  );

  // Opening a file activates its center tab; at the tab cap the active
  // document tab is replaced (editor-style preview reuse) rather than
  // silently dropping the request.
  const openDocumentTab = useCallback(
    (file: WorkspaceFileEntry) => {
      const data = workspaceData;
      if (!data) return;
      setActiveCenterTab(file.path);
      setOpenDocumentTabs((current) => {
        if (current.includes(file.path)) return current;
        if (current.length < MAX_DOCUMENT_TABS) return [...current, file.path];
        const replaced = current.indexOf(activeCenterTab);
        const next = [...current];
        next[replaced === -1 ? 0 : replaced] = file.path;
        return next;
      });
      void loadDocumentTab(data.workspaceId, file.path);
    },
    [activeCenterTab, loadDocumentTab, workspaceData],
  );

  // Stage artifacts (research / verify markdown, and the plan file once its
  // virtual tab retires) open as document tabs the moment the workflow
  // snapshot announces them. `workspacePath` is the event's workspace-relative
  // path; the tab uses the project-relative form. The list is refreshed first
  // so the new file gets its tree entry; the tab auto-opened for the same
  // stage earlier is replaced in place. Returns whether the tab was opened.
  const openStageArtifactTab = useCallback(
    async (
      kind: StageArtifactKind,
      workspacePath: string,
      activate = true,
    ): Promise<boolean> => {
      const path = stageArtifactTabPath(workspacePath);
      let data = workspaceData;
      try {
        data = await workspaceList();
        setWorkspaceData(data);
      } catch (error) {
        if (!data) {
          store.log(
            "warn",
            `${STAGE_TABS[kind].label}를 열지 못했습니다: ${String(error)}`,
          );
          return false;
        }
      }
      const previous = autoOpenedStageTabs.current.get(kind);
      autoOpenedStageTabs.current.set(kind, path);
      setOpenDocumentTabs((current) => {
        if (current.includes(path)) return current;
        const next = [...current];
        const replaced =
          previous === undefined ? -1 : current.indexOf(previous);
        if (replaced !== -1) {
          next[replaced] = path;
        } else if (current.length < MAX_DOCUMENT_TABS) {
          next.push(path);
        } else {
          const active = current.indexOf(activeCenterTab);
          next[active === -1 ? 0 : active] = path;
        }
        return next;
      });
      if (activate) setActiveCenterTab(path);
      void loadDocumentTab(data.workspaceId, path);
      return true;
    },
    [activeCenterTab, loadDocumentTab, store, workspaceData],
  );

  const closeDocumentTab = useCallback(
    (id: DocumentTabId) => {
      if (id === "chat") return;
      if (id === PLAN_TAB_ID) {
        const sessionId = selectedSessionIdRef.current;
        if (sessionId !== null) {
          setPlanTabHidden((current) => new Set(current).add(sessionId));
        }
        if (activeCenterTab === PLAN_TAB_ID) {
          setActiveCenterTab(openDocumentTabs[openDocumentTabs.length - 1] ?? "chat");
        }
        return;
      }
      const index = openDocumentTabs.indexOf(id);
      if (index !== -1) {
        const next = openDocumentTabs.filter((path) => path !== id);
        setOpenDocumentTabs(next);
        if (activeCenterTab === id) {
          setActiveCenterTab(next[index] ?? next[index - 1] ?? "chat");
        }
      }
      setDocumentStates((current) => {
        if (!(id in current)) return current;
        const next = { ...current };
        delete next[id];
        return next;
      });
    },
    [activeCenterTab, openDocumentTabs],
  );

  const closeAllDocumentTabs = useCallback(() => {
    setOpenDocumentTabs([]);
    setDocumentStates({});
    const sessionId = selectedSessionIdRef.current;
    if (sessionId !== null) {
      setPlanTabHidden((current) => new Set(current).add(sessionId));
    }
    setActiveCenterTab("chat");
  }, []);

  // Ctrl+W closes the active document tab (the pinned conversation tab stays).
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!(event.ctrlKey && !event.metaKey && !event.altKey && !event.shiftKey)) return;
      if (event.key.toLowerCase() !== "w") return;
      if (activeCenterTab === "chat") return;
      event.preventDefault();
      closeDocumentTab(activeCenterTab);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [activeCenterTab, closeDocumentTab]);

  const handleWorkspaceSearch = useCallback(
    async (query: string) => {
      if (!workspaceData) return [];
      const response = await workspaceSearch(workspaceData.workspaceId, query);
      return response.paths;
    },
    [workspaceData],
  );

  // Refreshes the tree, drops tabs whose files disappeared, and reloads the
  // contents of the remaining open tabs so accepted agent writes show up.
  const handleWorkspaceRefresh = useCallback(async () => {
    setWorkspaceLoading(true);
    setWorkspaceError(null);
    try {
      const data = await workspaceList();
      setWorkspaceData(data);
      // Research/verify reports are readable but never listed, so their tabs
      // survive a refresh; the virtual plan tab is not a file at all.
      const available = new Set(data.files.map((file) => file.path));
      const retained = (path: string) =>
        available.has(path) || stageArtifactKind(path) !== null;
      const nextTabs = openDocumentTabs.filter(retained);
      if (nextTabs.length !== openDocumentTabs.length) {
        setOpenDocumentTabs(nextTabs);
        if (
          activeCenterTab !== "chat" &&
          activeCenterTab !== PLAN_TAB_ID &&
          !retained(activeCenterTab)
        ) {
          setActiveCenterTab(nextTabs[0] ?? "chat");
        }
      }
      for (const path of nextTabs) {
        void loadDocumentTab(data.workspaceId, path);
      }
    } catch (error) {
      setWorkspaceError(`워크스페이스를 불러오지 못했습니다: ${String(error)}`);
    } finally {
      setWorkspaceLoading(false);
    }
  }, [activeCenterTab, loadDocumentTab, openDocumentTabs]);

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
    projectState.hasProject ? activeProjectPath : null,
    (projectIdentity) => {
      setWorkspaceData(null);
      setWorkspaceError(null);
      setOpenDocumentTabs([]);
      setDocumentStates({});
      autoOpenedStageTabs.current.clear();
      setActiveCenterTab("chat");
      if (!projectIdentity) return;
      // The file tree is the default project tab, so its data loads with the
      // project; the wiki/memory ledgers load on first tab selection.
      void handleWorkspaceRefresh();
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
  // The stage strip and stage cards are pipeline surfaces: answer/direct
  // routes show 파악 during triage and then fall back to the ordinary
  // answer/changeset flow; done/failed hide them, while a cancelled request
  // stays visible as a terminal state offering 처음부터.
  const workflowActive =
    state.workflow !== null &&
    state.workflow.stage !== "done" &&
    state.workflow.stage !== "failed" &&
    (state.workflow.route === undefined || state.workflow.route === "pipeline");
  // The selected session's plan is a virtual tab (no workspace fetch: the
  // markdown is store state) that stays open read-only after approval until
  // the next request clears the plan.
  const planTabVisible =
    state.plan !== null &&
    selectedSessionId !== null &&
    !planTabHidden.has(selectedSessionId);
  const planRevision = state.plan?.revision;
  const documentTabs = useMemo<DocumentTab[]>(
    () => [
      { id: "chat", label: "대화" },
      ...openDocumentTabs.map((path) => ({
        id: path,
        ...documentTabPresentation(path),
      })),
      ...(planTabVisible && planRevision !== undefined
        ? [
            {
              id: PLAN_TAB_ID,
              label: `계획 (rev ${planRevision})`,
              icon: STAGE_TABS.plan.icon,
            },
          ]
        : []),
    ],
    [openDocumentTabs, planRevision, planTabVisible],
  );
  const handleCenterTabSelect = useCallback((id: DocumentTabId) => {
    setActiveCenterTab(id);
  }, []);
  const showPlanTab = useCallback(() => {
    const sessionId = selectedSessionIdRef.current;
    if (sessionId !== null) {
      setPlanTabHidden((current) => {
        if (!current.has(sessionId)) return current;
        const next = new Set(current);
        next.delete(sessionId);
        return next;
      });
    }
    setActiveCenterTab(PLAN_TAB_ID);
  }, []);

  // A plan the selected session has not shown yet opens (or reopens) the plan
  // tab and activates it; re-selecting the session later leaves the user's
  // tab choice alone. Identity, not revision: revisions restart at 1 for
  // every request, and the store creates one PlanState per `plan` event.
  const seenPlans = useRef(new WeakSet<PlanState>());
  const selectedPlan = state.plan;
  useEffect(() => {
    if (selectedSessionId === null || selectedPlan === null) return;
    if (seenPlans.current.has(selectedPlan)) return;
    seenPlans.current.add(selectedPlan);
    showPlanTab();
  }, [selectedPlan, selectedSessionId, showPlanTab]);

  // Research and verify artifacts open as 조사 보고 / 검증 보고 tabs once per
  // artifact hash while the pipeline is active (snapshots repeat the same
  // artifact on every later stage; hydrated done/failed requests stay quiet).
  const seenStageArtifacts = useRef(new Map<string, string>());
  const workflowResearch = workflowActive ? state.workflow?.research : undefined;
  const workflowVerdict = workflowActive ? state.workflow?.verdict : undefined;
  const openSeenStageArtifact = useCallback(
    (kind: "research" | "verify", sessionId: string, artifact: { path: string; sha256: string }, activate: boolean) => {
      const key = `${sessionId}:${kind}`;
      const identity = `${artifact.path}@${artifact.sha256}`;
      if (seenStageArtifacts.current.get(key) === identity) return;
      seenStageArtifacts.current.set(key, identity);
      void openStageArtifactTab(kind, artifact.path, activate).then((opened) => {
        // A failed open stays retryable on the next snapshot of the same artifact.
        if (!opened && seenStageArtifacts.current.get(key) === identity) {
          seenStageArtifacts.current.delete(key);
        }
      });
    },
    [openStageArtifactTab],
  );
  useEffect(() => {
    if (selectedSessionId === null || !workflowResearch) return;
    openSeenStageArtifact("research", selectedSessionId, workflowResearch, true);
  }, [openSeenStageArtifact, selectedSessionId, workflowResearch]);
  // The verdict arrives with changeset review (or a fix turn); when the
  // conversation is waiting on the user, the report opens without stealing
  // the tab so the changeset/ASK surface stays in view.
  const conversationAwaitsUser =
    state.phase === "changeset_review" || state.ask !== null;
  useEffect(() => {
    if (selectedSessionId === null || !workflowVerdict) return;
    openSeenStageArtifact("verify", selectedSessionId, workflowVerdict, !conversationAwaitsUser);
    // The activation choice belongs to the moment the verdict first appears,
    // so the phase/ASK state is read here rather than listed as a dependency.
  }, [openSeenStageArtifact, selectedSessionId, workflowVerdict]);

  // When the plan clears (the next request starts) while its tab is active,
  // the tab turns into the plan file's document tab so the approved plan stays
  // readable; without a known file, or after a session switch, fall back to
  // the conversation.
  const planTabRef = useRef<{
    sessionId: string;
    plan: PlanState;
    path?: string;
  } | null>(null);
  const planPath = state.workflow?.plan?.path;
  useEffect(() => {
    if (planTabVisible && selectedSessionId !== null && selectedPlan !== null) {
      const previous = planTabRef.current;
      planTabRef.current = {
        sessionId: selectedSessionId,
        plan: selectedPlan,
        path:
          planPath ??
          (previous?.sessionId === selectedSessionId && previous.plan === selectedPlan
            ? previous.path
            : undefined),
      };
      return;
    }
    if (activeCenterTab !== PLAN_TAB_ID) return;
    const previous = planTabRef.current;
    planTabRef.current = null;
    setActiveCenterTab("chat");
    if (
      previous !== null &&
      previous.sessionId === selectedSessionId &&
      previous.path !== undefined &&
      selectedPlan === null
    ) {
      void openStageArtifactTab("plan", previous.path);
    }
  }, [
    activeCenterTab,
    openStageArtifactTab,
    planPath,
    planTabVisible,
    selectedPlan,
    selectedSessionId,
  ]);
  if (launcherVisible) {
    return (
      <>
        <ProjectLauncher
          recent={recentProjects}
          loading={recentLoading}
          busy={launchInitializing || launcherBusy || projectSetupAction !== null}
          busyLabel={launchInitializing ? "프로젝트 시작 정보를 확인하는 중…" : "프로젝트 작업을 처리하는 중…"}
          pendingPath={pendingLaunchPath}
          onRetryPending={() => void processLaunchQueue()}
          onDismissPending={() => {
            launchQueueRef.current = [];
            setPendingLaunchPath(null);
            setLauncherError(null);
          }}
          error={launcherError}
          onOpenRecent={(path) => {
            launchQueueRef.current = [];
            enqueueLaunch(path);
          }}
          onOpenPicker={(directory = false) =>
            runProjectSetupAction("open", "setup_pick_project_path", directory)
          }
          onCreate={() => runProjectSetupAction("create", "setup_create_project")}
          onCreateBlank={openNewMapWizard}
          onImport={openE3sImport}
          onRemoveRecent={(path) => {
            if (projectSetupBusyRef.current || launchBusyRef.current) return;
            projectSetupBusyRef.current = true;
            setLauncherBusy(true);
            void projectRecentRemove(path)
              .then(setRecentProjects)
              .catch((error) =>
                setLauncherError(`최근 프로젝트를 제거하지 못했습니다: ${String(error)}`),
              )
              .finally(() => {
                projectSetupBusyRef.current = false;
                setLauncherBusy(false);
              });
          }}
          onCancel={activeProjectPath ? () => setLauncherVisible(false) : undefined}
        />
        <Toaster position="bottom-right" richColors closeButton />
        <E3sImportDialog
          open={e3sImportOpen}
          onOpenChange={setE3sImportOpen}
          pickSource={pickE3sSource}
          pickDestination={pickE3sImportDestination}
          importProject={importE3sProject}
          onImported={handleE3sImported}
        />
        <NewMapWizard
          open={newMapWizardOpen}
          onOpenChange={setNewMapWizardOpen}
          loadOptions={mapNewOptions}
          loadBrushes={mapNewBrushes}
          pickStarcraft={pickStarcraftPath}
          pickDestination={pickE3sImportDestination}
          create={createBlankProject}
          onCreated={handleBlankProjectCreated}
        />
      </>
    );
  }

  if (setup?.setupRequired || bootstrap.active) {

    return (
      <>
        <SetupScreen
          projectValid={setup?.projectValid ?? true}
          euddraftPath={setup?.euddraftPath ?? ""}
          euddraftValid={setup?.euddraftValid ?? true}
          pickError={setup?.error ?? null}
          onPickProject={() =>
            runProjectSetupAction("open", "setup_pick_project_path")
          }
          onCreateProject={() =>
            runProjectSetupAction("create", "setup_create_project")
          }
          onImportE3s={openE3sImport}
          projectAction={projectSetupAction === "export" ? null : projectSetupAction}
          onPickEuddraft={handlePickEuddraftPath}
          onInstallEuddraft={handleInstallEuddraft}
          euddraftAction={euddraftAction}
          bootstrapActive={bootstrap.active}
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
        <Toaster position="bottom-right" richColors closeButton />
        <E3sImportDialog
          open={e3sImportOpen}
          onOpenChange={setE3sImportOpen}
          pickSource={pickE3sSource}
          pickDestination={pickE3sImportDestination}
          importProject={importE3sProject}
          onImported={handleE3sImported}
        />
      </>
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
          projectAvailable={projectState.projectAvailable}
          hasProject={projectState.hasProject}
          onOpenMapAgent={() => handleOpenMapAgent()}
          onOpenScmdraft={() => void handleOpenScmdraft()}
          projectPanelOpen={projectSidebarOpen}
          onProjectPanelToggle={handleProjectPanelToggle}
          onSettingsOpen={() => setSettingsOpen(true)}
          onProjectSwitch={handleProjectSwitch}
        />
        {pendingLaunchPath && (
          <div role="status" className="flex items-center gap-3 border-b border-border bg-card px-4 py-2 text-sm">
            <span className="min-w-0 flex-1 truncate" title={formatPathForDisplay(pendingLaunchPath)}>대기 중인 프로젝트: {formatPathForDisplay(pendingLaunchPath)}</span>
            <button
              type="button"
              className="shrink-0 rounded-md px-3 py-2 text-primary outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring"
              onClick={() => setLauncherVisible(true)}
            >
              열기 요청 확인
            </button>
          </div>
        )}
        <SettingsDialog
          open={settingsOpen}
          category={settingsCategory}
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
          projectBusy={projectSetupAction}
          euddraft={euddraftSettings}
          euddraftBusy={euddraftSettingsBusy}
          euddraftError={euddraftSettingsError}
          scmdraftBusy={scmdraftPickBusy}
          scmdraftError={scmdraftPickError}
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
          onProjectOpen={() =>
            runProjectSetupAction("open", "setup_pick_project_path")
          }
          onProjectCreate={() =>
            runProjectSetupAction("create", "setup_create_project")
          }
          onProjectImport={openE3sImport}
          onProjectExport={handleProjectExport}
          onEuddraftCheck={handleEuddraftCheck}
          onEuddraftUpdate={handleEuddraftUpdate}
          onScmdraftPick={handleScmdraftPick}
        />
        <E3sImportDialog
          open={e3sImportOpen}
          onOpenChange={setE3sImportOpen}
          pickSource={pickE3sSource}
          pickDestination={pickE3sImportDestination}
          importProject={importE3sProject}
          onImported={handleE3sImported}
        />

        {update && !updateDismissed && (
          <UpdateNotice
            update={update}
            relaunch={updater.relaunch}
            onLater={() => setUpdateDismissed(true)}
          />
        )}
        {!projectState.projectAvailable && <ConnectionNotice />}

        <DocumentTabStrip
          tabs={documentTabs}
          activeTab={activeCenterTab}
          onSelect={handleCenterTabSelect}
          onClose={closeDocumentTab}
          onCloseAll={closeAllDocumentTabs}
        />

        <div
          id="document-panel-chat"
          role="tabpanel"
          aria-labelledby="document-tab-chat"
          className={
            activeCenterTab === "chat"
              ? "flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
              : "hidden"
          }
        >
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

        {workflowActive && state.workflow && (
          <WorkflowStrip
            workflow={state.workflow}
            phase={state.phase}
            actionBusy={messageActionBusy}
            onCancel={handleCancel}
            onResume={handleWorkflowResume}
            onRestart={handleWorkflowRestart}
          />
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
            waitSeconds={state.ask.waitSeconds}
            receivedAt={state.ask.receivedAt}
            onSubmit={handleAskSubmit}
          />
        )}


        {selectedPlan && state.phase === "plan_review" && (
          <div
            data-testid="plan-review-notice"
            className="flex shrink-0 items-center gap-3 border-t border-border bg-card/30 px-4 py-2 text-xs"
          >
            <ClipboardList
              className="size-4 shrink-0 text-primary"
              aria-hidden="true"
            />
            <span role="status" className="min-w-0 flex-1 text-foreground">
              {`계획안 (rev ${selectedPlan.revision})이 검토를 기다립니다. 계획 탭에서 승인하거나 아래 입력창에 피드백을 입력하세요.`}
            </span>
            {activeCenterTab !== PLAN_TAB_ID && (
              <Button
                type="button"
                size="sm"
                variant="outline"
                className="h-8"
                onClick={showPlanTab}
              >
                계획 보기
              </Button>
            )}
          </div>
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

        </div>

        {planTabVisible && selectedPlan && (
          <div
            id={`document-panel-${PLAN_TAB_ID}`}
            role="tabpanel"
            aria-labelledby={`document-tab-${PLAN_TAB_ID}`}
            className={
              activeCenterTab === PLAN_TAB_ID
                ? "flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
                : "hidden"
            }
          >
            <PlanView
              plan={selectedPlan}
              artifact={state.workflow?.plan}
              reviewable={state.phase === "plan_review"}
              pending={messageActionBusy}
              onApprove={handlePlanApprove}
            />
          </div>
        )}

        {activeCenterTab !== "chat" &&
          workspaceData &&
          openDocumentTabs.map((path) => {
            const documentState = documentStates[path] ?? {
              content: null,
              loading: false,
              error: "파일을 찾을 수 없습니다. 파일 트리에서 다시 열어 주세요.",
              notice: null,
            };
            return (
              <div
                key={path}
                id={`document-panel-${path}`}
                role="tabpanel"
                aria-labelledby={`document-tab-${path}`}
                className={
                  activeCenterTab === path
                    ? "flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
                    : "hidden"
                }
              >
                <WorkspaceDocument
                  workspace={workspaceData}
                  path={path}
                  file={
                    workspaceData.files.find((file) => file.path === path) ??
                    null
                  }
                  content={documentState.content}
                  loading={documentState.loading}
                  error={documentState.error}
                  notice={documentState.notice}
                  onSelect={openDocumentTab}
                />
              </div>
            );
          })}

        {/* The prompt is shared under every center tab: plan feedback, questions
            about an open report, and ordinary chat all enter here. */}
        <InstructionBox
          state={state}
          onSend={handleSend}
          onMentionSearch={handleMentionSearch}
          projectIdentity={projectState.project}
          scopeIdentity={selectedSlot?.id ?? `draft:${projectState.project}`}
          onStageAttachment={stageAttachment}
          onDiscardAttachment={discardAttachment}
          onCancel={handleCancel}
          autonomousRun={selectedSlot?.autonomousRun ?? null}
          onAutonomousPause={handleAutonomousPause}
          onAutonomousResume={handleAutonomousResume}
          onAutonomousStop={handleAutonomousStop}
          draft={editDraft}
          actionBusy={selectedActionBusy}
          modelSettings={promptModelSettings}
          modelSettingsBusy={!projectPollEnabled || providerSettingsBusy}
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
        workspaceSelectedPath={
          activeCenterTab !== "chat" && activeCenterTab !== PLAN_TAB_ID
            ? activeCenterTab
            : null
        }
        workspaceLoading={workspaceLoading}
        workspaceError={workspaceError}
        onTabChange={(tab) => void handleProjectPanelTab(tab)}
        onClose={() => setProjectSidebarOpen(false)}
        onWikiSave={handleWikiSave}
        onMemoryTabSelected={projectStore.memoryTabSelected}
        onMemoryEdited={projectStore.memoryEdited}
        onMemorySave={handleMemorySave}
        onWorkspaceSelect={openDocumentTab}
        onWorkspaceSearch={handleWorkspaceSearch}
        onWorkspaceRefresh={handleWorkspaceRefresh}
      />
    </div>
  );
}
