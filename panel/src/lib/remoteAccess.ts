/** Pure helpers for remote access (Settings → 원격 접속 and the remote page). */

export const MIN_REMOTE_PORT = 1024;
export const MAX_REMOTE_PORT = 65535;

/** `ABCD2345` → `ABCD-2345` for reading aloud or typing. */
export function formatPairingCode(code: string): string {
  return code.length === 8 ? `${code.slice(0, 4)}-${code.slice(4)}` : code;
}

/** Bare uppercase code from what a person typed (`abcd-2345`, `ABCD 2345`). */
export function normalizePairingCode(input: string): string {
  return input.replace(/[\s-]/g, "").toUpperCase();
}

/** A port the backend will accept, or null. */
export function parseRemotePort(input: string): number | null {
  const trimmed = input.trim();
  if (!/^\d{1,5}$/.test(trimmed)) return null;
  const port = Number(trimmed);
  return port >= MIN_REMOTE_PORT && port <= MAX_REMOTE_PORT ? port : null;
}

/** The backend's code alphabet (no `I`, `O`, `0`, `1`), eight symbols. */
const PAIRING_CODE = /^[A-HJ-NP-Z2-9]{8}$/;

/** Whether a normalized code can be one the PC issued. */
export function isPairingCode(code: string): boolean {
  return PAIRING_CODE.test(code);
}

/** A default device name from the browser's user agent. iPadOS Safari reports a Mac, told apart by touch. */
export function defaultDeviceName(userAgent: string, maxTouchPoints = 0): string {
  if (/android/i.test(userAgent)) return "Android 휴대폰";
  if (/ipad/i.test(userAgent) || (/macintosh/i.test(userAgent) && maxTouchPoints > 1)) {
    return "iPad";
  }
  if (/iphone|ipod/i.test(userAgent)) return "iPhone";
  return "휴대폰";
}

/** The pairing code carried in a QR link's hash (`#pair=CODE`). */
export function pairingCodeFromHash(hash: string): string | null {
  const params = new URLSearchParams(hash.replace(/^#/, ""));
  const code = params.get("pair");
  return code ? normalizePairingCode(code) : null;
}

/** Whole seconds until `expiresAt`, never negative. */
export function secondsUntil(expiresAt: number, now: number): number {
  return Math.max(0, Math.ceil((expiresAt - now) / 1000));
}

/** `4:07` style countdown. */
export function formatCountdown(seconds: number): string {
  const minutes = Math.floor(seconds / 60);
  return `${minutes}:${String(seconds % 60).padStart(2, "0")}`;
}

/** Korean relative time for a device's last visit. */
export function formatLastSeen(lastSeenAt: number | null, now: number): string {
  if (lastSeenAt === null) return "접속 기록 없음";
  const minutes = Math.floor(Math.max(0, now - lastSeenAt) / 60000);
  if (minutes < 1) return "방금 전 접속";
  if (minutes < 60) return `${minutes}분 전 접속`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}시간 전 접속`;
  return `${Math.floor(hours / 24)}일 전 접속`;
}

/** `12.3 / 36.9MB` for an installer download. */
export function formatDownloadProgress(downloaded: number, total: number): string {
  const mb = (bytes: number) => (bytes / (1024 * 1024)).toFixed(1);
  return `${mb(downloaded)} / ${mb(total)}MB`;
}

/** Whole percent of a download, 0..100. */
export function downloadPercent(downloaded: number, total: number): number {
  if (total <= 0) return 0;
  return Math.min(100, Math.max(0, Math.floor((downloaded * 100) / total)));
}
