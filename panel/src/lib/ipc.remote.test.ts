import { describe, expect, it, vi } from "vitest";

import {
  remoteDeviceRevoke,
  remotePairBegin,
  remoteSettingsSave,
  remoteStatus,
  type InvokeFn,
} from "./ipc";

const status = {
  enabled: true,
  port: 47821,
  listening: true,
  error: null,
  tailscale: {
    installed: true,
    running: true,
    dnsName: "pc.tail1234.ts.net",
    serveConfigured: false,
    serveCommand: "tailscale serve --bg --https=443 http://127.0.0.1:47821",
  },
};

describe("remote access IPC", () => {
  it("validates status and passes settings arguments", async () => {
    const invoke = vi.fn<InvokeFn>().mockResolvedValue(status);
    await expect(remoteStatus(invoke)).resolves.toEqual(status);
    await remoteSettingsSave(false, 50000, invoke);
    expect(invoke).toHaveBeenLastCalledWith("remote_settings_save", {
      enabled: false,
      port: 50000,
    });
    await expect(
      remoteStatus(vi.fn<InvokeFn>().mockResolvedValue({ ...status, tailscale: null })),
    ).rejects.toThrow("invalid remote status response");
  });

  it("validates pairing and device responses", async () => {
    await expect(
      remotePairBegin(
        vi.fn<InvokeFn>().mockResolvedValue({ code: "ABCD2345", expiresAt: 5, url: null }),
      ),
    ).resolves.toEqual({ code: "ABCD2345", expiresAt: 5, url: null });
    const invoke = vi
      .fn<InvokeFn>()
      .mockResolvedValue([{ id: "d2", name: "태블릿", createdAt: 1, lastSeenAt: 2 }]);
    await expect(remoteDeviceRevoke("d1", invoke)).resolves.toEqual([
      { id: "d2", name: "태블릿", createdAt: 1, lastSeenAt: 2 },
    ]);
    expect(invoke).toHaveBeenCalledWith("remote_device_revoke", { id: "d1" });
    await expect(
      remoteDeviceRevoke("d1", vi.fn<InvokeFn>().mockResolvedValue([{ id: 1 }])),
    ).rejects.toThrow("invalid remote device");
  });
});
