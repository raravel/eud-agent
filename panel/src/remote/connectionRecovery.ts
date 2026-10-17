/**
 * Long turns over HTTP. `chat`, `plan_feedback`, `plan_approve`,
 * `ask_response` and `map_agent_chat` hold their request open for the whole
 * turn, which a phone can lose (screen lock, network change) while the PC
 * keeps working. Such a loss is not a turn failure: the page notes it, waits
 * for the event socket, and sends the SAME request again with the same
 * request id. The app's request ledger (`remote/server.rs`) joins the turn
 * that is still running or answers with its result, so nothing runs twice,
 * and a request that never reached the PC runs now.
 */
import type { InvokeOptions } from "@tauri-apps/api/core";

import type { InvokeFn } from "@/lib/ipc";
import { CONNECTION_LOST_MESSAGE, REQUEST_HEADER } from "@/lib/remoteTransport";

export const LONG_TURN_COMMANDS: ReadonlySet<string> = new Set([
  "chat",
  "plan_feedback",
  "plan_approve",
  "ask_response",
  "map_agent_chat",
]);

export const RECOVERY_NOTE = "연결이 끊겨 결과를 다시 불러옵니다";

/** How often a lost request is sent again before the page gives up on it. */
export const MAX_RESENDS = 5;

/** The request broke on the way; the app never answered it. */
export function isConnectionLost(error: unknown): boolean {
  return error === CONNECTION_LOST_MESSAGE;
}

/** `invoke` with the transport's options (the request id header). */
export type InvokeWithOptions = (
  cmd: string,
  args?: Record<string, unknown>,
  options?: InvokeOptions,
) => Promise<unknown>;

export interface RecoveryHooks {
  /** The answer was lost; the request is sent again once the socket is back. */
  onLost(cmd: string): void;
  /** Resolves true once the event socket is open again, false to give up. */
  waitForOpen(): Promise<boolean>;
  /**
   * A lost request settled: `answered` when a resend got the app's answer,
   * false when the page gave up (socket never came back, or every resend
   * was lost too) and the caller must reload what the PC did.
   */
  onRecovered(cmd: string, answered: boolean): void;
}

function newRequestId(): string {
  return crypto.randomUUID();
}

/**
 * Run `cmd` once under one request id; when its answer is lost, send it
 * again under the same id after the socket reconnects. Resolves the app's
 * answer, or `null` when the page gave up; a backend error rejects as usual.
 */
export async function invokeResending(
  invoke: InvokeWithOptions,
  cmd: string,
  args: Record<string, unknown> | undefined,
  hooks: RecoveryHooks,
): Promise<unknown> {
  const options: InvokeOptions = { headers: { [REQUEST_HEADER]: newRequestId() } };
  try {
    return await invoke(cmd, args, options);
  } catch (error) {
    if (!isConnectionLost(error)) throw error;
  }
  hooks.onLost(cmd);
  let answered = false;
  try {
    for (let attempt = 0; attempt < MAX_RESENDS; attempt += 1) {
      if (!(await hooks.waitForOpen())) return null;
      try {
        const result = await invoke(cmd, args, options);
        answered = true;
        return result;
      } catch (error) {
        if (!isConnectionLost(error)) {
          answered = true;
          throw error;
        }
      }
    }
    return null;
  } finally {
    hooks.onRecovered(cmd, answered);
  }
}

/**
 * `invoke`, except that a long-turn command whose answer was lost is sent
 * again (see {@link invokeResending}); it resolves `null` only when the page
 * gave up. Every other command, and every backend error, behaves as before.
 */
export function recoveringInvoke(invoke: InvokeWithOptions, hooks: RecoveryHooks): InvokeFn {
  return (cmd, args) =>
    LONG_TURN_COMMANDS.has(cmd) ? invokeResending(invoke, cmd, args, hooks) : invoke(cmd, args);
}
