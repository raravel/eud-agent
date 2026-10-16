/**
 * The remote page's transport: commands over HTTP, events over one shared
 * WebSocket, both to the desktop app's `/remote/api` (see `remote/server.rs`).
 *
 * The socket names its connection in a `hello`; every command carries that
 * connection in `x-eud-remote-conn`, which is how the app resolves the project
 * the phone attached to. A dropped socket reconnects with backoff, re-attaches
 * the same project, and then emits the synthetic `remote_resync` event so the
 * page reloads what it may have missed. Errors reject with the same string the
 * desktop invoke rejects with.
 */
import type { EventCallback, UnlistenFn } from "@tauri-apps/api/event";
import type { InvokeArgs, InvokeOptions } from "@tauri-apps/api/core";

export const CONNECTION_HEADER = "x-eud-remote-conn";
/** Emitted after a reconnect, or when the app says events were dropped. */
export const RESYNC_EVENT = "remote_resync";
/** Emitted when the attached project stopped being available. */
export const DETACHED_EVENT = "remote_detached";

const API = "/remote/api";
const DEFAULT_RECONNECT_DELAYS = [500, 1_000, 2_000, 5_000, 10_000];
const DEFAULT_CONNECTION_TIMEOUT_MS = 15_000;

export const DISCONNECTED_MESSAGE =
  "PC와 연결되지 않았습니다. PC에서 앱이 실행 중인지, 네트워크가 연결되어 있는지 확인해 주세요.";
export const UNAUTHORIZED_MESSAGE =
  "연결된 기기가 아닙니다. PC의 설정 → 원격 접속에서 기기를 다시 연결해 주세요.";

export type RemoteStatus = "connecting" | "open" | "closed" | "unauthorized";

export interface RemoteProject {
  projectId: string;
  name: string;
  root: string;
}

/** The part of `WebSocket` the transport uses. */
export interface SocketLike {
  onopen: ((event: unknown) => void) | null;
  onmessage: ((event: { data: unknown }) => void) | null;
  onclose: ((event: unknown) => void) | null;
  onerror: ((event: unknown) => void) | null;
  send(data: string): void;
  close(): void;
}

export type FetchFn = (input: string, init?: RequestInit) => Promise<Response>;

export interface RemoteTransportOptions {
  fetch?: FetchFn;
  createSocket?: (url: string) => SocketLike;
  /** `ws(s)://host/remote/api/events`; derived from the page location by default. */
  socketUrl?: string;
  reconnectDelays?: readonly number[];
  connectionTimeoutMs?: number;
}

type Handler = (event: { event: string; id: number; payload: unknown }) => void;

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function defaultSocketUrl(): string {
  const scheme = window.location.protocol === "https:" ? "wss" : "ws";
  return `${scheme}://${window.location.host}${API}/events`;
}

export class RemoteTransport {
  private readonly fetchFn: FetchFn;
  private readonly createSocket: (url: string) => SocketLike;
  private readonly socketUrl: () => string;
  private readonly reconnectDelays: readonly number[];
  private readonly connectionTimeoutMs: number;

  private socket: SocketLike | null = null;
  private connId: string | null = null;
  private waiters: Array<{ resolve(conn: string): void; reject(error: string): void }> = [];
  private readonly listeners = new Map<string, Set<Handler>>();
  private readonly statusListeners = new Set<(status: RemoteStatus) => void>();
  private status: RemoteStatus = "closed";
  private attachedProject: string | null = null;
  private connectedBefore = false;
  /** A reconnect waits for its re-attach before announcing the resync. */
  private resyncPending = false;
  private attempt = 0;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private started = false;

  constructor(options: RemoteTransportOptions = {}) {
    this.fetchFn = options.fetch ?? ((input, init) => fetch(input, init));
    this.createSocket =
      options.createSocket ?? ((url) => new WebSocket(url) as unknown as SocketLike);
    this.socketUrl = options.socketUrl ? () => options.socketUrl as string : defaultSocketUrl;
    this.reconnectDelays = options.reconnectDelays ?? DEFAULT_RECONNECT_DELAYS;
    this.connectionTimeoutMs = options.connectionTimeoutMs ?? DEFAULT_CONNECTION_TIMEOUT_MS;
  }

  /** Open the event socket (idempotent). */
  start(): void {
    if (this.started) return;
    this.started = true;
    this.openSocket();
  }

