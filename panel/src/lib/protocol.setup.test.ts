import { describe, expect, it } from "vitest";

import { isServerMessage, isSetupMessage, type SetupMessage } from "./protocol";
import type { ProviderStatus } from "@/providers/types";

const providers: ProviderStatus[] = [
  "codex",
  "claude-code",
  "antigravity",
  "opencode-go",
  "ollama",
].map((provider, index) => ({
  provider: provider as ProviderStatus["provider"],
  availability: index === 0 ? "ready" : "unavailable",
  selectedAsDefault: index === 0,
  canInstall: index < 2,
  canImport: index < 2,
  experimental: provider === "antigravity",
}));

describe("setup message guard", () => {
  it("accepts the typed five-provider snapshot", () => {
    const message: SetupMessage = {
      type: "setup",
      projectPath: "C:\\Project",
      projectValid: true,
      projectOpened: false,
      euddraftPath: "C:\\euddraft.exe",
      euddraftValid: true,
      assetsReady: true,
      defaultProvider: "codex",
      providers,
      setupRequired: false,
    };
    expect(isSetupMessage(message)).toBe(true);
    expect(isServerMessage(message)).toBe(true);
  });
  it("accepts nullable option fields with the explicit-launch marker", () => {
    const message = {
      type: "setup",
      projectPath: "",
      projectValid: false,
      projectOpened: false,
      euddraftPath: "",
      euddraftValid: false,
      assetsReady: true,
      defaultProvider: null,
      providers: providers.map((status) => ({ ...status, detailCode: null })),
      setupRequired: true,
      error: null,
    };
    expect(isSetupMessage(message)).toBe(true);
    expect(isServerMessage(message)).toBe(true);
  });

  it("requires exactly the closed five provider ids", () => {
    expect(
      isSetupMessage({
        type: "setup",
        projectPath: "",
        projectValid: false,
        projectOpened: false,
        euddraftPath: "",
        euddraftValid: false,
        assetsReady: false,
        providers: providers.slice(0, 4),
        setupRequired: true,
      }),
    ).toBe(false);
    expect(
      isSetupMessage({
        type: "setup",
        projectPath: "",
        projectValid: false,
        projectOpened: false,
        euddraftPath: "",
        euddraftValid: false,
        assetsReady: false,
        providers: providers.map((status, index) =>
          index === 4 ? { ...status, provider: "other" } : status,
        ),
        setupRequired: true,
      }),
    ).toBe(false);
  });

  it("accepts a string, null, or omitted stable error code", () => {
    expect(
      isSetupMessage({
        type: "setup",
        projectPath: "",
        projectValid: false,
        euddraftPath: "",
        euddraftValid: false,
        assetsReady: false,
        providers,
        projectOpened: false,
        setupRequired: true,
      }),
    ).toBe(true);
    expect(
      isSetupMessage({
        type: "setup",
        projectPath: "",
        projectValid: false,
        euddraftPath: "",
        euddraftValid: false,
        assetsReady: false,
        providers,
        projectOpened: false,
        setupRequired: true,
        error: "invalid_project_folder",
      }),
    ).toBe(true);
    expect(
      isSetupMessage({
        type: "setup",
        projectPath: "",
        projectValid: false,
        projectOpened: false,
        euddraftPath: "",
        euddraftValid: false,
        assetsReady: false,
        providers,
        setupRequired: true,
        error: 42,
      }),
    ).toBe(false);
  });

  it("validates all harness import issue fields and scopes", () => {
    const base = {
      type: "setup" as const,
      projectPath: "",
      projectValid: false,
      euddraftPath: "",
      euddraftValid: false,
      assetsReady: false,
      providers,
      projectOpened: false,
      setupRequired: true,
      importIssues: [
        {
          id: "memory-1",
          scope: "memory" as const,
          path: "C:\\legacy\\.eud-agent\\memory\\meta.json",
          reason: "invalid UTF-8",
        },
      ],
    };
    expect(isSetupMessage(base)).toBe(true);
    expect(
      isSetupMessage({
        ...base,
        importIssues: [{ ...base.importIssues[0], scope: "unknown" }],
      }),
    ).toBe(false);
    expect(
      isSetupMessage({
        ...base,
        importIssues: [{ ...base.importIssues[0], reason: " " }],
      }),
    ).toBe(false);
  });
});
