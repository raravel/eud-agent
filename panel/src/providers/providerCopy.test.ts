import { describe, expect, it } from "vitest";

import { providerChatErrorText } from "./providerCopy";

describe("providerChatErrorText", () => {
  it("leads a provider failure with its recovery copy and keeps the raw detail", () => {
    const raw = "provider transport failed: provider_auth_refresh_failed";
    const text = providerChatErrorText(raw);
    expect(text).toMatch(/^Claude 로그인 토큰을 갱신하지 못했습니다/);
    expect(text).toMatch(/다시 로그인/);
    expect(text.endsWith(`(${raw})`)).toBe(true);
  });

  it("explains a closed transport instead of showing only its code", () => {
    expect(
      providerChatErrorText("provider transport failed: provider_transport_closed"),
    ).toMatch(/^제공자 연결이 끊겼습니다/);
  });

  it("leaves a message without a provider code unchanged", () => {
    expect(providerChatErrorText("build failed")).toBe("build failed");
  });
});
