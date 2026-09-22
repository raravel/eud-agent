/**
 * Tauri IPC protocol client (typed v2 messages, invoke + events).
 *
 * The panel talks to the core in-process through Tauri IPC only:
 *   - panel -> core commands use `invoke(command, args)`;
 *   - core -> panel push messages use `listen(event, handler)`;
 *   - `status` and `list` are request/response commands whose resolved values
 *     are normalized into server messages and delivered through `onMessage`.
 *
 * The Tauri `invoke` and `listen` functions are injected (constructor seams) so
 * the client is unit-testable headless without a real Tauri runtime.
 */

import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen } from "@tauri-apps/api/event";
import {
  PROVIDER_IDS,
  isProviderId,
  isProviderModel,
  isProviderStatus,
  type ProviderId,
  type ProviderModel,
  type ProviderSettingsView,
  type ProviderStatus,
  type ReasoningSelection,
  type SessionModelSettings,
} from "@/providers/types";
export type {
  ProviderId,
  ProviderModel,
  ProviderSettingsView,
  ProviderProgressEvent,
  ProviderStatus,
  ReasoningSelection,
  SessionModelSettings,
} from "@/providers/types";

import {
  isServerMessage,
  isSetupMessage,
  type ClientMessage,
  type LedgerEntry,
  type BackendSessionActivity,
  type MentionSearchRequest,
  type MentionSearchResponse,
  type RecentProject,
  type ServerMessage,
  type ServerMessageType,
  type SetupMessage,
  type WikiMessage,
  type WorkspaceFileEntry,
  type WorkspaceListResponse,
  type WorkspaceReadResponse,
  type WorkspaceSearchResponse,
} from "./protocol";

export * from "./protocol";

/** Log kinds the client emits via {@link IpcClientOptions.onLog}. */
export type IpcLogKind =
  | "info" // ready / informational
  | "unknown" // an event payload of an unrecognized type/shape
  | "badjson"; // a payload that could not be treated as an object

/** Injectable Tauri invoke seam. */
export type InvokeFn = (
  cmd: string,
  args?: Record<string, unknown>,
) => Promise<unknown>;
export interface NotificationChannelSettings {
  sound: boolean;
  osNotification: boolean;
}

export type NotificationEvent =
  | "planApproval"
  | "changesetReview"
  | "agentTurnComplete"
  | "askResponseRequired";

export interface NotificationSettings {
  planApproval: NotificationChannelSettings;
  changesetReview: NotificationChannelSettings;
  agentTurnComplete: NotificationChannelSettings;
  askResponseRequired: NotificationChannelSettings;
}

export interface AppSettings {
  notifications: NotificationSettings;
  codexLargeContextModels: string[];
  /** Deep planning (planner → architect → critic loop); captured at triage. */
  deepPlanning: boolean;
  /** SCMDraft 2 executable used by the header's "SCMDraft 2로 열기"; empty when unset. */
  scmdraftPath: string;
}

export interface EuddraftSettings {
  path: string;
  valid: boolean;
  managed: boolean;
  installedVersion?: string;
  latestVersion?: string;
  updateAvailable: boolean;
}

export type AttentionNotificationKind = NotificationEvent;



/** Unlisten callback returned by Tauri event registration. */
export type UnlistenFn = () => void;

/** Minimal Tauri event shape the client reads. */
export interface IpcEvent {
  payload: unknown;
}

/** Injectable Tauri listen seam. */
export type ListenFn = (
  event: string,
  handler: (event: IpcEvent) => void,
) => Promise<UnlistenFn>;

export interface IpcClientOptions {
  /** Tauri command invoker. Defaults to `@tauri-apps/api/core` invoke. */
  invoke?: InvokeFn;
  /** Tauri event listener. Defaults to `@tauri-apps/api/event` listen. */
  listen?: ListenFn;
  /** Called for every structurally-valid server message. */
  onMessage: (msg: ServerMessage) => void;
  /** Called for lifecycle + unknown/bad payloads (optional). */
  onLog?: (kind: IpcLogKind, text: string) => void;
  /**
   * Called when the in-process Tauri event transport becomes ready or closes.
   * Native project availability is reported separately.
   */
  onOpenChange?: (open: boolean) => void;
  /**
   * Called when the configured native project changes between readable and
   * unavailable. Fired only on transitions and the first refresh failure.
   */
  onProjectAvailabilityChange?: (available: boolean, detail?: string) => void;
}

