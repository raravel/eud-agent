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
  it("archives streamed prose and tools in their arrival order", () => {
    let turn = createMapTurn();
    let cursor = createMapTurnCursor();
    const apply = (
      kind: string,
      detail: string,
      data?: { args?: string; result?: string; status?: string },
    ) => {
      const next = reduceMapTurnEvent(turn, cursor, kind, detail, data);
      turn = next.turn;
      cursor = next.cursor;
    };

    apply("reasoning", "요청을 분석합니다.");
    apply("delta", "먼저 맵을 확인합니다.");
    apply("tool_call", "map_status", { args: "{}" });
    apply("tool_result", "map_status", {
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
