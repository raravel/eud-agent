import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { MapPropertiesDialog } from "./MapPropertiesDialog";
import type {
  CandidateStateView,
  MapContextSnapshot,
  MapDigest,
  MapPropertiesRequest,
} from "./mapProtocol";

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

function digest(): MapDigest {
  return {
    map: { width: 128, height: 128, tileset: "jungle", title: "협동 방어전", description: "네 명이 지킨다" },
    units: [],
    doodads: [],
    sprites: [],
    locations: [],
    startLocations: [],
    players: Array.from({ length: 12 }, (_, slot) => ({
      player: `P${slot + 1}`,
      controller: slot < 8 ? "human" : slot === 11 ? "neutral" : "inactive",
      controllerId: slot < 8 ? 6 : slot === 11 ? 7 : 0,
      race: slot < 8 ? "userSelectable" : slot === 11 ? "neutral" : "inactive",
      raceId: slot < 8 ? 5 : slot === 11 ? 4 : 7,
      ...(slot < 8 ? { force: 1 } : {}),
    })),
    forces: [1, 2, 3, 4].map((force) => ({
      force,
      name: `Force ${force}`,
      players: [],
      flags: { randomStartLocation: false, allies: true, alliedVictory: true, sharedVision: false },
    })),
  };
}

function context(overrides: Partial<MapDigest> = {}): MapContextSnapshot {
  return {
    revision,
    savedSourceNotice: "saved",
    sourceFileSize: 1024,
    starcraftPath: "C:\\StarCraft",
    digest: { ...digest(), ...overrides },
  };
}

function candidate(overrides: Partial<CandidateStateView> = {}): CandidateStateView {
  return {
    sessionId: "map-session",
    baseline: revision,
    currentRevision: 0,
    currentHash: "base",
    revisionKey: "r0:base",
    revisions: [],
    selections: [],
    stale: false,
    sourceDiverged: false,
    canApply: false,
    canUndo: false,
    ...overrides,
  };
}

async function choose(name: string | RegExp, option: string | RegExp) {
  await userEvent.click(screen.getByRole("combobox", { name }));
  await userEvent.click(await screen.findByRole("option", { name: option }));
}

describe("MapPropertiesDialog", () => {
  it("loads the digest, enables Save only after a change and sends the full document", async () => {
    const onSave = vi.fn(async (_properties: MapPropertiesRequest) => undefined);
    const onOpenChange = vi.fn();
    render(
      <MapPropertiesDialog open context={context()} candidate={candidate()} onOpenChange={onOpenChange} onSave={onSave} />,
    );

    expect(screen.getByRole("dialog", { name: "맵 속성" })).toBeInTheDocument();
    expect(screen.getByLabelText("맵 제목")).toHaveValue("협동 방어전");
    expect(screen.getByLabelText("맵 설명")).toHaveValue("네 명이 지킨다");
    const save = screen.getByRole("button", { name: "저장" });
    expect(save).toBeDisabled();

    await userEvent.clear(screen.getByLabelText("맵 제목"));
    await userEvent.type(screen.getByLabelText("맵 제목"), "새 제목");
    expect(save).toBeEnabled();

    await userEvent.click(screen.getByRole("tab", { name: "플레이어" }));
    expect(screen.getAllByRole("combobox", { name: /^P\d+ 타입$/u })).toHaveLength(12);
    expect(screen.getByLabelText("P9 포스 없음")).toBeInTheDocument();
    await choose("P2 타입", "컴퓨터");
    await choose("P2 종족", "저그");

    await userEvent.click(screen.getByRole("tab", { name: "포스" }));
    await userEvent.clear(screen.getByLabelText("포스 2 이름"));
    await userEvent.type(screen.getByLabelText("포스 2 이름"), "방어");
    await userEvent.click(screen.getByRole("button", { name: "2팀으로 나누기" }));
    expect(screen.getByLabelText("포스 2 구성원")).toHaveTextContent("P5, P6, P7, P8 · 4명");

    await userEvent.click(screen.getByRole("button", { name: "저장" }));
    await waitFor(() => expect(onSave).toHaveBeenCalledOnce());
    const request = onSave.mock.calls[0]![0];
    expect(request.title).toBe("새 제목");
    expect(request.description).toBe("네 명이 지킨다");
    expect(request.players).toHaveLength(12);
    expect(request.players[1]).toEqual({ type: "computer", race: "zerg", force: 0 });
    expect(request.players.slice(0, 8).map((player) => player.force)).toEqual([0, 0, 0, 0, 1, 1, 1, 1]);
    expect(request.players[11]).toEqual({ type: "neutral", race: "neutral" });
    expect(request.forces.map((force) => force.name)).toEqual(["Force 1", "방어", "Force 3", "Force 4"]);
    await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
  }, 20_000);

  it("refuses to save while an unapplied candidate revision exists", () => {
    render(
      <MapPropertiesDialog
        open
        context={context()}
        candidate={candidate({ currentRevision: 1, canApply: true })}
        onOpenChange={vi.fn()}
        onSave={vi.fn(async () => undefined)}
      />,
    );
    expect(screen.getByRole("status")).toHaveTextContent(
      "적용하지 않은 후보 revision이 있어 맵 속성을 저장할 수 없습니다. 먼저 적용하거나 폐기해 주세요.",
    );
    expect(screen.getByRole("button", { name: "저장" })).toBeDisabled();
    expect(screen.getByLabelText("맵 제목")).toBeDisabled();
  });

  it("keeps the dialog open and shows the backend message when saving fails", async () => {
    const onSave = vi.fn(async () => {
      throw new Error("원본 맵이 다른 프로그램에서 열려 있습니다. SCMDraft에서 저장 후 닫아 주세요.");
    });
    const onOpenChange = vi.fn();
    render(
      <MapPropertiesDialog open context={context()} candidate={candidate()} onOpenChange={onOpenChange} onSave={onSave} />,
    );
    await userEvent.type(screen.getByLabelText("맵 설명"), "!");
    await userEvent.click(screen.getByRole("button", { name: "저장" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("SCMDraft에서 저장 후 닫아 주세요.");
    expect(onOpenChange).not.toHaveBeenCalledWith(false);
    expect(screen.getByRole("button", { name: "저장" })).toBeEnabled();
  });

  it("blocks saving when the digest carries no player or force information", () => {
    render(
      <MapPropertiesDialog
        open
        context={context({ players: undefined })}
        candidate={candidate()}
        onOpenChange={vi.fn()}
        onSave={vi.fn(async () => undefined)}
      />,
    );
    expect(screen.getByRole("status")).toHaveTextContent("플레이어·포스 정보를 읽지 못했습니다");
    expect(screen.getByRole("button", { name: "저장" })).toBeDisabled();
    expect(screen.queryByLabelText("맵 제목")).not.toBeInTheDocument();
  });
});