  /** Close the socket, forget the project, and stop reconnecting. */
  stop(): void {
    this.started = false;
    if (this.reconnectTimer !== null) clearTimeout(this.reconnectTimer);
    this.reconnectTimer = null;
    const socket = this.socket;
    this.socket = null;
    this.connId = null;
    this.attachedProject = null;
    this.connectedBefore = false;
    this.resyncPending = false;
    this.attempt = 0;
    socket?.close();
    this.rejectWaiters(DISCONNECTED_MESSAGE);
    this.setStatus("closed");
  }

  currentStatus(): RemoteStatus {
    return this.status;
  }

  onStatus(listener: (status: RemoteStatus) => void): () => void {
    this.statusListeners.add(listener);
    return () => this.statusListeners.delete(listener);
  }

  attachedProjectId(): string | null {
    return this.attachedProject;
  }

  /** Work on the open project `projectId`; kept across reconnects. */
  async attach(projectId: string): Promise<RemoteProject> {
    const project = await this.invoke<RemoteProject>("remote_attach", { projectId });
    this.attachedProject = projectId;
    return project;
  }

  async detach(): Promise<void> {
    this.attachedProject = null;
    await this.invoke("remote_detach");
  }

  /** Register `handler` for `event`, like a Tauri listener on this window. */
  listen<T>(event: string, handler: EventCallback<T>): Promise<UnlistenFn> {
    let handlers = this.listeners.get(event);
    if (!handlers) {
      handlers = new Set();
      this.listeners.set(event, handlers);
    }
    const entry = handler as unknown as Handler;
    handlers.add(entry);
    return Promise.resolve(() => {
      this.listeners.get(event)?.delete(entry);
    });
  }

  /** Run one command; `attachment_stage` takes the raw bytes and headers. */
  async invoke<T>(cmd: string, args?: InvokeArgs, options?: InvokeOptions): Promise<T> {
    const raw = args instanceof Uint8Array || args instanceof ArrayBuffer || Array.isArray(args);
    if (raw !== (cmd === "attachment_stage")) {
      throw "원격 접속에서는 사용할 수 없는 기능입니다. PC에서 진행해 주세요.";
    }
    const headers: Record<string, string> = {};
    if (cmd !== "remote_projects") headers[CONNECTION_HEADER] = await this.connection();
    let url: string;
    let body: BodyInit;
    if (raw) {
      url = `${API}/attachment_stage`;
      const extra = options?.headers;
      if (extra instanceof Headers) extra.forEach((value, key) => (headers[key] = value));
      else if (Array.isArray(extra)) for (const [key, value] of extra) headers[key] = value;
      else if (extra) Object.assign(headers, extra);
      const bytes = args instanceof ArrayBuffer ? new Uint8Array(args) : new Uint8Array(args as Uint8Array | number[]);
      body = new Blob([bytes]);
    } else {
      url = `${API}/invoke/${encodeURIComponent(cmd)}`;
      headers["content-type"] = "application/json";
      body = JSON.stringify(args ?? {});
    }
    let response: Response;
    try {
      response = await this.fetchFn(url, {
        method: "POST",
        credentials: "same-origin",
        headers,
        body,
      });
    } catch {
      throw DISCONNECTED_MESSAGE;
    }
    return this.readResult<T>(response);
  }

  private async readResult<T>(response: Response): Promise<T> {
    const type = response.headers.get("content-type") ?? "";
    if (response.ok && type.startsWith("application/octet-stream")) {
      return (await response.arrayBuffer()) as T;
    }
    let body: unknown;
    try {
      body = await response.json();
    } catch {
      body = undefined;
    }
    if (response.status === 401) {
      this.setStatus("unauthorized");
      throw isObject(body) && typeof body.error === "string" ? body.error : UNAUTHORIZED_MESSAGE;
    }
    if (isObject(body) && body.ok === true) return body.result as T;
    if (isObject(body) && typeof body.error === "string") {
      // The connection this command named is gone: open a new one.
      if (body.code === "no_connection") this.dropSocket();
      throw body.error;
    }
    throw `PC가 요청을 처리하지 못했습니다 (HTTP ${response.status}). 잠시 후 다시 시도해 주세요.`;
  }

