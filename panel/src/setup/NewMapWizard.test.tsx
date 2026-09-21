import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import type { BlankProjectRequest, BrushOption, MapNewOptions } from "@/lib/mapNew";
import type { SetupMessage } from "@/lib/protocol";
import { PROVIDER_IDS, type ProviderStatus } from "@/providers/types";
import { NewMapWizard } from "@/setup/NewMapWizard";

const providers: ProviderStatus[] = PROVIDER_IDS.map((provider) => ({
  provider,
  availability: "unavailable",
  selectedAsDefault: false,
  canInstall: false,
  canImport: false,
  experimental: false,
}));

const openedSetup: SetupMessage = {
  type: "setup",
  projectPath: "C:\\Work\\Arena",
  projectValid: true,
  projectOpened: true,
  euddraftPath: "",
  euddraftValid: false,
  assetsReady: false,
  providers,
  setupRequired: true,
  error: null,
};

const readyOptions: MapNewOptions = {
  starcraft: { available: true, path: "C:\\Games\\StarCraft" },
  tilesets: [
    { id: 0, key: "badlands", label: "배드랜드" },
    { id: 4, key: "jungle", label: "정글" },
  ],
  sizePresets: [64, 96, 128, 192, 256],
  sizeMin: 64,
  sizeMax: 256,
};

const missingOptions: MapNewOptions = {
  ...readyOptions,
  starcraft: { available: false, path: "", reason: "StarCraft data directory could not be resolved" },
};

const jungleBrushes: BrushOption[] = [
  { id: 3, name: "Jungle", graphicsValid: true },
  { id: 5, name: "High Jungle", graphicsValid: true },
];

/** Open a shadcn/Radix Select by its accessible name and pick an option label. */
async function choose(name: string | RegExp, option: string | RegExp) {
  await userEvent.click(screen.getByRole("combobox", { name }));
  await userEvent.click(await screen.findByRole("option", { name: option }));
}

function renderWizard(overrides: Partial<Parameters<typeof NewMapWizard>[0]> = {}) {
  const props = {
    open: true,
    onOpenChange: vi.fn(),
    loadOptions: vi.fn(async () => readyOptions),
    loadBrushes: vi.fn(async () => jungleBrushes),
    pickStarcraft: vi.fn(async () => readyOptions),
    pickDestination: vi.fn(async () => ({ path: "C:\\Work\\Arena", empty: true })),
    create: vi.fn(async () => ({ setup: openedSetup, preview: null })),
    onCreated: vi.fn(),
    ...overrides,
  };
  render(<NewMapWizard {...props} />);
  return props;
}

