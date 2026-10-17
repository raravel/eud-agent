import { describe, expect, it, vi } from "vitest";

import {
  CONNECTION_LOST_MESSAGE,
  DISCONNECTED_MESSAGE,
  REQUEST_HEADER,
} from "@/lib/remoteTransport";

import {
  LONG_TURN_COMMANDS,
  MAX_RESENDS,
  isConnectionLost,
  recoveringInvoke,
  type RecoveryHooks,
} from "./connectionRecovery";

function hooks(open = true): RecoveryHooks & {
  onLost: ReturnType<typeof vi.fn>;
  onRecovered: ReturnType<typeof vi.fn>;
} {
  return {
    onLost: vi.fn(),
    onRecovered: vi.fn(),
    waitForOpen: vi.fn(async () => open),
  };
}

function requestIdOf(call: unknown[]): string | undefined {
  const options = call[2] as { headers?: Record<string, string> } | undefined;
  return options?.headers?.[REQUEST_HEADER];
}

describe("recoveringInvoke", () => {
  it("resends a lost long turn with the same request id and returns its answer", async () => {
    for (const cmd of LONG_TURN_COMMANDS) {
      const invoke = vi
        .fn()
        .mockRejectedValueOnce(CONNECTION_LOST_MESSAGE)
        .mockResolvedValueOnce({ done: true });
      const recovery = hooks();
      await expect(recoveringInvoke(invoke, recovery)(cmd, { sessionId: "s" })).resolves.toEqual({
        done: true,
      });
      expect(invoke).toHaveBeenCalledTimes(2);
      const [first, resent] = invoke.mock.calls;
      expect(requestIdOf(first)).toBeTruthy();
      expect(requestIdOf(resent)).toBe(requestIdOf(first));
      expect(resent[1]).toEqual({ sessionId: "s" });
      expect(recovery.onLost).toHaveBeenCalledWith(cmd);
      expect(recovery.onRecovered).toHaveBeenCalledWith(cmd, true);
    }
  });

  it("each turn has its own request id", async () => {
    const invoke = vi.fn().mockResolvedValue(null);
    const run = recoveringInvoke(invoke, hooks());
    await run("chat", {});
    await run("chat", {});
    expect(requestIdOf(invoke.mock.calls[0])).not.toBe(requestIdOf(invoke.mock.calls[1]));
  });

  it("reports a backend error a resend got as the turn's failure", async () => {
    const invoke = vi
      .fn()
      .mockRejectedValueOnce(CONNECTION_LOST_MESSAGE)
      .mockRejectedValueOnce("모델 오류");
    const recovery = hooks();
    await expect(recoveringInvoke(invoke, recovery)("chat", {})).rejects.toBe("모델 오류");
    expect(recovery.onRecovered).toHaveBeenCalledWith("chat", true);
  });

  it("gives up when the socket does not come back or every resend is lost", async () => {
    const closed = hooks(false);
    const lostOnce = vi.fn().mockRejectedValue(CONNECTION_LOST_MESSAGE);
    await expect(recoveringInvoke(lostOnce, closed)("chat", {})).resolves.toBeNull();
    expect(lostOnce).toHaveBeenCalledTimes(1);
    expect(closed.onRecovered).toHaveBeenCalledWith("chat", false);

    const open = hooks();
    const alwaysLost = vi.fn().mockRejectedValue(CONNECTION_LOST_MESSAGE);
    await expect(recoveringInvoke(alwaysLost, open)("chat", {})).resolves.toBeNull();
    expect(alwaysLost).toHaveBeenCalledTimes(1 + MAX_RESENDS);
    expect(open.onRecovered).toHaveBeenCalledWith("chat", false);
  });

  it("keeps backend errors, unsent requests and other commands as failures", async () => {
    const recovery = hooks();
    await expect(
      recoveringInvoke(vi.fn().mockRejectedValue("이미 실행 중인 요청이 있습니다."), recovery)("chat"),
    ).rejects.toBe("이미 실행 중인 요청이 있습니다.");
    await expect(
      recoveringInvoke(vi.fn().mockRejectedValue(DISCONNECTED_MESSAGE), recovery)("chat"),
    ).rejects.toBe(DISCONNECTED_MESSAGE);
    await expect(
      recoveringInvoke(vi.fn().mockRejectedValue(CONNECTION_LOST_MESSAGE), recovery)("session_load"),
    ).rejects.toBe(CONNECTION_LOST_MESSAGE);
    expect(recovery.onLost).not.toHaveBeenCalled();
  });

  it("passes results through", async () => {
    const invoke = recoveringInvoke(vi.fn().mockResolvedValue({ ok: 1 }), hooks());
    await expect(invoke("chat", {})).resolves.toEqual({ ok: 1 });
    expect(isConnectionLost("other")).toBe(false);
  });
});
