import { describe, expect, it, vi } from "vitest";

import {
  remoteDeviceRevoke,
  remotePairBegin,
  remoteSettingsSave,
  remoteStatus,
  remoteTailscaleInstall,
  remoteTailscaleLogin,
  remoteTailscaleServe,
  toRemotePairingNotice,
  toTailscaleInstallProgress,
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
    funnel: true,
    serveCommand: "tailscale serve --bg --https=443 http://127.0.0.1:47821",
    backendState: "Running",
    installSupported: true,
    loginName: "me@example.com",
  },
};

describe("remote access IPC", () => {
  it("installs and signs in to Tailscale", async () => {
    const install = vi.fn<InvokeFn>().mockResolvedValue({ rebootRequired: true, status });
    await expect(remoteTailscaleInstall(install)).resolves.toEqual({ rebootRequired: true, status });
    expect(install).toHaveBeenCalledWith("remote_tailscale_install");
    await expect(
      remoteTailscaleInstall(vi.fn<InvokeFn>().mockResolvedValue({ status })),
    ).rejects.toThrow("invalid tailscale install response");

    const url = "https://login.tailscale.com/a/abc";
    await expect(
      remoteTailscaleLogin(vi.fn<InvokeFn>().mockResolvedValue({ url, status })),
    ).resolves.toEqual({ url, status });
    await expect(
      remoteTailscaleLogin(vi.fn<InvokeFn>().mockResolvedValue({ url: null, status })),
    ).resolves.toEqual({ url: null, status });

    const enable = "https://login.tailscale.com/f/serve?node=n1";
    const serve = vi.fn<InvokeFn>().mockResolvedValue({ url: enable, status });
    await expect(remoteTailscaleServe(serve)).resolves.toEqual({ url: enable, status });
    expect(serve).toHaveBeenCalledWith("remote_tailscale_serve");
    await expect(
      remoteTailscaleServe(vi.fn<InvokeFn>().mockResolvedValue({ url: null, status })),
    ).resolves.toEqual({ url: null, status });
    await expect(
      remoteTailscaleServe(vi.fn<InvokeFn>().mockResolvedValue({ url: null, status: null })),
    ).rejects.toThrow("invalid remote status response");
    await expect(
      remoteTailscaleServe(vi.fn<InvokeFn>().mockResolvedValue({ url: 1, status })),
    ).rejects.toThrow("invalid tailscale serve response");

    expect(toTailscaleInstallProgress({ phase: "download", downloaded: 1, total: 2 })).toEqual({
      phase: "download",
      downloaded: 1,
      total: 2,
    });
    expect(toTailscaleInstallProgress({ phase: "install" })).toEqual({ phase: "install" });
    expect(toTailscaleInstallProgress({ phase: "download" })).toBeNull();
  });

  it("reads an older status without install fields", async () => {
    const {
      backendState: _state,
      installSupported: _supported,
      loginName: _login,
      ...older
    } = status.tailscale;
    const result = await remoteStatus(
      vi.fn<InvokeFn>().mockResolvedValue({ ...status, tailscale: older }),
    );
    expect(result.tailscale.backendState).toBeNull();
    expect(result.tailscale.installSupported).toBe(false);
    expect(result.tailscale.loginName).toBeNull();
  });

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
      .mockResolvedValue([
        { id: "d2", name: "태블릿", createdAt: 1, lastSeenAt: 2, ownerLogin: "me@example.com", expired: true },
      ]);
    await expect(remoteDeviceRevoke("d1", invoke)).resolves.toEqual([
      { id: "d2", name: "태블릿", createdAt: 1, lastSeenAt: 2, ownerLogin: "me@example.com", expired: true },
    ]);
    // An older app without the fields: not expired, no owner.
    await expect(
      remoteDeviceRevoke(
        "d1",
        vi.fn<InvokeFn>().mockResolvedValue([{ id: "d3", name: "폰", createdAt: 1, lastSeenAt: null }]),
      ),
    ).resolves.toEqual([
      { id: "d3", name: "폰", createdAt: 1, lastSeenAt: null, ownerLogin: null, expired: false },
    ]);
    expect(invoke).toHaveBeenCalledWith("remote_device_revoke", { id: "d1" });
    await expect(
      remoteDeviceRevoke("d1", vi.fn<InvokeFn>().mockResolvedValue([{ id: 1 }])),
    ).rejects.toThrow("invalid remote device");
  });

  it("reads pairing notices", () => {
    expect(toRemotePairingNotice({ kind: "paired", deviceId: "d1", name: "폰" })).toEqual({
      kind: "paired",
      deviceId: "d1",
      name: "폰",
    });
    expect(toRemotePairingNotice({ kind: "codeLocked" })).toEqual({ kind: "codeLocked" });
    expect(toRemotePairingNotice({ kind: "paired" })).toBeNull();
    expect(toRemotePairingNotice(null)).toBeNull();
  });
});