describe("NewMapWizard", () => {
  it("walks basic → terrain → players and sends one strict request", async () => {
    const create = vi.fn(async (request: BlankProjectRequest) => ({
      setup: openedSetup,
      preview: {
        mapPath: `${request.destination}\\maps\\${request.name}.scx`,
        outputMap: `build/[EUD]${request.name}.scx`,
        width: request.spec.width,
        height: request.spec.height,
        tileset: "jungle",
        players: request.spec.players.length,
        startLocations: request.spec.players.filter((player) => player.start).length,
        previewPng: "iVBORw0KGgo=",
      },
    }));
    const props = renderWizard({ create });

    expect(await screen.findByRole("heading", { name: "빈 맵으로 새 프로젝트" })).toBeInTheDocument();
    const next = screen.getByRole("button", { name: "다음" });
    expect(next).toBeDisabled();

    await userEvent.type(screen.getByLabelText("프로젝트 이름"), "Arena");
    expect(screen.getByText(/build\/\[EUD\]Arena\.scx/u)).toBeInTheDocument();
    expect(next).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "새 작업 폴더 선택" }));
    expect(screen.getByText("C:\\Work\\Arena")).toBeInTheDocument();
    expect(next).toBeEnabled();
    await userEvent.click(next);

    await choose("타일셋", "정글");
    await waitFor(() => expect(props.loadBrushes).toHaveBeenLastCalledWith(4));
    await userEvent.click(screen.getByRole("button", { name: "96×96" }));
    await userEvent.clear(screen.getByLabelText("세로"));
    await userEvent.type(screen.getByLabelText("세로"), "64");
    await waitFor(() => expect(screen.getByRole("combobox", { name: "초기 지형" })).toBeEnabled());
    await choose("초기 지형", "High Jungle");
    await userEvent.click(screen.getByRole("button", { name: "다음" }));

    await choose("플레이어 수", "3명");
    await choose("P2 타입", "컴퓨터");
    await choose("P2 종족", "저그");
    await userEvent.click(screen.getByRole("button", { name: "2팀으로 나누기" }));
    expect(screen.getByRole("combobox", { name: "포스 수" })).toHaveTextContent("2개");
    await userEvent.clear(screen.getByLabelText("포스 2 이름"));
    await userEvent.type(screen.getByLabelText("포스 2 이름"), "방어");
    await choose("P2 포스", "방어");
    expect(screen.getByLabelText("포스 1 구성원")).toHaveTextContent("P1 · 1명");
    expect(screen.getByLabelText("포스 2 구성원")).toHaveTextContent("P2, P3 · 2명");
    await userEvent.click(screen.getByRole("button", { name: "맵 만들기" }));

    expect(create).toHaveBeenCalledTimes(1);
    const request = create.mock.calls[0]![0];
    expect(request.destination).toBe("C:\\Work\\Arena");
    expect(request.name).toBe("Arena");
    expect(request.spec).toMatchObject({
      version: "remastered",
      tileset: 4,
      width: 96,
      height: 64,
      terrainType: 5,
      title: "Arena",
    });
    expect(request.spec.forces.map((force) => force.name)).toEqual(["포스 1", "방어"]);
    expect(request.spec.players.map((player) => [player.slot, player.type, player.race, player.force])).toEqual([
      [0, "human", "userSelectable", 0],
      [1, "computer", "zerg", 1],
      [2, "human", "userSelectable", 1],
    ]);
    expect(request.spec.players.every((player) => player.start !== undefined)).toBe(true);

    expect(await screen.findByRole("status")).toHaveTextContent("맵과 프로젝트를 만들었습니다.");
    expect(screen.getByRole("img", { name: /96×64 jungle 맵 미리보기/u })).toBeInTheDocument();
    expect(screen.getByText("build/[EUD]Arena.scx")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "프로젝트 열기" }));
    expect(props.onCreated).toHaveBeenCalledWith(openedSetup);
    expect(props.onOpenChange).toHaveBeenCalledWith(false);
  });

  it("lets the user set up to four named forces and reassigns players when one is removed", async () => {
    const create = vi.fn(async (_request: BlankProjectRequest) => ({ setup: openedSetup, preview: null }));
    renderWizard({ create });
    await userEvent.type(await screen.findByLabelText("프로젝트 이름"), "Arena");
    await userEvent.click(screen.getByRole("button", { name: "새 작업 폴더 선택" }));
    await userEvent.click(screen.getByRole("button", { name: "다음" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "다음" })).toBeEnabled());
    await userEvent.click(screen.getByRole("button", { name: "다음" }));

    await choose("플레이어 수", "5명");
    expect(screen.getByRole("button", { name: "각자 개별 포스" })).toBeDisabled();
    await choose("포스 수", "4개");
    expect(screen.getAllByLabelText(/^포스 \d 이름$/u)).toHaveLength(4);
    await userEvent.clear(screen.getByLabelText("포스 4 이름"));
    await userEvent.type(screen.getByLabelText("포스 4 이름"), "관전");
    await choose("P5 포스", "관전");
    await choose("P4 포스", "포스 3");
    await choose("P3 포스", "포스 2");
    expect(screen.getByLabelText("포스 4 구성원")).toHaveTextContent("P5 · 1명");

    await choose("포스 수", "3개");
    expect(screen.queryByLabelText("포스 4 이름")).not.toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "P5 포스" })).toHaveTextContent("포스 3");
    expect(screen.getByLabelText("포스 3 구성원")).toHaveTextContent("P4, P5 · 2명");
    await userEvent.click(screen.getByRole("button", { name: "맵 만들기" }));

    const request = create.mock.calls[0]![0];
    expect(request.spec.forces.map((force) => force.name)).toEqual(["포스 1", "포스 2", "포스 3"]);
    expect(request.spec.players.map((player) => player.force)).toEqual([0, 0, 1, 2, 2]);
  });

  it("blocks progress until a StarCraft folder is chosen and lets the user pick it inline", async () => {
    const pickStarcraft = vi.fn(async () => readyOptions);
    const props = renderWizard({ loadOptions: vi.fn(async () => missingOptions), pickStarcraft });

    expect(await screen.findByText("StarCraft 설치 폴더가 필요합니다.")).toBeInTheDocument();
    expect(screen.getByText("StarCraft data directory could not be resolved")).toBeInTheDocument();
    await userEvent.type(screen.getByLabelText("프로젝트 이름"), "Arena");
    await userEvent.click(screen.getByRole("button", { name: "새 작업 폴더 선택" }));
    expect(screen.getByRole("button", { name: "다음" })).toBeDisabled();
    expect(props.loadBrushes).not.toHaveBeenCalled();

    await userEvent.click(screen.getByRole("button", { name: "StarCraft 폴더 선택" }));
    expect(pickStarcraft).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(screen.queryByText("StarCraft 설치 폴더가 필요합니다.")).not.toBeInTheDocument());
    expect(screen.getByRole("button", { name: "다음" })).toBeEnabled();
    await waitFor(() => expect(props.loadBrushes).toHaveBeenCalled());
  });

  it("surfaces creation failures with the backend detail and keeps the wizard open", async () => {
    const create = vi.fn(async () => ({
      setup: { ...openedSetup, projectOpened: false, error: "project_create_failed: project destination must be empty: C:\\Work\\Arena" },
      preview: null,
    }));
    const props = renderWizard({ create });

    await userEvent.type(await screen.findByLabelText("프로젝트 이름"), "Arena");
    await userEvent.click(screen.getByRole("button", { name: "새 작업 폴더 선택" }));
    await userEvent.click(screen.getByRole("button", { name: "다음" }));
    await screen.findByRole("combobox", { name: "초기 지형" });
    await waitFor(() => expect(screen.getByRole("button", { name: "다음" })).toBeEnabled());
    await userEvent.click(screen.getByRole("button", { name: "다음" }));
    await userEvent.click(screen.getByRole("button", { name: "맵 만들기" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("비어 있지 않습니다");
    expect(screen.getByText(/project destination must be empty/u)).toBeInTheDocument();
    expect(props.onCreated).not.toHaveBeenCalled();
    expect(props.onOpenChange).not.toHaveBeenCalledWith(false);
    expect(screen.getByRole("button", { name: "맵 만들기" })).toBeEnabled();
  });

  it("blocks the terrain step when brushes fail to load and offers the StarCraft picker", async () => {
    const loadBrushes = vi
      .fn<(tileset: number) => Promise<BrushOption[]>>()
      .mockResolvedValueOnce(jungleBrushes)
      .mockRejectedValueOnce(new Error("cannot open StarCraft data at: C:\Games"));
    const props = renderWizard({ loadBrushes });
    await userEvent.type(await screen.findByLabelText("프로젝트 이름"), "Arena");
    await userEvent.click(screen.getByRole("button", { name: "새 작업 폴더 선택" }));
    await userEvent.click(screen.getByRole("button", { name: "다음" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "다음" })).toBeEnabled());

    await choose("타일셋", "배드랜드");
    expect(await screen.findByRole("alert")).toHaveTextContent("지형 브러시를 불러오지 못했습니다");
    expect(screen.getByRole("button", { name: "다음" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "StarCraft 폴더 다시 선택" }));
    expect(props.pickStarcraft).toHaveBeenCalledTimes(1);
  });

  it("treats a non-numeric size as invalid instead of keeping the previous value", async () => {
    renderWizard();
    await userEvent.type(await screen.findByLabelText("프로젝트 이름"), "Arena");
    await userEvent.click(screen.getByRole("button", { name: "새 작업 폴더 선택" }));
    await userEvent.click(screen.getByRole("button", { name: "다음" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "다음" })).toBeEnabled());
    await userEvent.clear(screen.getByLabelText("가로"));
    await userEvent.type(screen.getByLabelText("가로"), "12a");
    expect(screen.getByRole("alert")).toHaveTextContent("정수");
    expect(screen.getByLabelText("가로")).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByRole("button", { name: "다음" })).toBeDisabled();
  });

  it("rejects invalid names and sizes before anything is sent", async () => {
    const props = renderWizard();
    await userEvent.type(await screen.findByLabelText("프로젝트 이름"), "a/b");
    expect(screen.getByText(/문자를 쓸 수 없습니다/u)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "새 작업 폴더 선택" }));
    expect(screen.getByRole("button", { name: "다음" })).toBeDisabled();
    await userEvent.clear(screen.getByLabelText("프로젝트 이름"));
    await userEvent.type(screen.getByLabelText("프로젝트 이름"), "Arena");
    await userEvent.click(screen.getByRole("button", { name: "다음" }));

    await screen.findByRole("combobox", { name: "초기 지형" });
    await userEvent.clear(screen.getByLabelText("가로"));
    await userEvent.type(screen.getByLabelText("가로"), "300");
    expect(screen.getByRole("alert")).toHaveTextContent("64~256");
    expect(screen.getByRole("button", { name: "다음" })).toBeDisabled();
    expect(props.create).not.toHaveBeenCalled();
  });
});
