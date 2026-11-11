import { describe, expect, it } from "vitest";

import {
  defaultDeviceName,
  downloadPercent,
  formatDownloadProgress,
  formatCountdown,
  formatLastSeen,
  formatPairingCode,
  isPairingCode,
  normalizePairingCode,
  pairingCodeFromHash,
  parseRemotePort,
  secondsUntil,
} from "./remoteAccess";

describe("remote access helpers", () => {
  it("formats installer download progress", () => {
    expect(formatDownloadProgress(12.3 * 1024 * 1024, 36.92 * 1024 * 1024)).toBe("12.3 / 36.9MB");
    expect(downloadPercent(1, 4)).toBe(25);
    expect(downloadPercent(5, 4)).toBe(100);
    expect(downloadPercent(1, 0)).toBe(0);
  });

  it("formats and normalizes pairing codes", () => {
    expect(formatPairingCode("ABCD2345")).toBe("ABCD-2345");
    expect(formatPairingCode("ABC")).toBe("ABC");
    expect(normalizePairingCode(" abcd-2345 ")).toBe("ABCD2345");
    expect(normalizePairingCode("ab cd 23 45")).toBe("ABCD2345");
  });

  it("accepts only unprivileged ports", () => {
    expect(parseRemotePort("47821")).toBe(47821);
    expect(parseRemotePort(" 1024 ")).toBe(1024);
    expect(parseRemotePort("65535")).toBe(65535);
    expect(parseRemotePort("80")).toBeNull();
    expect(parseRemotePort("65536")).toBeNull();
    expect(parseRemotePort("4782a")).toBeNull();
    expect(parseRemotePort("")).toBeNull();
    expect(parseRemotePort("-1")).toBeNull();
  });

  it("reads the pairing code from a QR link hash", () => {
    expect(pairingCodeFromHash("#pair=abcd2345")).toBe("ABCD2345");
    expect(pairingCodeFromHash("#other=1")).toBeNull();
    expect(pairingCodeFromHash("")).toBeNull();
  });

  it("accepts only codes from the backend alphabet", () => {
    expect(isPairingCode("ABCD2345")).toBe(true);
    expect(isPairingCode("ABCD234")).toBe(false);
    expect(isPairingCode("ABCD2341")).toBe(false);
    expect(isPairingCode("ABCDO345")).toBe(false);
    expect(isPairingCode("abcd2345")).toBe(false);
  });

  it("names the device after its browser", () => {
    expect(
      defaultDeviceName("Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 Mobile"),
    ).toBe("Android 휴대폰");
    expect(defaultDeviceName("Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X)")).toBe(
      "iPhone",
    );
    expect(defaultDeviceName("Mozilla/5.0 (iPad; CPU OS 17_5 like Mac OS X)")).toBe("iPad");
    expect(defaultDeviceName("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)", 5)).toBe("iPad");
    expect(defaultDeviceName("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)", 0)).toBe("휴대폰");
    expect(defaultDeviceName("")).toBe("휴대폰");
  });

  it("counts down to expiry", () => {
    expect(secondsUntil(10_000, 0)).toBe(10);
    expect(secondsUntil(10_000, 9_001)).toBe(1);
    expect(secondsUntil(10_000, 20_000)).toBe(0);
    expect(formatCountdown(300)).toBe("5:00");
    expect(formatCountdown(67)).toBe("1:07");
  });

  it("describes the last visit in Korean", () => {
    const now = 10 * 24 * 60 * 60 * 1000;
    expect(formatLastSeen(null, now)).toBe("접속 기록 없음");
    expect(formatLastSeen(now - 30_000, now)).toBe("방금 전 접속");
    expect(formatLastSeen(now - 5 * 60_000, now)).toBe("5분 전 접속");
    expect(formatLastSeen(now - 3 * 60 * 60_000, now)).toBe("3시간 전 접속");
    expect(formatLastSeen(now - 2 * 24 * 60 * 60_000, now)).toBe("2일 전 접속");
  });
});
