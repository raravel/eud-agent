import {
  type EventCallback,
  type EventName,
  type UnlistenFn,
} from "@tauri-apps/api/event";
import { listenToCurrentWindow as listen } from "@/lib/windowEvents";

/** Owns asynchronous listener registrations for one Map turn effect. */
export function createMapEventSubscriptions() {
  const unlisteners: UnlistenFn[] = [];
  let disposed = false;
  return {
    async listen<T>(event: EventName, handler: EventCallback<T>): Promise<void> {
      if (disposed) return;
      const unlisten = await listen<T>(event, (message) => {
        if (!disposed) handler(message);
      });
      // React cleanup can run while the native registration is pending.
      if (disposed) unlisten();
      else unlisteners.push(unlisten);
    },
    dispose(): void {
      disposed = true;
      for (const unlisten of unlisteners.splice(0)) unlisten();
    },
  };
}
