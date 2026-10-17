import { describe, expect, it, vi } from "vitest";

import { CONNECTION_LOST_MESSAGE, DISCONNECTED_MESSAGE } from "@/lib/remoteTransport";

import { LONG_TURN_COMMANDS, isConnectionLost, recoveringInvoke } from "./connectionRecovery";

describe("recoveringInvoke", () => {
  it("turns a lost long turn into a recovery instead of a failure", async () => {
    const onLost = vi.fn();
    const invoke = recoveringInvoke(vi.fn().mockRejectedValue(CONNECTION_LOST_MESSAGE), onLost);
    for (const cmd of LONG_TURN_COMMANDS) {
      await expect(invoke(cmd, { sessionId: "s" })).resolves.toBeNull();
    }
    expect(onLost.mock.calls.map(([cmd]) => cmd)).toEqual([...LONG_TURN_COMMANDS]);
  });

  it("keeps backend errors, unsent requests and other commands as failures", async () => {
    const onLost = vi.fn();
    await expect(
      recoveringInvoke(vi.fn().mockRejectedValue("이미 실행 중인 요청이 있습니다."), onLost)("chat"),
    ).rejects.toBe("이미 실행 중인 요청이 있습니다.");
    await expect(
      recoveringInvoke(vi.fn().mockRejectedValue(DISCONNECTED_MESSAGE), onLost)("chat"),
    ).rejects.toBe(DISCONNECTED_MESSAGE);
    await expect(
      recoveringInvoke(vi.fn().mockRejectedValue(CONNECTION_LOST_MESSAGE), onLost)("session_load"),
    ).rejects.toBe(CONNECTION_LOST_MESSAGE);
    expect(onLost).not.toHaveBeenCalled();
  });

  it("passes results through", async () => {
    const invoke = recoveringInvoke(vi.fn().mockResolvedValue({ ok: 1 }), vi.fn());
    await expect(invoke("chat", {})).resolves.toEqual({ ok: 1 });
    expect(isConnectionLost("other")).toBe(false);
  });
});
