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
  isServerMessage,
  type ClientMessage,
  type ServerMessage,
  type ServerMessageType,
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
  /** Called when the IPC client becomes ready / not ready (optional). */
  onOpenChange?: (open: boolean) => void;
}

const PUSH_EVENT_TYPES = [
  "agent_event",
  "answer",
  "plan",
  "changeset",
  "rollback_result",
  "progress",
  "error",
  "status",
  "memory",
  "memory_saved",
] as const satisfies readonly ServerMessageType[];

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function formatError(error: unknown): string {
  if (error instanceof Error) return error.message;
  return String(error);
}

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

  private unlisteners: UnlistenFn[] = [];
  private active = false;
  private open = false;

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
  }

  /**
   * Register push-event listeners, request the initial status/list snapshots,
   * then report readiness. There is no reconnect loop for in-process IPC.
   */
  async connect(): Promise<void> {
    if (this.active) return;
    this.active = true;
    try {
      await this.registerListeners();
      if (!this.active) return;
      const [status, list] = await Promise.all([
        this.invoke("status"),
        this.invoke("list"),
      ]);
      if (!this.active) return;
      this.dispatchPayload("status", status);
      this.dispatchPayload("list", list);
      this.open = true;
      this.onOpenChange(true);
      this.onLog("info", "IPC client ready.");
    } catch (error) {
      if (!this.active) return;
      this.stop();
      this.onOpenChange(false);
      this.onLog("unknown", `IPC connect failed: ${formatError(error)}`);
    }
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
        return { text: msg.text };
      case "plan_feedback":
        return { text: msg.text };
      case "plan_approve":
        return {};
      case "changeset_decision":
        return { decision: msg.decision, ids: msg.ids };
      case "cancel":
        return {};
      case "reset":
        return {};
      case "status":
        return {};
      case "list":
        return {};
      case "memory_get":
        return {};
      case "memory_save":
        return { file: msg.file, content: msg.content };
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
      }
      return true;
    } catch (error) {
      this.onLog(
        "unknown",
        `IPC command failed (${msg.type}): ${formatError(error)}`,
      );
      return false;
    }
  }

  /** True iff the client has completed its initial IPC readiness handshake. */
  isOpen(): boolean {
    return this.open;
  }

  /** Stop listening to Tauri events. No reconnect or retry timer exists. */
  stop(): void {
    this.active = false;
    this.open = false;
    const unlisteners = this.unlisteners.splice(0);
    for (const unlisten of unlisteners) {
      try {
        unlisten();
      } catch {
        // ignore - listener may already be removed
      }
    }
  }
}