const PUSH_EVENT_TYPES = [
  "agent_event",
  "context_usage",
  "answer",
  "plan",
  "ask",
  "delegation",
  "team_task",
  "changeset",
  "workflow",
  "harness_job",
  "rollback_result",
  "progress",
  "error",
  "session_activity",
  "autonomous_run",
  "status",
  "memory",
  "memory_saved",
  "wiki",
] as const satisfies readonly ServerMessageType[];

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function formatError(error: unknown): string {
  if (error instanceof Error) return error.message;
  return String(error);
}

/**
 * Contractual no-project marker. A configured native root may be temporarily
 * unavailable or may not yet expose a source list. Treat that as project state,
 * not as a transport failure.
 */
const NO_PROJECT_MARKER = "no project";

/**
 * Stateful IPC client. Construct, then call {@link IpcClient.connect}. The
 * client owns listener registration; callers drive commands via
 * {@link IpcClient.send}.
 */
export class IpcClient {
  private readonly invoke: InvokeFn;
  private readonly listen: ListenFn;
  private readonly onMessage: (msg: ServerMessage) => void;
  private readonly onLog: (kind: IpcLogKind, text: string) => void;
  private readonly onOpenChange: (open: boolean) => void;
  private readonly onProjectAvailabilityChange: (
    available: boolean,
    detail?: string,
  ) => void;

  private unlisteners: UnlistenFn[] = [];
  private active = false;
  private open = false;
  // Native-project tracking for refresh(). The last project identity avoids
  // reloading the source list on every periodic status refresh.
  private projectUp = false;
  private projectProbed = false;
  private lastProject: string | undefined;
  private projectGeneration = 0;

  constructor(options: IpcClientOptions) {
    this.invoke =
      options.invoke ??
      ((cmd, args) => tauriInvoke(cmd, args));
    this.listen =
      options.listen ??
      ((event, handler) => tauriListen(event, handler));
    this.onMessage = options.onMessage;
    this.onLog = options.onLog ?? (() => {});
    this.onOpenChange = options.onOpenChange ?? (() => {});
    this.onProjectAvailabilityChange =
      options.onProjectAvailabilityChange ?? (() => {});
  }

  /**
   * Register push-event listeners and mark the in-process transport open.
   * Bootstrap progress can flow before a native project has been configured;
   * project availability is resolved independently by {@link IpcClient.refresh}.
   */
  async connect(): Promise<void> {
    if (this.active) return;
    this.active = true;
    try {
      await this.registerListeners();
      if (!this.active) return;
      this.open = true;
      this.onOpenChange(true);
    } catch (error) {
      if (!this.active) return;
      this.stop();
      this.onOpenChange(false);
      this.onLog("unknown", `IPC connect failed: ${formatError(error)}`);
    }
  }

  /**
   * Refresh the native project snapshot without changing transport state.
   * `status` validates the configured project and reports its build state;
   * `list` reloads only after recovery or a project identity change.
   */
  async refresh(): Promise<boolean> {
    if (!this.active) return false;
    const generation = this.projectGeneration;
    let status: unknown;
    try {
      status = await this.invoke("status");
    } catch (error) {
      if (generation !== this.projectGeneration) return false;
      if (!this.active) return false;
      const wasUp = this.projectUp;
      this.projectUp = false;
      this.lastProject = undefined;
      if (wasUp || !this.projectProbed) {
        this.projectProbed = true;
        this.onProjectAvailabilityChange(false, formatError(error));
      }
      return false;
    }
    if (!this.active) return false;
    if (generation !== this.projectGeneration) return false;
    this.dispatchPayload("status", status);
    const project =
      isObject(status) && typeof status.project === "string"
        ? status.project
        : undefined;
    const wasUp = this.projectUp;
    const needList = !wasUp || project !== this.lastProject;
    this.projectUp = true;
    this.lastProject = project;
    if (needList) {
      try {
        const list = await this.invoke("list");
        if (generation !== this.projectGeneration) return false;
        if (this.active) this.dispatchPayload("list", list);
      } catch (error) {
        if (generation !== this.projectGeneration) return false;
        if (this.active) {
          const detail = formatError(error);
          if (detail.toLowerCase().includes(NO_PROJECT_MARKER)) {
            // No configured/open project: feed the store's no-project state
            // without treating it as an IPC transport failure.
            this.dispatchPayload("list", { error: detail });
          } else {
            this.onLog("unknown", `IPC command failed (list): ${detail}`);
          }
        }
      }
    }
    if (!wasUp) {
      this.projectProbed = true;
      this.onProjectAvailabilityChange(true);
    }
    return true;
  }

