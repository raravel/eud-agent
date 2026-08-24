import { beforeEach, describe, expect, it, vi } from "vitest";

const tauri = vi.hoisted(() => ({ invoke: vi.fn(async () => undefined) }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: tauri.invoke }));

import {
  parseImagePreviewEnvelope,
  mapImageCancel,
  mapSessionCreate,
  mapStampConfirm,
  mapStampPreview,
  mapObjects,
  mapSessionDelete,
  mapSessionList,
  mapSessionLoad,
  mapSessionRename,
} from "./mapProtocol";

beforeEach(() => {
  tauri.invoke.mockClear();
});

describe("Map Agent session IPC", () => {
  it("uses map-window-specific commands and confined session identifiers", async () => {
    await mapSessionList();
    await mapSessionCreate();
    await mapSessionLoad("map-session-2");
    await mapSessionRename("map-session-2", "멀티 배치 검토");
    await mapSessionDelete("map-session-2");

    expect(tauri.invoke.mock.calls).toEqual([
      ["map_agent_session_list"],
      ["map_agent_session_create"],
      ["map_agent_session_load", { sessionId: "map-session-2" }],
      [
        "map_agent_session_rename",
        { sessionId: "map-session-2", name: "멀티 배치 검토" },
      ],
      ["map_agent_session_delete", { sessionId: "map-session-2" }],
    ]);
  });
});

describe("Map draft object IPC", () => {
  it("binds object pages to the active request draft", async () => {
    await mapObjects({
      sessionId: "map-session",
      layer: "locations",
      view: "draft",
      requestId: "map-request",
      offset: 0,
      draftGeneration: 2,
      limit: 500,
    });

    expect(tauri.invoke).toHaveBeenCalledWith("map_agent_objects", {
      command: {
        sessionId: "map-session",
        layer: "locations",
        view: "draft",
        requestId: "map-request",
        offset: 0,
        draftGeneration: 2,
        limit: 500,
      },
    });
  });
});

describe("Map stamp placement IPC", () => {
  it("sends a strict source ref, top-left destinations, and explicit collision policy", async () => {
    const destinations = [
      { x: 43, y: 5 },
      { x: 5, y: 35 },
      { x: 43, y: 35 },
    ];
    const source = {
      kind: "candidateSelection" as const,
      selectionId: "selection-a",
      snapshotHash: "snapshot-a",
    };
    await mapStampPreview({
      sessionId: "map-session",
      revisionKey: "r1:hash",
      source,
      destinations,
    });
    await mapStampConfirm({
      sessionId: "map-session",
      revisionKey: "r1:hash",
      source,
      destinations,
      collisionPolicy: "merge",
    });

    expect(tauri.invoke.mock.calls).toEqual([
      [
        "map_agent_stamp_preview",
        {
          command: {
            sessionId: "map-session",
            revisionKey: "r1:hash",
            source,
            destinations,
          },
        },
      ],
      [
        "map_agent_stamp_confirm",
        {
          command: {
            sessionId: "map-session",
            revisionKey: "r1:hash",
            source,
            destinations,
            collisionPolicy: "merge",
          },
        },
      ],
    ]);
  });
});

describe("Map image preview binary IPC", () => {
  it("separates the bounded JSON header from PNG bytes without base64", () => {
    const png = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
    const header = new TextEncoder().encode(
      JSON.stringify({
        previewSequence: 7,
        descriptor: {
          attachmentId: "attachment",
          name: "terrain.png",
          mime: "image/png",
          attachmentSha256: "sha",
          sourceDimensions: { width: 2, height: 1 },
        },
        report: {
          sourceDimensions: { width: 2, height: 1 },
          placement: { x: 1, y: 2, width: 2, height: 1 },
          changedCells: 2,
          changedRows: [{ y: 2, spans: [[1, 3]] }],
          uniqueTileCount: 2,
          walkabilityChangedCells: 1,
          heightChangedCells: 0,
          protectedConflicts: 0,
          outsideAuthorityConflicts: 0,
          tileGridSha256: "digest",
          quantizerVersion: "sd-bayer8-v1",
        },
        pngByteLength: png.byteLength,
      }),
    );
    const envelope = new Uint8Array(8 + header.byteLength + png.byteLength);
    envelope.set(new TextEncoder().encode("MIP1"), 0);
    new DataView(envelope.buffer).setUint32(4, header.byteLength, true);
    envelope.set(header, 8);
    envelope.set(png, 8 + header.byteLength);

    const parsed = parseImagePreviewEnvelope(envelope);
    expect(parsed.header.previewSequence).toBe(7);
    expect(parsed.header.report.tileGridSha256).toBe("digest");
    expect(parsed.preview.type).toBe("image/png");
    expect(parsed.preview.size).toBe(png.byteLength);
  });

  it("rejects truncated and mismatched preview envelopes", () => {
    expect(() => parseImagePreviewEnvelope(new Uint8Array([1, 2, 3]))).toThrow(
      "invalid binary envelope",
    );
    const truncated = new Uint8Array(16);
    truncated.set(new TextEncoder().encode("MIP1"), 0);
    new DataView(truncated.buffer).setUint32(4, 20, true);
    expect(() => parseImagePreviewEnvelope(truncated)).toThrow("truncated");
  });
});

describe("Map image placement IPC", () => {
  it("releases only the current session image cache on cancel", async () => {
    await mapImageCancel("map-session-7");
    expect(tauri.invoke).toHaveBeenCalledWith("map_agent_image_cancel", {
      sessionId: "map-session-7",
    });
  });
});
