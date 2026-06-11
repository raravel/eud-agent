/**
 * Wire-schema guards for the first-run setup messages (EUD-132).
 *
 * `setup` is the response shape of the `setup_status` / `setup_pick_editor_path`
 * commands; the client normalizes it through isServerMessage before dispatch.
 */
import { describe, it, expect } from "vitest";
import {
  isServerMessage,
  isSetupMessage,
  CLIENT_MESSAGE_TYPES,
} from "@/lib/protocol";

describe("setup message guard", () => {
  it("accepts a full setup snapshot", () => {
    const msg = {
      type: "setup",
      editor_path: "C:\\Games\\EUDEditor3",
      editor_valid: true,
      assets_ready: false,
      codex_resolved: true,
      codex_authed: false,
      setup_required: true,
    };
    expect(isSetupMessage(msg)).toBe(true);
    expect(isServerMessage(msg)).toBe(true);
  });

  it("accepts an optional stable error code", () => {
    expect(
      isSetupMessage({
        type: "setup",
        editor_path: "",
        editor_valid: false,
        assets_ready: false,
        codex_resolved: true,
        codex_authed: false,
        setup_required: true,
        error: "invalid_editor_folder",
      }),
    ).toBe(true);
  });

  it("rejects structurally invalid snapshots", () => {
    expect(isSetupMessage({ type: "setup" })).toBe(false);
    expect(
      isSetupMessage({
        type: "setup",
        editor_path: "",
        editor_valid: "yes", // wrong type
        assets_ready: false,
        codex_resolved: true,
        codex_authed: false,
        setup_required: true,
      }),
    ).toBe(false);
    expect(
      isSetupMessage({
        type: "setup",
        editor_path: "",
        editor_valid: false,
        assets_ready: false,
        codex_resolved: true,
        codex_authed: false,
        setup_required: true,
        error: 42, // wrong type
      }),
    ).toBe(false);
    // The codex gate fields are required (the setup screen blocks ready on them).
    expect(
      isSetupMessage({
        type: "setup",
        editor_path: "",
        editor_valid: false,
        assets_ready: false,
        setup_required: true,
      }),
    ).toBe(false);
  });

  it("exposes the setup client commands in the closed set", () => {
    expect(CLIENT_MESSAGE_TYPES).toContain("setup_status");
    expect(CLIENT_MESSAGE_TYPES).toContain("setup_pick_editor_path");
    expect(CLIENT_MESSAGE_TYPES).toContain("bootstrap_run");
  });
});