  private async registerListeners(): Promise<void> {
    const unlisteners: UnlistenFn[] = [];
    try {
      for (const type of PUSH_EVENT_TYPES) {
        unlisteners.push(
          await this.listen(type, (event) =>
            this.dispatchPayload(type, event.payload),
          ),
        );
      }
    } catch (error) {
      for (const unlisten of unlisteners) unlisten();
      throw error;
    }
    if (!this.active) {
      for (const unlisten of unlisteners) unlisten();
      return;
    }
    this.unlisteners.push(...unlisteners);
  }

  private dispatchPayload(type: ServerMessageType, payload: unknown): void {
    if (!isObject(payload)) {
      this.onLog("badjson", `Bad IPC payload for ${type}.`);
      return;
    }
    const candidate = { ...payload, type };
    if (isServerMessage(candidate)) {
      this.onMessage(candidate);
      return;
    }
    this.onLog("unknown", `Unknown IPC message payload for ${type}.`);
  }

  private commandArgs(msg: ClientMessage): Record<string, unknown> {
    switch (msg.type) {
      case "chat":
        return {
          sessionId: msg.sessionId,
          clientTurnId: msg.clientTurnId,
          text: msg.text,
          attachments: msg.attachments,
          mentions: msg.mentions ?? [],
          executionMode: msg.executionMode ?? "interactive",
          ...(msg.autonomousPolicy
            ? { autonomousPolicy: msg.autonomousPolicy }
            : {}),
        };
      case "plan_feedback":
        return {
          sessionId: msg.sessionId,
          clientTurnId: msg.clientTurnId,
          text: msg.text,
          attachments: msg.attachments,
          mentions: msg.mentions ?? [],
        };
      case "plan_approve":
        return { sessionId: msg.sessionId };
      case "workflow_resume":
      case "workflow_restart":
        return { sessionId: msg.sessionId };
      case "ask_response":
        return {
          sessionId: msg.sessionId,
          requestId: msg.requestId,
          answers: msg.answers,
        };
      case "changeset_decision":
        return {
          sessionId: msg.sessionId,
          decision: msg.decision,
          ids: msg.ids,
        };
      case "cancel":
        return { sessionId: msg.sessionId };
      case "autonomous_pause":
      case "autonomous_resume":
      case "autonomous_stop":
        return { sessionId: msg.sessionId };
      case "conversation_rewind":
        return { sessionId: msg.sessionId, panelLog: msg.panelLog };
      case "status":
        return {};
      case "list":
        return {};
      case "memory_get":
        return {};
      case "memory_save":
        return { file: msg.file, content: msg.content };
      case "setup_status":
        return {};
      case "setup_pick_project_path":
        return msg.directory === undefined ? {} : { directory: msg.directory };
      case "setup_create_project":
        return {};
      case "setup_import_e3s": {
        const request: Record<string, unknown> = {
          sourceE3s: msg.sourceE3s,
          destination: msg.destination,
        };
        if (msg.excludedImportItems !== undefined) {
          request.excludedImportItems = msg.excludedImportItems;
        }
        return { request };
      }
      case "setup_pick_euddraft_path":
        return msg.directory === undefined ? {} : { directory: msg.directory };
      case "setup_install_euddraft":
        return {};
      case "bootstrap_run":
        return {};
      default: {
        const _exhaustive: never = msg;
        return _exhaustive;
      }
    }
  }

