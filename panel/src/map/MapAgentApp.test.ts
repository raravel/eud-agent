import { describe, expect, it } from "vitest";

import {
  archiveMapTurn,
  createMapTurn,
  createMapTurnCursor,
  imagePlacementPreviewIsFresh,
  imagePlacementPreviewResponseIsCurrent,
  mapSourceChanged,
  mapSourceProbeChanged,
  nextSelectionLabel,
  reduceMapTurnEvent,
  restoreMentionChips,
  staleImportedMentions,
  staleMentions,
  advanceLiveDraftPreview,
  type LiveDraftPreview,
} from "./MapAgentApp";
import type {
  CandidateStateView,
  MapBootstrapResponse,
  MapContextSnapshot,
  MapSourceProbe,
  MentionChip,
} from "./mapProtocol";
import type { ImportedStampView } from "./importProtocol";

function bootstrap(projectId: string, sourcePath: string): MapBootstrapResponse {
  return {
    context: {
      revision: { projectId, sourcePath },
    },
  } as MapBootstrapResponse;
}

describe("Map Agent OpenMapName switching", () => {
  it("treats a source-path change inside the same project as a map switch", () => {
    const current = bootstrap("project", "C:\\maps\\first.scx");
    expect(mapSourceChanged(current, bootstrap("project", "C:\\maps\\first.scx"))).toBe(false);
    expect(mapSourceChanged(current, bootstrap("project", "C:\\maps\\second.scx"))).toBe(true);
    expect(mapSourceChanged(current, bootstrap("other-project", "C:\\maps\\first.scx"))).toBe(true);
  });

  it("detects source metadata edges without requiring a full bootstrap", () => {
    const context = {
      revision: {
        projectId: "project",
        sourcePath: "C:\\maps\\first.scx",
        mtimeNs: "1700000000000000000",
      },
      sourceFileSize: 1024,
    } as MapContextSnapshot;
    const unchanged: MapSourceProbe = {
      projectId: "project",
      sourcePath: "C:\\maps\\first.scx",
      mtimeNs: "1700000000000000000",
      fileSize: 1024,
    };

    expect(mapSourceProbeChanged(context, unchanged)).toBe(false);
    expect(
      mapSourceProbeChanged(context, {
        ...unchanged,
        mtimeNs: "1700000001000000000",
      }),
    ).toBe(true);
    expect(
      mapSourceProbeChanged(context, { ...unchanged, fileSize: 2048 }),
    ).toBe(true);
    expect(
      mapSourceProbeChanged(context, {
        ...unchanged,
        sourcePath: "C:\\maps\\second.scx",
      }),
    ).toBe(true);
  });

  it("advances to the first unused automatic selection label", () => {
    const selections = [
      { label: "영역 A" },
      { label: "사용자 지정" },
      { label: "영역 B" },
    ];
    expect(nextSelectionLabel(selections)).toBe("영역 C");
  });
});

describe("Map image preview ordering", () => {
  const placement = { x: 4, y: 5, width: 16, height: 8 };

  it("accepts only the latest sequence, transform, and candidate revision", () => {
    expect(
      imagePlacementPreviewResponseIsCurrent(
        3,
        3,
        placement,
        placement,
        "r2:hash",
        "r2:hash",
      ),
    ).toBe(true);
    expect(
      imagePlacementPreviewResponseIsCurrent(
        2,
        3,
        placement,
        placement,
        "r2:hash",
        "r2:hash",
      ),
    ).toBe(false);
    expect(
      imagePlacementPreviewResponseIsCurrent(
        3,
        3,
        placement,
        { ...placement, x: 5 },
        "r2:hash",
        "r2:hash",
      ),
    ).toBe(false);
    expect(
      imagePlacementPreviewResponseIsCurrent(
        3,
        3,
        placement,
        placement,
        "r2:hash",
        "r3:other",
      ),
    ).toBe(false);
  });

  it("keeps confirm disabled until digest sequence and transform are fresh", () => {
    const preview = {
      placement,
      previewPlacement: placement,
      previewRevisionKey: "r2:hash",
      requestedSequence: 4,
      acceptedSequence: 4,
    };
    expect(imagePlacementPreviewIsFresh(preview, "r2:hash")).toBe(true);
    expect(
      imagePlacementPreviewIsFresh(
        { ...preview, placement: { ...placement, y: 6 } },
        "r2:hash",
      ),
    ).toBe(false);
    expect(
      imagePlacementPreviewIsFresh({ ...preview, acceptedSequence: 3 }, "r2:hash"),
    ).toBe(false);
    expect(imagePlacementPreviewIsFresh(preview, "r3:other")).toBe(false);
  });
});

