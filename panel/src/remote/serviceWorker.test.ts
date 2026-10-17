import { describe, expect, it, vi } from "vitest";

import { registerRemoteServiceWorker, type ServiceWorkerHost } from "./serviceWorker";

function host(overrides: Partial<ServiceWorkerHost> = {}) {
  const register = vi.fn().mockResolvedValue({});
  return {
    register,
    host: {
      isSecureContext: true,
      location: { pathname: "/remote/" },
      navigator: { serviceWorker: { register } },
      ...overrides,
    } as ServiceWorkerHost,
  };
}

describe("registerRemoteServiceWorker", () => {
  it("registers the worker beside the remote page for its scope", async () => {
    const { register, host: page } = host();
    await expect(registerRemoteServiceWorker(page)).resolves.toBe(true);
    expect(register).toHaveBeenCalledWith("/remote/sw.js", { scope: "/remote/" });
  });

  it("never registers outside /remote/ or without a secure context", async () => {
    const outside = host({ location: { pathname: "/" } });
    await expect(registerRemoteServiceWorker(outside.host)).resolves.toBe(false);
    expect(outside.register).not.toHaveBeenCalled();
    const insecure = host({ isSecureContext: false });
    await expect(registerRemoteServiceWorker(insecure.host)).resolves.toBe(false);
    expect(insecure.register).not.toHaveBeenCalled();
    await expect(
      registerRemoteServiceWorker({ ...host().host, navigator: {} }),
    ).resolves.toBe(false);
  });

  it("treats a refused registration as no installation", async () => {
    const { register, host: page } = host();
    register.mockRejectedValueOnce(new Error("blocked"));
    await expect(registerRemoteServiceWorker(page)).resolves.toBe(false);
  });
});