  /**
   * Send a client command. Returns true if the command resolved; false (no
   * throw) if the invocation failed. `status` and `list` responses are surfaced
   * to `onMessage` as normalized server messages.
   */
  async send(msg: ClientMessage): Promise<boolean> {
    try {
      const result = await this.invoke(msg.type, this.commandArgs(msg));
      if (msg.type === "status" || msg.type === "list") {
        this.dispatchPayload(msg.type, result);
      } else if (msg.type === "memory_get") {
        this.dispatchPayload("memory", result);
      } else if (msg.type === "memory_save") {
        this.dispatchPayload("memory_saved", result);
      } else if (
        msg.type === "setup_status" ||
        msg.type === "setup_pick_project_path" ||
        msg.type === "setup_create_project" ||
        msg.type === "setup_import_e3s" ||
        msg.type === "setup_pick_euddraft_path" ||
        msg.type === "setup_install_euddraft"
      ) {
        this.dispatchPayload("setup", result);
      }
      return true;
    } catch (error) {
      const detail = formatError(error);
      const text =
        msg.type === "chat" || msg.type === "plan_feedback"
          ? `요청을 처리하지 못했습니다: ${detail}`
          : `IPC command failed (${msg.type}): ${detail}`;
      this.onLog("unknown", text);
      return false;
    }
  }

  /** True iff the transport is open (push listeners registered). */
  isOpen(): boolean {
    return this.open;
  }

  /**
   * Stop listening to Tauri events. Native project recovery is driven by the
   * App's periodic {@link IpcClient.refresh}; transport has no reconnect timer.
   */
  stop(): void {
    this.active = false;
    this.open = false;
    this.projectUp = false;
    this.projectProbed = false;
    this.lastProject = undefined;
    const unlisteners = this.unlisteners.splice(0);
    for (const unlisten of unlisteners) {
      try {
        unlisten();
      } catch {
        // ignore - listener may already be removed
      }
    }
  }
  /** Force the next refresh to reload status and source data after a project switch. */
  invalidateProject(): void {
    this.projectGeneration += 1;
    this.projectUp = false;
    this.projectProbed = false;
    this.lastProject = undefined;
  }
}

function toMentionSearchResponse(value: unknown): MentionSearchResponse {
  if (
    !isObject(value) ||
    value.schema !== "eud-mention-search/1" ||
    !Array.isArray(value.results) ||
    typeof value.truncated !== "boolean"
  ) {
    throw new Error("invalid mention search response");
  }
  return value as unknown as MentionSearchResponse;
}

/** Search current backend-owned resources without exposing an unbounded catalog. */
export async function mentionSearch(
  request: MentionSearchRequest,
  invoke: InvokeFn = tauriInvoke,
): Promise<MentionSearchResponse> {
  return toMentionSearchResponse(await invoke("mention_search", { request }));
}

/**
 * Normalize a `WikiResponse` (`{version, entries}`) into a `WikiMessage` (adds
 * the `type` discriminant). The Rust `wiki_get`/`wiki_save` commands and the
 * pushed `wiki` event carry the same shape; tagging it lets the App route it
 * through the same `onMessage`/store path as the push event.
 */
function toWikiMessage(value: unknown): WikiMessage {
  const obj = isObject(value) ? value : {};
  const version = typeof obj.version === "number" ? obj.version : 1;
  const entries =
    isObject(obj.entries) && obj.entries !== null
      ? (obj.entries as Record<string, LedgerEntry>)
      : {};
  return { type: "wiki", version, entries };
}

/**
 * `wiki_get` command: fetch the current dat-edit ledger for the open project.
 * Resolves to a `WikiMessage` (the App feeds it to `store.wikiReceived`). The
 * default `invoke` is used unless one is injected (tests).
 */
export async function wikiGet(invoke: InvokeFn = tauriInvoke): Promise<WikiMessage> {
  return toWikiMessage(await invoke("wiki_get"));
}

/**
 * `wiki_save` command: persist user-corrected ledger entries (the core flips
 * `editedByUser=true`) and resolve to the refreshed `WikiMessage`.
 */
export async function wikiSave(
  entries: Record<string, LedgerEntry>,
  invoke: InvokeFn = tauriInvoke,
): Promise<WikiMessage> {
  return toWikiMessage(await invoke("wiki_save", { entries }));
}

