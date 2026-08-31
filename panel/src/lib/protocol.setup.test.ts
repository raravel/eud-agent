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
      editorPath: "C:\\Editor",
      editorValid: true,
      assetsReady: true,
      defaultProvider: "codex",
      providers,
      setupRequired: false,
    };
    expect(isSetupMessage(message)).toBe(true);
    expect(isServerMessage(message)).toBe(true);
  });

  it("accepts nullable option fields emitted by older Rust builds", () => {
    const message = {
      type: "setup",
      editorPath: "",
      editorValid: false,
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
        editorPath: "",
        editorValid: false,
        assetsReady: false,
        providers: providers.slice(0, 4),
        setupRequired: true,
      }),
    ).toBe(false);
    expect(
      isSetupMessage({
        type: "setup",
        editorPath: "",
        editorValid: false,
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
        editorPath: "",
        editorValid: false,
        assetsReady: false,
        providers,
        setupRequired: true,
        error: "invalid_editor_folder",
      }),
    ).toBe(true);
    expect(
      isSetupMessage({
        type: "setup",
        editorPath: "",
        editorValid: false,
        assetsReady: false,
        providers,
        setupRequired: true,
        error: null,
      }),
    ).toBe(true);
    expect(
      isSetupMessage({
        type: "setup",
        editorPath: "",
        editorValid: false,
        assetsReady: false,
        providers,
        setupRequired: true,
        error: 42,
      }),
    ).toBe(false);
  });
});