describe("Map Agent live draft preview", () => {
  const completedPatch = {
    kind: "tool_result",
    detail: "map_draft_patch",
    status: "completed",
    requestId: "map-request",
    candidateRevision: "r1:hash",
  };

  it("advances once for each successful draft mutation batch", () => {
    let preview: LiveDraftPreview | null = null;
    preview = advanceLiveDraftPreview(preview, completedPatch);
    expect(preview).toEqual({
      requestId: "map-request",
      candidateRevision: "r1:hash",
      generation: 1,
    });

    preview = advanceLiveDraftPreview(preview, {
      ...completedPatch,
      detail: "map_image_place",
    });
    expect(preview?.generation).toBe(2);

    expect(
      advanceLiveDraftPreview(preview, {
        ...completedPatch,
        detail: "map_draft_reset",
        status: "failed",
      }),
    ).toBe(preview);
    expect(
      advanceLiveDraftPreview(preview, {
        ...completedPatch,
        detail: "map_draft_analyze",
      }),
    ).toBe(preview);
  });

  it("starts a new generation sequence for a different request", () => {
    const previous = advanceLiveDraftPreview(null, completedPatch);
    expect(
      advanceLiveDraftPreview(previous, {
        ...completedPatch,
        requestId: "next-request",
        candidateRevision: "r2:next",
      }),
    ).toEqual({
      requestId: "next-request",
      candidateRevision: "r2:next",
      generation: 1,
    });
  });
});

