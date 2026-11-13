/**
 * HTTP client for the remote page served at `/remote/` by the desktop app.
 * The device token travels only in the HttpOnly `eud_remote` cookie.
 */

import i18n from "@/lib/i18n";

export interface RemoteDeviceInfo {
  deviceId: string;
  name: string;
}

export type FetchFn = (input: string, init?: RequestInit) => Promise<Response>;

const API = "/remote/api";

async function errorMessage(response: Response, fallback: string): Promise<string> {
  try {
    const body: unknown = await response.json();
    if (
      typeof body === "object" &&
      body !== null &&
      typeof (body as { error?: unknown }).error === "string"
    ) {
      return (body as { error: string }).error;
    }
  } catch {
    // Not JSON: fall through to the generic text.
  }
  return fallback;
}

function toDevice(value: unknown): RemoteDeviceInfo {
  if (
    typeof value !== "object" ||
    value === null ||
    typeof (value as RemoteDeviceInfo).deviceId !== "string" ||
    typeof (value as RemoteDeviceInfo).name !== "string"
  ) {
    throw new Error(i18n.t("remote.api.badResponse"));
  }
  const device = value as RemoteDeviceInfo;
  return { deviceId: device.deviceId, name: device.name };
}

/** Why the PC refused this browser's device, when it must pair again. */
export interface PairAgain {
  /** `expired` (unused for 30 days) or `identity` (another Tailscale account). */
  code: "expired" | "identity";
  message: string;
}

/** This browser's paired device; null when it is not paired. */
export async function fetchMe(fetchFn: FetchFn = fetch): Promise<RemoteDeviceInfo | null> {
  const status = await fetchMeStatus(fetchFn);
  return status.device;
}

/**
 * This browser's paired device, or why it has none: a device the PC
 * expired or that another Tailscale account uses must pair again, and the
 * page says so.
 */
export async function fetchMeStatus(
  fetchFn: FetchFn = fetch,
): Promise<{ device: RemoteDeviceInfo | null; pairAgain?: PairAgain }> {
  const response = await fetchFn(`${API}/me`, { credentials: "same-origin" });
  if (response.status === 401) {
    try {
      const body = (await response.json()) as { code?: unknown; error?: unknown };
      if ((body.code === "expired" || body.code === "identity") && typeof body.error === "string") {
        return { device: null, pairAgain: { code: body.code, message: body.error } };
      }
    } catch {
      // Not JSON: an unpaired browser.
    }
    return { device: null };
  }
  if (!response.ok) {
    throw new Error(await errorMessage(response, i18n.t("remote.api.connectFailed")));
  }
  return { device: toDevice(await response.json()) };
}

/** Exchange a pairing code for a device cookie. */
export async function pair(
  code: string,
  deviceName: string,
  fetchFn: FetchFn = fetch,
): Promise<RemoteDeviceInfo> {
  const response = await fetchFn(`${API}/pair`, {
    method: "POST",
    credentials: "same-origin",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ code, deviceName }),
  });
  if (!response.ok) {
    throw new Error(
      await errorMessage(response, i18n.t("remote.api.pairFailed")),
    );
  }
  return toDevice(await response.json());
}

/** Disconnect this browser's device. */
export async function logout(fetchFn: FetchFn = fetch): Promise<void> {
  const response = await fetchFn(`${API}/logout`, {
    method: "POST",
    credentials: "same-origin",
  });
  if (!response.ok && response.status !== 401) {
    throw new Error(
      await errorMessage(response, i18n.t("remote.api.logoutFailed")),
    );
  }
}