  /** The current connection id, waiting for the socket's hello. */
  private connection(): Promise<string> {
    this.start();
    if (this.status === "unauthorized") return Promise.reject(UNAUTHORIZED_MESSAGE);
    if (this.connId !== null) return Promise.resolve(this.connId);
    return new Promise((resolve, reject) => {
      const waiter = {
        resolve: (conn: string) => {
          clearTimeout(timer);
          resolve(conn);
        },
        reject: (error: string) => {
          clearTimeout(timer);
          reject(error);
        },
      };
      const timer = setTimeout(() => {
        this.waiters = this.waiters.filter((candidate) => candidate !== waiter);
        reject(DISCONNECTED_MESSAGE);
      }, this.connectionTimeoutMs);
      this.waiters.push(waiter);
    });
  }

  private rejectWaiters(error: string): void {
    const waiters = this.waiters.splice(0);
    for (const waiter of waiters) waiter.reject(error);
  }

  private setStatus(status: RemoteStatus): void {
    if (this.status === status) return;
    this.status = status;
    for (const listener of this.statusListeners) listener(status);
  }

  private emit(event: string, payload: unknown): void {
    const handlers = this.listeners.get(event);
    if (!handlers) return;
    for (const handler of [...handlers]) {
      try {
        handler({ event, id: 0, payload });
      } catch (error) {
        console.error(`remote event handler failed: ${event}`, error);
      }
    }
  }

  private openSocket(): void {
    if (!this.started || this.status === "unauthorized") return;
    this.setStatus("connecting");
    let socket: SocketLike;
    try {
      socket = this.createSocket(this.socketUrl());
    } catch {
      this.scheduleReconnect();
      return;
    }
    this.socket = socket;
    socket.onmessage = (event) => {
      if (this.socket !== socket || typeof event.data !== "string") return;
      let message: unknown;
      try {
        message = JSON.parse(event.data);
      } catch {
        return;
      }
      if (isObject(message)) this.handleMessage(message);
    };
    socket.onclose = () => {
      if (this.socket !== socket) return;
      this.socket = null;
      this.connId = null;
      this.scheduleReconnect();
    };
    socket.onerror = () => {
      // `onclose` follows and schedules the reconnect.
    };
  }

  /** Close the socket so a fresh connection replaces it. */
  private dropSocket(): void {
    const socket = this.socket;
    if (!socket) return;
    this.socket = null;
    this.connId = null;
    socket.close();
    this.scheduleReconnect();
  }

  private scheduleReconnect(): void {
    if (!this.started || this.status === "unauthorized") return;
    this.setStatus(this.connectedBefore ? "closed" : "connecting");
    if (this.reconnectTimer !== null) return;
    const delays = this.reconnectDelays;
    const delay = delays[Math.min(this.attempt, delays.length - 1)] ?? 1_000;
    this.attempt += 1;
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.openSocket();
    }, delay);
  }

  private handleMessage(message: Record<string, unknown>): void {
    switch (message.type) {
      case "hello": {
        if (typeof message.connId !== "string") return;
        this.connId = message.connId;
        this.attempt = 0;
        const reconnect = this.connectedBefore;
        this.connectedBefore = true;
        this.setStatus("open");
        const waiters = this.waiters.splice(0);
        for (const waiter of waiters) waiter.resolve(message.connId);
        if (this.attachedProject !== null) {
          this.resyncPending = reconnect;
          this.socket?.send(JSON.stringify({ type: "attach", projectId: this.attachedProject }));
        } else if (reconnect) {
          this.emit(RESYNC_EVENT, {});
        }
        return;
      }
      case "attached":
        if (this.resyncPending) {
          this.resyncPending = false;
          this.emit(RESYNC_EVENT, {});
        }
        return;
      case "attach_failed":
      case "detached": {
        this.resyncPending = false;
        if (message.type === "detached" && typeof message.error !== "string") return;
        this.attachedProject = null;
        this.emit(DETACHED_EVENT, {
          error: typeof message.error === "string" ? message.error : undefined,
        });
        return;
      }
      case "event":
        if (typeof message.event === "string") this.emit(message.event, message.payload);
        return;
      case "resync":
        this.emit(RESYNC_EVENT, {});
        return;
      default:
        return;
    }
  }
}

let shared: RemoteTransport | null = null;

/** The page's one remote transport. */
export function remoteTransport(): RemoteTransport {
  shared ??= new RemoteTransport();
  return shared;
}
