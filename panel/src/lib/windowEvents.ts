import { listen, type EventCallback, type EventName, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

/** Global listeners receive every emit_to in a project's per-window fanout. */
export function listenToCurrentWindow<T>(
  event: EventName,
  handler: EventCallback<T>,
): Promise<UnlistenFn> {
  return listen(event, handler, { target: getCurrentWindow().label });
}