describe("Map Agent conversation timeline", () => {
  it("matches interleaved same-name tool results by call id", () => {
    // Given: two overlapping calls with the same display name and distinct IDs.
    let turn = createMapTurn();
    let cursor = createMapTurnCursor();
    const apply = (
      kind: string,
      detail: string,
      data?: {
        callId?: string;
        args?: string;
        result?: string;
        status?: string;
      },
    ) => {
      const next = reduceMapTurnEvent(turn, cursor, kind, detail, data);
      turn = next.turn;
      cursor = next.cursor;
    };
    apply("tool_call", "map_status", {
      callId: "map-call-a",
      args: '{"scope":"a"}',
    });
    apply("tool_call", "map_status", {
      callId: "map-call-b",
      args: '{"scope":"b"}',
    });

    // When: their results arrive in start order rather than stack order.
    apply("tool_result", "map_status", {
      callId: "map-call-a",
      result: "alpha",
      status: "completed",
    });
    apply("tool_result", "map_status", {
      callId: "map-call-b",
      result: "beta-error",
      status: "failed",
    });

    // Then: each result and terminal state stays with its originating call.
    expect(turn.tools).toMatchObject([
      {
        callId: "map-call-a",
        args: '{"scope":"a"}',
        detail: "alpha",
        state: "done",
      },
      {
        callId: "map-call-b",
        args: '{"scope":"b"}',
        detail: "beta-error",
        state: "failed",
      },
    ]);
  });

  it("preserves current-run orphan terminals without changing a keyed call", () => {
    let turn = createMapTurn();
    let cursor = createMapTurnCursor();
    for (const [kind, callId, result, status] of [
      ["tool_call", "known", undefined, undefined],
      ["tool_result", "unknown", "orphan-error", "failed"],
      ["tool_result", "known", "known-result", "completed"],
      ["tool_result", "known", "duplicate-error", "failed"],
    ] as const) {
      const next = reduceMapTurnEvent(turn, cursor, kind, "map_status", {
        callId,
        ...(result !== undefined ? { result } : {}),
        ...(status !== undefined ? { status } : {}),
      });
      turn = next.turn;
      cursor = next.cursor;
    }

    expect(turn.tools).toMatchObject([
      { callId: "known", state: "done", detail: "known-result" },
      { callId: "unknown", state: "failed", detail: "orphan-error" },
    ]);
  });

  it("reports id-less starts as information and keeps id-less terminals standalone", () => {
    const initial = createMapTurn();
    const start = reduceMapTurnEvent(
      initial,
      createMapTurnCursor(),
      "tool_call",
      "command",
      { callId: "", args: "cargo test" },
    );
    expect(start.turn.tools).toEqual([]);
    expect(start.unpairedToolStart).toEqual({
      name: "command",
      args: "cargo test",
    });

    const terminal = reduceMapTurnEvent(
      start.turn,
      start.cursor,
      "tool_result",
      "command",
      { result: "12 passed", status: "completed" },
    );
    expect(terminal.turn.tools).toMatchObject([
      { name: "command", state: "done", detail: "12 passed" },
    ]);
  });

  it("archives streamed prose and tools in their arrival order", () => {
    let turn = createMapTurn();
    let cursor = createMapTurnCursor();
    const apply = (
      kind: string,
      detail: string,
      data?: {
        callId?: string;
        args?: string;
        result?: string;
        status?: string;
      },
    ) => {
      const next = reduceMapTurnEvent(turn, cursor, kind, detail, data);
      turn = next.turn;
      cursor = next.cursor;
    };

    apply("reasoning", "요청을 분석합니다.");
    apply("delta", "먼저 맵을 확인합니다.");
    apply("tool_call", "map_status", { callId: "map-status", args: "{}" });
    apply("tool_result", "map_status", {
      callId: "map-status",
      result: "loaded",
      status: "completed",
    });
    apply("item_started", "message-2");
    apply("delta", "후보를 만들었습니다.");

    expect(turn.reasoning).toBe("요청을 분석합니다.");
    expect(
      turn.blocks.map((block) =>
        block.type === "text"
          ? `text:${block.text}`
          : `tools:${block.tools.map((tool) => tool.name).join(",")}`,
      ),
    ).toEqual([
      "text:먼저 맵을 확인합니다.",
      "tools:map_status",
      "text:후보를 만들었습니다.",
    ]);

    const archived = archiveMapTurn(turn, "중복되면 안 되는 최종 응답", 10);
    expect(
      archived.entries.map((entry) => `${entry.kind}:${entry.text}`),
    ).toEqual([
      "agent:먼저 맵을 확인합니다.",
      "info:도구 호출 1건 — map_status",
      "agent:후보를 만들었습니다.",
    ]);
    expect(archived.entries[1].tools?.[0]).toMatchObject({
      name: "map_status",
      state: "done",
      args: "{}",
      detail: "loaded",
    });
    expect(archived.logSequence).toBe(13);
  });
});


describe("Candidate mention freshness", () => {
  const candidate: CandidateStateView = {
    sessionId: "map-session",
    baseline: {
      projectId: "project",
      sourcePath: "C:\\maps\\source.scx",
      fileSha256: "baseline",
      chkSha256: "chk",
      mtimeNs: "1700000000000000000",
      tileset: "jungle",
      width: 64,
      height: 64,
    },
    currentRevision: 2,
    currentHash: "candidate",
    revisionKey: "r2:candidate",
    revisions: [],
    selections: [
      {
        id: "target",
        label: "영역 A",
        sourceRevision: "r2:candidate",
        role: "target",
        layers: ["terrain"],
        bounds: { left: 0, top: 0, right: 1, bottom: 1 },
        selectedCells: 4,
        rows: [{ y: 0, spans: [[0, 1]] }],
        snapshotHash: "mask-a",
      },
    ],
    stale: false,
    canApply: true,
    canUndo: false,
  };
  const chip: MentionChip = {
    id: "chip",
    label: "target:영역 A",
    mention: {
      kind: "region",
      selectionId: "target",
      snapshotHash: "mask-a",
      sourceRevision: "r1:previous",
    },
  };

  it("rebinds an unchanged saved region to the visible candidate revision", () => {
    const [rebound] = staleMentions([chip], candidate);

    expect(rebound.stale).toBe(false);
    expect(rebound.mention).toMatchObject({
      kind: "region",
      sourceRevision: "r2:candidate",
    });
  });

  it("keeps a changed saved region stale", () => {
    const [stale] = staleMentions(
      [chip],
      {
        ...candidate,
        selections: [
          {
            ...candidate.selections[0],
            snapshotHash: "mask-b",
          },
        ],
      },
    );

    expect(stale.stale).toBe(true);
  });
});