function toWorkspaceList(value: unknown): WorkspaceListResponse {
  if (
    !isObject(value) ||
    typeof value.project !== "string" ||
    typeof value.workspaceId !== "string" ||
    !Array.isArray(value.files)
  ) {
    throw new Error("invalid workspace list response");
  }
  const files = value.files.filter(
    (entry): entry is WorkspaceFileEntry =>
      isObject(entry) &&
      typeof entry.path === "string" &&
      typeof entry.size === "number" &&
      (entry.state === undefined || typeof entry.state === "string") &&
      (entry.revision === undefined || typeof entry.revision === "number"),
  );
  if (files.length !== value.files.length) {
    throw new Error("invalid workspace file entry");
  }
  return {
    project: value.project,
    workspaceId: value.workspaceId,
    files,
  };
}

/** List the current project's accepted, project-local harness documents. */
export async function workspaceList(
  invoke: InvokeFn = tauriInvoke,
): Promise<WorkspaceListResponse> {
  return toWorkspaceList(await invoke("workspace_list"));
}

/** Read one confined project file for the viewer (text, or why it stays closed). */
export async function workspaceRead(
  workspaceId: string,
  path: string,
  invoke: InvokeFn = tauriInvoke,
): Promise<WorkspaceReadResponse> {
  const value = await invoke("workspace_read", { workspaceId, path });
  if (
    !isObject(value) ||
    value.workspaceId !== workspaceId ||
    value.path !== path ||
    typeof value.size !== "number" ||
    !(typeof value.content === "string" || value.content === null) ||
    !(
      value.unreadable === undefined ||
      value.unreadable === "binary" ||
      value.unreadable === "too_large"
    ) ||
    (value.content === null) === (value.unreadable === undefined)
  ) {
    throw new Error("invalid workspace read response");
  }
  return value as unknown as WorkspaceReadResponse;
}

/** Search confined UTF-8 workspace files by path and content. */
export async function workspaceSearch(
  workspaceId: string,
  query: string,
  invoke: InvokeFn = tauriInvoke,
): Promise<WorkspaceSearchResponse> {
  const value = await invoke("workspace_search", { workspaceId, query });
  if (
    !isObject(value) ||
    value.workspaceId !== workspaceId ||
    value.query !== query ||
    !Array.isArray(value.paths) ||
    !value.paths.every((path) => typeof path === "string")
  ) {
    throw new Error("invalid workspace search response");
  }
  return {
    workspaceId,
    query,
    paths: value.paths,
  };
}

function toProviderStatus(value: unknown): ProviderStatus {
  if (!isProviderStatus(value)) {
    throw new Error("invalid provider status response");
  }
  return value;
}

function toProviderModels(value: unknown): ProviderModel[] {
  if (!Array.isArray(value) || !value.every(isProviderModel)) {
    throw new Error("invalid provider catalog response");
  }
  return value;
}

function toProviderSettingsView(value: unknown): ProviderSettingsView {
  if (
    !isObject(value) ||
    !isProviderId(value.provider) ||
    !isProviderStatus(value.status) ||
    !Array.isArray(value.models) ||
    !value.models.every(isProviderModel) ||
    (value.selectedModel !== undefined &&
      value.selectedModel !== null &&
      typeof value.selectedModel !== "string") ||
    (value.selectedReasoning !== undefined &&
      value.selectedReasoning !== null &&
      (!isObject(value.selectedReasoning) ||
        typeof value.selectedReasoning.level !== "string")) ||
    (value.version !== undefined &&
      value.version !== null &&
      typeof value.version !== "string") ||
    (value.channel !== undefined &&
      value.channel !== null &&
      typeof value.channel !== "string") ||
    (value.baseUrl !== undefined &&
      value.baseUrl !== null &&
      typeof value.baseUrl !== "string") ||
    typeof value.hasApiKey !== "boolean"
  ) {
    throw new Error("invalid provider settings response");
  }
  return value as unknown as ProviderSettingsView;
}

function toSessionModelSettings(value: unknown): SessionModelSettings {
  if (
    !isObject(value) ||
    !isProviderId(value.provider) ||
    !Array.isArray(value.models) ||
    !value.models.every(isProviderModel) ||
    typeof value.selectedModel !== "string" ||
    (value.selectedReasoning !== undefined &&
      value.selectedReasoning !== null &&
      (!isObject(value.selectedReasoning) ||
        typeof value.selectedReasoning.level !== "string"))
  ) {
    throw new Error("invalid session model settings response");
  }
  return {
    provider: value.provider,
    models: value.models as ProviderModel[],
    selectedModel: value.selectedModel,
    selectedReasoning:
      value.selectedReasoning === null ||
      value.selectedReasoning === undefined
        ? undefined
        : (value.selectedReasoning as unknown as ReasoningSelection),
  };
}

