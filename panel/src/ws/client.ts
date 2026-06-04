/**
 * WebSocket protocol client (typed messages, reconnect, single-disconnect log).
 *
 * features/03_agent-panel.md ## Behaviors → Connection:
 *   - token from `location.search` (URLSearchParams); url is
 *     `ws://${location.host}/ws?token=...`.
 *   - auto-reconnect with 2s backoff; on (re)connect re-request `status` + `list`.
 *   - FIX (vanilla advisory): track was-open state so an outage logs ONE
 *     disconnect line, not one per 2s retry cycle.
 *   - unknown WS message types append a log line and NEVER throw.
 *
 * The DOM `WebSocket` and `location` are injected (constructor seams) so the
 * client is unit-testable headless with a mock socket and a synthetic location.
 */

import { isServerMessage, type ClientMessage, type ServerMessage } from "@/ws/protocol";

/** 2s reconnect backoff (features/03 ## Behaviors → Connection). */
export const RECONNECT_BACKOFF_MS = 2000;

/**
 * `WebSocket.OPEN` readyState value. Hardcoded to the spec constant (1) rather
 * than read from the global `WebSocket.OPEN`: the static is not reliably present
 * in every environment (e.g. happy-dom under vitest), and an injected mock
 * socket reports the same numeric readyState. Both DOM and mock agree on 1.
 */
const WS_OPEN = 1;

/** Log kinds the client emits via {@link WsClientOptions.onLog}. */
export type WsLogKind =
  | "info" // connected / informational
  | "disconnect" // outage — emitted ONCE per outage (was-open fix)
  | "unknown" // a server message of an unrecognized type
  | "badjson"; // a frame that was not valid JSON

/** The minimal `location` surface the client reads (injectable for tests). */
export interface LocationLike {
  /** `location.search`, e.g. "?token=abc". */
  search: string;
  /** `location.host`, e.g. "127.0.0.1:8765". */
  host: string;
}

/**
 * Factory that produces a WebSocket-like object for a url. In production this
 * is `(url) => new WebSocket(url)`; tests inject a mock.
 */
export type SocketFactory = (url: string) => WebSocket;

export interface WsClientOptions {
  /** Produces the underlying socket. Defaults to the global `WebSocket`. */
  socketFactory?: SocketFactory;
  /** Location source. Defaults to the global `location`. */
  location?: LocationLike;
  /** Called for every structurally-valid server message. */
  onMessage: (msg: ServerMessage) => void;
  /** Called for connection lifecycle + unknown/bad frames (optional). */
  onLog?: (kind: WsLogKind, text: string) => void;
  /** Called whenever the connection becomes open / closed (optional). */
  onOpenChange?: (open: boolean) => void;
}

type Timer = ReturnType<typeof setTimeout>;

/**
 * Stateful WS client. Construct, then call {@link WsClient.connect}. The client
 * owns the reconnect loop; callers drive sends via {@link WsClient.send}.
 */
export class WsClient {
  private readonly socketFactory: SocketFactory;
  private readonly location: LocationLike;
  private readonly onMessage: (msg: ServerMessage) => void;
  private readonly onLog: (kind: WsLogKind, text: string) => void;
  private readonly onOpenChange: (open: boolean) => void;

  private ws: WebSocket | null = null;
  private reconnectTimer: Timer | null = null;
  /**
   * Was-open tracking (vanilla advisory fix): true once a socket has opened in
   * the CURRENT outage window. Set on open; the FIRST close after an open logs
   * one disconnect line and clears the flag, so subsequent failed retry cycles
   * (which never reopen) do not re-log. A successful reopen re-arms it.
   */
  private wasOpen = false;
  /** True between connect() and a terminal stop(); guards the reconnect loop. */
  private active = false;

  constructor(options: WsClientOptions) {
    this.socketFactory =
      options.socketFactory ?? ((url) => new WebSocket(url));
    this.location = options.location ?? globalThis.location;
    this.onMessage = options.onMessage;
    this.onLog = options.onLog ?? (() => {});
    this.onOpenChange = options.onOpenChange ?? (() => {});
  }

