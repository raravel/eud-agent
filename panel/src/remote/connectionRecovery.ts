/**
 * Long turns over HTTP. `chat`, `plan_feedback`, `plan_approve`,
 * `ask_response` and `map_agent_chat` hold their request open for the whole
 * turn, which a phone can lose (screen lock, network change) while the PC
 * keeps working. Such a loss is not a turn failure: the page notes it, waits
 * for the event socket, and reloads the conversation from the PC.
 */
import type { InvokeFn } from "@/lib/ipc";
import { CONNECTION_LOST_MESSAGE } from "@/lib/remoteTransport";

export const LONG_TURN_COMMANDS: ReadonlySet<string> = new Set([
  "chat",
  "plan_feedback",
  "plan_approve",
  "ask_response",
  "map_agent_chat",
]);

export const RECOVERY_NOTE = "연결이 끊겨 결과를 다시 불러옵니다";

/** The request broke on the way; the app never answered it. */
export function isConnectionLost(error: unknown): boolean {
  return error === CONNECTION_LOST_MESSAGE;
}

/**
 * `invoke`, except that a long-turn command whose answer was lost resolves
 * `null` and reports `onLost` instead of rejecting. Backend errors and every
 * other command reject as before.
 */
export function recoveringInvoke(invoke: InvokeFn, onLost: (cmd: string) => void): InvokeFn {
  return async (cmd, args) => {
    try {
      return await invoke(cmd, args);
    } catch (error) {
      if (!LONG_TURN_COMMANDS.has(cmd) || !isConnectionLost(error)) throw error;
      onLost(cmd);
      return null;
    }
  };
}