function parseSetupResponse(value: unknown, command: string): SetupMessage {
  if (isObject(value)) {
    const candidate = { ...value, type: "setup" };
    if (isSetupMessage(candidate)) return candidate;
  }
  throw new Error(`invalid response from ${command}`);
}

function parseRecentProject(value: unknown): RecentProject {
  if (
    !isObject(value) ||
    typeof value.name !== "string" ||
    typeof value.path !== "string" ||
    typeof value.lastOpenedAt !== "number" ||
    typeof value.available !== "boolean"
  ) {
    throw new Error("invalid response from project_recent_list");
  }
  return {
    name: value.name,
    path: value.path,
    lastOpenedAt: value.lastOpenedAt,
    available: value.available,
  };
}

export async function setupProjectOpen(
  path: string,
  invoke: InvokeFn = tauriInvoke,
): Promise<SetupMessage> {
  return parseSetupResponse(await invoke("project_open", { path }), "project_open");
}

export async function setupPickProjectPath(
  directory = false,
  invoke: InvokeFn = tauriInvoke,
): Promise<SetupMessage> {
  return parseSetupResponse(
    await invoke("setup_pick_project_path", directory ? { directory: true } : {}),
    "setup_pick_project_path",
  );
}
export async function setupCreateProject(
  invoke: InvokeFn = tauriInvoke,
): Promise<SetupMessage> {
  return parseSetupResponse(await invoke("setup_create_project"), "setup_create_project");
}

export async function projectRecentList(
  invoke: InvokeFn = tauriInvoke,
): Promise<RecentProject[]> {
  const value = await invoke("project_recent_list");
  if (!Array.isArray(value)) throw new Error("invalid response from project_recent_list");
  return value.map(parseRecentProject);
}

export async function projectRecentRemove(
  path: string,
  invoke: InvokeFn = tauriInvoke,
): Promise<RecentProject[]> {
  const value = await invoke("project_recent_remove", { path });
  if (!Array.isArray(value)) throw new Error("invalid response from project_recent_remove");
  return value.map(parseRecentProject);
}

export async function projectTakeLaunchRequest(
  invoke: InvokeFn = tauriInvoke,
): Promise<string | null> {
  const value = await invoke("project_take_launch_request");
  if (value === null) return null;
  if (isObject(value) && typeof value.path === "string" && value.path.trim() !== "") {
    return value.path;
  }
  throw new Error("invalid response from project_take_launch_request");
}



export interface E3sExportResponse {
  path: string;
}

export async function projectExportE3s(
  invoke: InvokeFn = tauriInvoke,
): Promise<E3sExportResponse | null> {
  const value = await invoke("project_export_e3s");
  if (value === null) return null;
  if (
    !isObject(value) ||
    typeof value.path !== "string" ||
    value.path.trim().length === 0
  ) {
    throw new Error("invalid E3S export response");
  }
  return { path: value.path };
}

export async function providerSettingsGet(
  provider: ProviderId,
  invoke: InvokeFn = tauriInvoke,
): Promise<ProviderSettingsView> {
  return toProviderSettingsView(
    await invoke("provider_settings", { provider }),
  );
}

export async function providerStatusList(
  invoke: InvokeFn = tauriInvoke,
): Promise<ProviderStatus[]> {
  const value = await invoke("provider_status_list");
  if (!Array.isArray(value) || value.length !== PROVIDER_IDS.length) {
    throw new Error("invalid provider status list response");
  }
  return value.map(toProviderStatus);
}

export async function providerInstall(
  provider: ProviderId,
  invoke: InvokeFn = tauriInvoke,
): Promise<ProviderStatus> {
  return toProviderStatus(await invoke("provider_install", { provider }));
}

export async function providerLoginStart(
  provider: ProviderId,
  invoke: InvokeFn = tauriInvoke,
): Promise<string> {
  const value = await invoke("provider_login_start", { provider });
  if (typeof value !== "string") throw new Error("invalid provider login attempt");
  return value;
}