  /** Token from `location.search` via URLSearchParams (empty if absent). */
  private token(): string {
    return new URLSearchParams(this.location.search).get("token") ?? "";
  }

  /** `ws://${host}/ws?token=<encoded>` (features/03 ## Behaviors → Connection). */
  url(): string {
    return `ws://${this.location.host}/ws?token=${encodeURIComponent(this.token())}`;
  }

  /** Open the connection and start the reconnect loop. */
  connect(): void {
    this.active = true;
    this.open();
  }

  private open(): void {
    let socket: WebSocket;
    try {
      socket = this.socketFactory(this.url());
    } catch {
      // Construction itself failed — schedule a retry (treated like an outage).
      this.scheduleReconnect();
      return;
    }
    this.ws = socket;
    socket.onopen = () => this.handleOpen();
    socket.onmessage = (ev: MessageEvent) => this.handleMessage(ev);
    socket.onerror = () => this.handleError();
    socket.onclose = () => this.handleClose();
  }

  private handleOpen(): void {
    this.wasOpen = true;
    this.onOpenChange(true);
    this.onLog("info", "서버에 연결되었습니다.");
    // On (re)connect, re-request status + list (features/03 ## Behaviors).
    this.send({ type: "status" });
    this.send({ type: "list" });
  }

  private handleError(): void {
    // Reconnect path; the close handler follows and owns the single log + retry.
    // No state change here beyond letting onclose run (vanilla advisory: the
    // was-open flag, not onerror, decides whether to log).
  }

  private handleClose(): void {
    this.onOpenChange(false);
    // Log the disconnect exactly once per outage: only when this close follows
    // a socket that had opened. Failed retry cycles (never opened) skip the log.
    if (this.wasOpen) {
      this.wasOpen = false;
      this.onLog("disconnect", "연결이 끊겼습니다. 재연결합니다…");
    }
    this.scheduleReconnect();
  }

  private scheduleReconnect(): void {
    if (!this.active) return;
    if (this.reconnectTimer !== null) return; // never stack timers
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.open();
    }, RECONNECT_BACKOFF_MS);
  }

  private handleMessage(ev: MessageEvent): void {
    let parsed: unknown;
    try {
      parsed = JSON.parse(ev.data as string);
    } catch {
      // Malformed frame — log, never throw (features/03 ## Behaviors).
      this.onLog("badjson", "잘못된 메시지를 받았습니다.");
      return;
    }
    if (isServerMessage(parsed)) {
      this.onMessage(parsed);
      return;
    }
    // Unknown / unrecognized type — surface to the log, never throw.
    const t =
      typeof parsed === "object" && parsed !== null && "type" in parsed
        ? String((parsed as { type: unknown }).type)
        : String(parsed);
    this.onLog("unknown", `알 수 없는 메시지 유형(unknown): ${t}`);
  }

  /** True iff the underlying socket is OPEN. */
  isOpen(): boolean {
    return this.ws !== null && this.ws.readyState === WS_OPEN;
  }

  /**
   * Send a client message. Returns true if it was written; false (no throw) if
   * the socket is not open — callers gate UI off the store, not the return.
   */
  send(msg: ClientMessage): boolean {
    if (this.ws !== null && this.ws.readyState === WS_OPEN) {
      this.ws.send(JSON.stringify(msg));
      return true;
    }
    return false;
  }

  /** Stop the reconnect loop and close the socket (no further retries). */
  stop(): void {
    this.active = false;
    if (this.reconnectTimer !== null) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    if (this.ws !== null) {
      this.ws.onopen = null;
      this.ws.onmessage = null;
      this.ws.onerror = null;
      this.ws.onclose = null;
      try {
        this.ws.close();
      } catch {
        // ignore — socket may already be closed
      }
      this.ws = null;
    }
  }
}
