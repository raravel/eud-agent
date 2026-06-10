/**
 * Editor-connection send gate (EUD-120).
 *
 * The bridge writes heartbeat.txt/status.txt; the core reports stale/absent
 * heartbeat as an editor-disconnected state. The panel fails open by default
 * (`editorConnected: true`) and gates send only once the backend explicitly
 * reports the editor is not connected.
 */
import { describe, it, expect } from "vitest";
import { createPanelStore } from "@/state/store";

function freshStore() {
  return createPanelStore();
}

describe("editor connection state", () => {
  it("defaults to connected so old cores fail open", () => {
    const s = freshStore().getState();
    expect(s.editorConnected).toBe(true);
  });

  it("allows send with an open project while the editor is connected", () => {
    const store = freshStore();
    store.wsOpen();
    store.applyStatus({ compiling: false, project: "P" });
    store.applyList({ files: [] });

    const s = store.getState();
    expect(s.editorConnected).toBe(true);
    expect(s.hasProject).toBe(true);
    expect(s.canSend).toBe(true);
  });

  it("blocks send after an editor-disconnected error marker", () => {
    const store = freshStore();
    store.wsOpen();
    store.applyStatus({ compiling: false, project: "P" });
    store.applyList({ files: [] });

    store.errorReceived("editor not connected");

    const s = store.getState();
    expect(s.editorConnected).toBe(false);
    expect(s.canSend).toBe(false);
  });

  it("restores the editor connection when a status push arrives", () => {
    const store = freshStore();
    store.wsOpen();
    store.applyStatus({ compiling: false, project: "P" });
    store.applyList({ files: [] });
    store.errorReceived("editor not connected");

    store.applyStatus({ compiling: false, project: "P" });

    expect(store.getState().editorConnected).toBe(true);
    expect(store.getState().canSend).toBe(true);
  });

  it("exposes an explicit editor connection setter", () => {
    const store = freshStore();

    store.editorConnectionChanged(false);
    expect(store.getState().editorConnected).toBe(false);

    store.editorConnectionChanged(true);
    expect(store.getState().editorConnected).toBe(true);
  });

  it("does not change editorConnected for unrelated errors", () => {
    const store = freshStore();

    store.editorConnectionChanged(false);
    store.errorReceived("some unrelated error");
    expect(store.getState().editorConnected).toBe(false);

    store.editorConnectionChanged(true);
    store.errorReceived("some unrelated error");
    expect(store.getState().editorConnected).toBe(true);
  });
});