export async function providerLoginCancel(
  provider: ProviderId,
  attemptId: string,
  invoke: InvokeFn = tauriInvoke,
): Promise<void> {
  await invoke("provider_login_cancel", { provider, attemptId });
}

export async function providerLoginStatus(
  provider: ProviderId,
  invoke: InvokeFn = tauriInvoke,
): Promise<ProviderStatus> {
  return toProviderStatus(await invoke("provider_login_status", { provider }));
}

export async function providerCredentialImport(
  provider: ProviderId,
  invoke: InvokeFn = tauriInvoke,
): Promise<ProviderStatus> {
  return toProviderStatus(
    await invoke("provider_credential_import", { provider }),
  );
}

export async function providerApiKeySave(
  provider: ProviderId,
  key: string,
  invoke: InvokeFn = tauriInvoke,
): Promise<ProviderStatus> {
  return toProviderStatus(
    await invoke("provider_api_key_save", { provider, key }),
  );
}

export async function providerBaseUrlSave(
  provider: ProviderId,
  baseUrl: string,
  invoke: InvokeFn = tauriInvoke,
): Promise<ProviderSettingsView> {
  return toProviderSettingsView(
    await invoke("provider_base_url_save", { provider, baseUrl }),
  );
}

export async function providerLogout(
  provider: ProviderId,
  invoke: InvokeFn = tauriInvoke,
): Promise<ProviderStatus> {
  return toProviderStatus(await invoke("provider_logout", { provider }));
}

export async function providerCatalog(
  provider: ProviderId,
  invoke: InvokeFn = tauriInvoke,
): Promise<ProviderModel[]> {
  return toProviderModels(await invoke("provider_catalog", { provider }));
}

export async function providerDefaultsSave(
  provider: ProviderId,
  model: string,
  reasoning: ReasoningSelection | undefined,
  setDefaultProvider: boolean,
  invoke: InvokeFn = tauriInvoke,
): Promise<ProviderSettingsView> {
  return toProviderSettingsView(
    await invoke("provider_defaults_save", {
      provider,
      model,
      reasoning,
      setDefaultProvider,
    }),
  );
}

export async function sessionModelSettingsGet(
  sessionId: string,
  invoke: InvokeFn = tauriInvoke,
): Promise<SessionModelSettings> {
  return toSessionModelSettings(
    await invoke("session_model_settings", { sessionId }),
  );
}

export async function sessionModelSettingsSave(
  sessionId: string,
  model: string,
  reasoning: ReasoningSelection | undefined,
  invoke: InvokeFn = tauriInvoke,
): Promise<SessionModelSettings> {
  return toSessionModelSettings(
    await invoke("session_model_settings_save", {
      sessionId,
      model,
      reasoning,
    }),
  );
}

export async function setupProviderSelect(
  provider: ProviderId,
  invoke: InvokeFn = tauriInvoke,
): Promise<unknown> {
  return invoke("setup_provider_select", { provider });
}

/** Run native Codex compaction for one persisted session. */
export async function compactSession(
  sessionId: string,
  invoke: InvokeFn = tauriInvoke,
): Promise<void> {
  await invoke("compact", { sessionId });
}

function isNotificationChannelSettings(
  value: unknown,
): value is NotificationChannelSettings {
  return (
    isObject(value) &&
    typeof value.sound === "boolean" &&
    typeof value.osNotification === "boolean"
  );
}

function toAppSettings(value: unknown): AppSettings {
  if (
    !isObject(value) ||
    !isObject(value.notifications) ||
    !isNotificationChannelSettings(value.notifications.planApproval) ||
    !isNotificationChannelSettings(value.notifications.changesetReview) ||
    !isNotificationChannelSettings(value.notifications.agentTurnComplete) ||
    !isNotificationChannelSettings(value.notifications.askResponseRequired) ||
    !Array.isArray(value.codexLargeContextModels) ||
    !value.codexLargeContextModels.every(
      (model) => typeof model === "string" && model.trim().length > 0,
    ) ||
    typeof value.deepPlanning !== "boolean" ||
    (value.scmdraftPath !== undefined && typeof value.scmdraftPath !== "string")
  ) {
    throw new Error("invalid app settings response");
  }
  // Older backends omit the SCMDraft path; normalize so every save carries the field.
  return {
    ...(value as unknown as AppSettings),
    scmdraftPath: typeof value.scmdraftPath === "string" ? value.scmdraftPath : "",
  };
}

