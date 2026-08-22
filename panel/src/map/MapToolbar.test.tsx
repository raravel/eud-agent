import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { MapToolbar } from "./MapToolbar";
import type {
  CandidateStateView,
  MapContextSnapshot,
  MapDiff,
  VerificationReport,
} from "./mapProtocol";

const emptyCounts = { added: 0, removed: 0, moved: 0, changed: 0 };
const emptyDiff: MapDiff = {
  terrainCells: 0,
  units: emptyCounts,
  buildings: emptyCounts,
  doodads: emptyCounts,
  sprites: emptyCounts,
  locations: emptyCounts,
  outsideTarget: 0,
  protected: 0,
  unsupportedSectionChanges: [],
};
const verification: VerificationReport = {
  valid: true,
  errors: [],
  warnings: [],
  diff: emptyDiff,
  candidateSha256: "candidate",
  canonicalDigest: "canonical",
  extraAssetsDigest: "assets",
};
const revision = {
  projectId: "project",
  sourcePath: "C:\\maps\\demo.scx",
  fileSha256: "a".repeat(64),
  chkSha256: "b".repeat(64),
  mtimeNs: "1700000000000000000",
  tileset: "jungle" as const,
  width: 128,
  height: 128,
};
const context: MapContextSnapshot = {
  revision,
  savedSourceNotice: "saved",
  sourceFileSize: 1024,
  starcraftPath: "C:\\StarCraft",
  digest: {
    map: { width: 128, height: 128, tileset: "jungle" },
    units: [],
    doodads: [],
    sprites: [],
    locations: [],
    startLocations: [],
  },
};

function candidate(overrides: Partial<CandidateStateView> = {}): CandidateStateView {
  return {
    sessionId: "map-session",
    baseline: revision,
    currentRevision: 1,
    currentHash: "candidate",
    revisionKey: "r1:candidate",
    revisions: [
      {
        revision: 1,
        parent: 0,
        requestId: "request",
        mapSha256: "candidate",
        diff: emptyDiff,
        verification,
      },
    ],
    selections: [],
    stale: false,
    canApply: true,
    canUndo: false,
    ...overrides,
  };
}

const callbacks = {
  changedSource: null,
  reloadingSource: false,
  onView: vi.fn(),
  onRevert: vi.fn(),
  onDiscard: vi.fn(),
  onApply: vi.fn(),
  onUndo: vi.fn(),
  onImagePlace: vi.fn(),
  onReloadSource: vi.fn(),
};

describe("MapToolbar candidate rails", () => {
  it("enables whole-candidate Apply only for a verified non-stale revision", async () => {
    callbacks.onApply.mockClear();
    render(
      <MapToolbar
        context={context}
        candidate={candidate()}
        view="candidate"
        busy={false}
        imagePlacementActive={false}
        {...callbacks}
      />,
    );
    const apply = screen.getByRole("button", { name: "전체 Apply" });
    expect(apply).toBeEnabled();
    await userEvent.click(apply);
    expect(callbacks.onApply).toHaveBeenCalledTimes(1);
  });

  it("shows an explicit candidate cancel action immediately beside Apply", async () => {
    callbacks.onDiscard.mockClear();
    const { rerender } = render(
      <MapToolbar
        context={context}
        candidate={candidate()}
        view="candidate"
        busy={false}
        imagePlacementActive={false}
        {...callbacks}
      />,
    );
    const cancel = screen.getByRole("button", { name: "후보 취소" });
    const apply = screen.getByRole("button", { name: "전체 Apply" });
    expect(cancel).toBeEnabled();
    const actionButtons = [...cancel.parentElement!.querySelectorAll("button")];
    expect(actionButtons.indexOf(apply) - actionButtons.indexOf(cancel)).toBe(1);
    await userEvent.click(cancel);
    expect(callbacks.onDiscard).toHaveBeenCalledOnce();

    rerender(
      <MapToolbar
        context={context}
        candidate={candidate({ currentRevision: 0, canApply: false })}
        view="candidate"
        busy={false}
        imagePlacementActive={false}
        {...callbacks}
      />,
    );
    expect(screen.getByRole("button", { name: "후보 취소" })).toBeDisabled();
  });

  it("uses the shadcn revision picker for candidate reverts", async () => {
    callbacks.onRevert.mockClear();
    render(
      <MapToolbar
        context={context}
        candidate={candidate()}
        view="candidate"
        busy={false}
        imagePlacementActive={false}
        {...callbacks}
      />,
    );
    await userEvent.click(screen.getByLabelText("후보 revision"));
    await userEvent.click(screen.getByRole("option", { name: "r0 · 기준" }));
    expect(callbacks.onRevert).toHaveBeenCalledWith(0);
  });

  it("blocks Apply and offers a new preserved work item for a stale source", async () => {
    callbacks.onReloadSource.mockClear();
    render(
      <MapToolbar
        context={context}
        candidate={candidate({ stale: true, canApply: false })}
        view="candidate"
        busy={false}
        imagePlacementActive={false}
        {...callbacks}
        changedSource={{
          projectId: "project",
          sourcePath: "C:\\maps\\demo.scx",
          mtimeNs: "1700000001000000000",
          fileSize: 2048,
        }}
      />,
    );
    expect(screen.getByRole("button", { name: "전체 Apply" })).toBeDisabled();
    expect(screen.getByText(/원본 변경됨/)).toBeInTheDocument();

    const reload = screen.getByRole("button", {
      name: "변경된 원본으로 새 작업",
    });
    expect(reload).toBeEnabled();
    await userEvent.click(reload);
    expect(callbacks.onReloadSource).toHaveBeenCalledOnce();
  });

  it("offers direct photo placement unless source or workbench state is unsafe", async () => {
    callbacks.onImagePlace.mockClear();
    const { rerender } = render(
      <MapToolbar
        context={context}
        candidate={candidate()}
        view="candidate"
        busy={false}
        imagePlacementActive={false}
        {...callbacks}
      />,
    );
    const photo = screen.getByRole("button", { name: "사진 배치" });
    expect(photo).toBeEnabled();
    await userEvent.click(photo);
    expect(callbacks.onImagePlace).toHaveBeenCalledOnce();

    rerender(
      <MapToolbar
        context={context}
        candidate={candidate({ stale: true })}
        view="candidate"
        busy={false}
        imagePlacementActive={false}
        {...callbacks}
      />,
    );
    expect(screen.getByRole("button", { name: "사진 배치" })).toBeDisabled();
  });

  it("marks a live draft as an uncommitted preview", () => {
    render(
      <MapToolbar
        context={context}
        candidate={candidate()}
        view="candidate"
        busy
        imagePlacementActive={false}
        {...callbacks}
        liveDraftActive
      />,
    );

    expect(screen.getByRole("status")).toHaveTextContent("수정 중 미리보기");
    expect(screen.getByRole("button", { name: "후보" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
  });
});
