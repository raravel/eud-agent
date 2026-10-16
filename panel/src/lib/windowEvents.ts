import type { EventCallback, EventName, UnlistenFn } from "@tauri-apps/api/event";

import { listen } from "./transport";

/** Global listeners receive every emit_to in a project's per-window fanout. */
export function listenToCurrentWindow<T>(
  event: EventName,
  handler: EventCallback<T>,
): Promise<UnlistenFn> {
  return listen<T>(event, handler);
}
