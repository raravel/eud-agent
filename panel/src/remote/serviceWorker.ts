/**
 * The remote page registers its network-only service worker (`public/sw.js`)
 * so a phone can install it as an app. Only the page served at `/remote/`
 * registers it, and only in a secure context (`tailscale serve` HTTPS or
 * loopback); the desktop windows never do.
 */
export interface ServiceWorkerHost {
  isSecureContext: boolean;
  location: { pathname: string };
  navigator: { serviceWorker?: Pick<ServiceWorkerContainer, "register"> };
}

/** Register `/remote/sw.js` for scope `/remote/`; false when it cannot. */
export async function registerRemoteServiceWorker(
  host: ServiceWorkerHost = window,
): Promise<boolean> {
  const container = host.navigator.serviceWorker;
  if (!container || !host.isSecureContext) return false;
  if (!host.location.pathname.startsWith("/remote/")) return false;
  try {
    await container.register("/remote/sw.js", { scope: "/remote/" });
    return true;
  } catch {
    // Installation is a convenience; the page works without it.
    return false;
  }
}