function toEuddraftSettings(value: unknown): EuddraftSettings {
  if (
    !isObject(value) ||
    typeof value.path !== "string" ||
    typeof value.valid !== "boolean" ||
    typeof value.managed !== "boolean" ||
    (value.installedVersion !== undefined &&
      (typeof value.installedVersion !== "string" ||
        value.installedVersion.trim().length === 0)) ||
    (value.latestVersion !== undefined &&
      (typeof value.latestVersion !== "string" ||
        value.latestVersion.trim().length === 0)) ||
    typeof value.updateAvailable !== "boolean" ||
    value.managed !== (typeof value.installedVersion === "string") ||
    (value.updateAvailable &&
      (typeof value.latestVersion !== "string" ||
        value.installedVersion === value.latestVersion))
  ) {
    throw new Error("invalid euddraft settings response");
  }
  return value as unknown as EuddraftSettings;
}

/** Read the configured euddraft path and managed-install version. */
export async function euddraftSettingsGet(
  invoke: InvokeFn = tauriInvoke,
): Promise<EuddraftSettings> {
  return toEuddraftSettings(await invoke("euddraft_settings"));
}

/** Fetch GitHub's latest official euddraft release and compare it locally. */
export async function euddraftCheckUpdate(
  invoke: InvokeFn = tauriInvoke,
): Promise<EuddraftSettings> {
  return toEuddraftSettings(await invoke("euddraft_check_update"));
}

/** Install and select GitHub's latest official managed euddraft release. */
export async function euddraftUpdate(
  invoke: InvokeFn = tauriInvoke,
): Promise<EuddraftSettings> {
  return toEuddraftSettings(await invoke("euddraft_update"));
}

/** Fetch app-owned preferences for the extensible settings dialog. */
export async function appSettingsGet(
  invoke: InvokeFn = tauriInvoke,
): Promise<AppSettings> {
  return toAppSettings(await invoke("app_settings"));
}

/** Persist app-owned preferences without replacing unrelated core config. */
export async function appSettingsSave(
  settings: AppSettings,
  invoke: InvokeFn = tauriInvoke,
): Promise<AppSettings> {
  return toAppSettings(await invoke("app_settings_save", { settings }));
}

/** Open the SCMDraft 2 executable picker; the backend persists the choice. Null when cancelled. */
export async function pickScmdraftPath(
  invoke: InvokeFn = tauriInvoke,
): Promise<string | null> {
  const value = await invoke("settings_pick_scmdraft_path");
  if (value === null || value === undefined) return null;
  if (typeof value !== "string") throw new Error("invalid scmdraft path response");
  return value;
}

/** Outcome of "SCMDraft 2로 열기": launched, or the executable is not configured yet. */
export type ScmdraftLaunch = { kind: "launched" } | { kind: "unconfigured" };

/** Launch the configured SCMDraft 2 on the current project's source map. */
export async function openScmdraft(invoke: InvokeFn = tauriInvoke): Promise<ScmdraftLaunch> {
  const value = await invoke("project_open_scmdraft");
  const kind = isObject(value) ? value.kind : undefined;
  if (kind !== "launched" && kind !== "unconfigured") {
    throw new Error("invalid scmdraft launch response");
  }
  return { kind };
}

/** Play the native Windows sound used by attention notifications. */
export async function notificationSoundPreview(
  invoke: InvokeFn = tauriInvoke,
): Promise<void> {
  await invoke("notification_sound_preview");
}
/** True only when an active agent turn settles without entering a dedicated review state. */
export function isAgentTurnEndTransition(
  previous: BackendSessionActivity,
  current: BackendSessionActivity,
): boolean {
  const wasRunning =
    previous === "running_read" || previous === "running_write";
  return wasRunning && (current === "idle" || current === "error");
}


/** Deliver a user-attention event through the channels enabled in persisted settings. */
export async function attentionNotify(
  kind: AttentionNotificationKind,
  showOs: boolean,
  sessionId: string,
  itemCount?: number,
  invoke: InvokeFn = tauriInvoke,
): Promise<void> {
  await invoke("attention_notify", {
    kind,
    showOs,
    sessionId,
    ...(itemCount === undefined ? {} : { itemCount }),
  });
}