describe("Edited message mention restoration", () => {
  it("rebuilds tray chips from persisted snapshots with live labels when the items still exist", () => {
    const chips = restoreMentionChips(
      [
        { kind: "region", selectionId: "target", snapshotHash: "mask-a", sourceRevision: "r1:a" },
        { kind: "stamp", selectionId: "missing-selection", snapshotHash: "mask-b" },
        { kind: "importedStamp", importId: "import-a", snapshotHash: "snapshot-a" },
        {
          kind: "object",
          objectRef: {
            kind: "unit",
            ordinal: 7,
            semanticFingerprint: "fp",
            revisionKey: "r1:a",
            baselineHash: "baseline",
          },
          role: "subject",
        },
        {
          kind: "palette",
          entry: { layer: "locations", kind: "newLocation", entryId: 0, tileset: "jungle", fingerprint: "new-location/1" },
          qualifiers: {},
        },
        {
          kind: "palette",
          entry: { layer: "units", kind: "unit", entryId: 0, tileset: "jungle", fingerprint: "unit/0" },
          qualifiers: { owner: 1 },
        },
        { kind: "location", locationId: 3, revisionKey: "r1:a", baselineHash: "baseline" },
        { kind: "location", locationId: 9, revisionKey: "r1:a", baselineHash: "baseline" },
      ],
      {
        selections: [
          {
            id: "target",
            label: "영역 A",
            role: "target",
            sourceRevision: "r1:a",
            layers: ["terrain"],
            bounds: { left: 0, top: 0, right: 1, bottom: 1 },
            selectedCells: 4,
            rows: [],
            snapshotHash: "mask-a",
          },
        ],
        locations: [
          { id: 3, name: "Spawn", left: 0, top: 0, right: 32, bottom: 32, tileRect: [0, 0, 1, 1], elevationFlags: 0 },
        ],
        imported: [{ id: "import-a", label: "언덕", snapshotHash: "snapshot-a" } as ImportedStampView],
      },
    );

    expect(chips.map((chip) => chip.label)).toEqual([
      "target:영역 A",
      "stamp:missing-",
      "imported:언덕",
      "instance:unit #7",
      "type:새 로케이션",
      "type:unit #0",
      "location:#3 Spawn",
      "location:#9",
    ]);
    expect(new Set(chips.map((chip) => chip.id)).size).toBe(chips.length);
    expect(chips[5].mention).toMatchObject({ kind: "palette", qualifiers: { owner: 1 } });
  });
});

describe("Imported stamp mention freshness", () => {
  it("marks deleted, unavailable, or snapshot-mismatched imported chips stale", () => {
    const chip = {
      id: "chip",
      label: "imported:언덕",
      mention: {
        kind: "importedStamp" as const,
        importId: "import-a",
        snapshotHash: "snapshot-a",
      },
    };
    const stamp = {
      id: "import-a",
      snapshotHash: "snapshot-a",
      available: true,
      compatible: true,
    } as ImportedStampView;
    expect(staleImportedMentions([chip], [stamp])[0].stale).toBe(false);
    expect(staleImportedMentions([chip], [])[0].stale).toBe(true);
    expect(
      staleImportedMentions(
        [chip],
        [{ ...stamp, snapshotHash: "snapshot-b" }],
      )[0].stale,
    ).toBe(true);
    expect(
      staleImportedMentions([chip], [{ ...stamp, available: false }])[0].stale,
    ).toBe(true);
  });
});
