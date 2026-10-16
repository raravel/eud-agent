/**
 * The one way panel code reaches the app: `invoke` a command and `listen` to
 * this window's events.
 *
 * Inside the desktop app these are Tauri's own `invoke` and a listener
 * targeted at the current window. The remote page (served by the app at
 * `/remote/`, with no Tauri runtime) sends the same commands over HTTP and
 * receives the same events over the remote WebSocket (`remoteTransport.ts`).
 * Results and errors keep the desktop's shapes: a command rejects with the
 * app's error string, and a binary result is an `ArrayBuffer`.
 */
import {
  invoke as tauriInvoke,
  type InvokeArgs,
  type InvokeOptions,
} from "@tauri-apps/api/core";
import {
  listen as tauriListen,
  type EventCallback,
  type EventName,
  type UnlistenFn,
} from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { remoteTransport } from "./remoteTransport";

export type TransportKind = "tauri" | "remote";

let override: TransportKind | null = null;

/** Force a transport (tests); `null` restores detection. */
export function setTransportKind(kind: TransportKind | null): void {
  override = kind;
}

/**
 * Tauri whenever its runtime is present. Without it, the remote page (any
 * path under `/remote`) talks to the app over the network; anything else
 * (unit tests with mocked Tauri modules) keeps the Tauri path.
 */
export function transportKind(): TransportKind {
  if (override !== null) return override;
  if (typeof window === "undefined") return "tauri";
  if ("__TAURI_INTERNALS__" in window) return "tauri";
  return window.location.pathname.startsWith("/remote") ? "remote" : "tauri";
}

export function isRemote(): boolean {
  return transportKind() === "remote";
}

/** Arguments are forwarded exactly as given, so the desktop call is Tauri's own. */
export function invoke<T = unknown>(
  ...call: [cmd: string, args?: InvokeArgs, options?: InvokeOptions]
): Promise<T> {
  return isRemote() ? remoteTransport().invoke<T>(...call) : tauriInvoke<T>(...call);
}

/**
 * Listen to `event` for this window only. Tauri's global `listen` receives
 * every `emit_to` in a project's per-window fanout; the remote socket carries
 * only the attached project's events.
 */
export function listen<T>(event: EventName, handler: EventCallback<T>): Promise<UnlistenFn> {
  return isRemote()
    ? remoteTransport().listen<T>(event, handler)
    : tauriListen(event, handler, { target: getCurrentWindow().label });
}
