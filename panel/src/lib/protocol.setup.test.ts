/**
 * Wire-schema guards for native project/euddraft first-run setup messages.
 *
 * Every setup response crosses the runtime guard before dispatch.
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
      project_path: "C:\\Projects\\MyMap",
      project_valid: true,
      euddraft_path: "C:\\Tools\\euddraft.exe",
      euddraft_valid: true,
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
        project_path: "",
        project_valid: false,
        euddraft_path: "",
        euddraft_valid: false,
        assets_ready: false,
        codex_resolved: true,
        codex_authed: false,
        setup_required: true,
        error: "invalid_project_folder",
      }),
    ).toBe(true);
  });

  it("rejects structurally invalid snapshots", () => {
    expect(isSetupMessage({ type: "setup" })).toBe(false);
    expect(
      isSetupMessage({
        type: "setup",
        project_path: "",
        project_valid: "yes", // wrong type
        euddraft_path: "",
        euddraft_valid: false,
        assets_ready: false,
        codex_resolved: true,
        codex_authed: false,
        setup_required: true,
      }),
    ).toBe(false);
    expect(
      isSetupMessage({
        type: "setup",
        project_path: "",
        project_valid: false,
        euddraft_path: "",
        euddraft_valid: false,
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
        project_path: "",
        project_valid: false,
        euddraft_path: "",
        euddraft_valid: false,
        assets_ready: false,
        setup_required: true,
      }),
    ).toBe(false);
  });

  it("exposes the setup client commands in the closed set", () => {
    expect(CLIENT_MESSAGE_TYPES).toContain("setup_status");
    expect(CLIENT_MESSAGE_TYPES).toContain("setup_pick_project_path");
    expect(CLIENT_MESSAGE_TYPES).toContain("setup_create_project");
    expect(CLIENT_MESSAGE_TYPES).toContain("setup_import_e3s");
    expect(CLIENT_MESSAGE_TYPES).toContain("setup_pick_euddraft_path");
    expect(CLIENT_MESSAGE_TYPES).toContain("bootstrap_run");
  });
});
