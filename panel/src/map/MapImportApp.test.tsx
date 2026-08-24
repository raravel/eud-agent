import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({
  bootstrap: vi.fn(),
  pick: vi.fn(),
  objects: vi.fn(),
  list: vi.fn(),
  save: vi.fn(),
  renderSource: vi.fn(),
  canvasProps: vi.fn(),
  minimapProps: vi.fn(),
}));

vi.mock("./importProtocol", () => ({
  mapImportBootstrap: api.bootstrap,
  mapImportSourcePick: api.pick,
  mapImportSourceObjects: api.objects,
  mapImportStampList: api.list,
  mapImportStampSave: api.save,
  mapImportRenderSource: api.renderSource,
}));
vi.mock("./MapCanvas", () => ({
  MapCanvas: (props: {
    renderSource: { key: string };
    selectionShape: string;
    selectionOperation: string;
    onActiveCells(cells: Set<string>): void;
  }) => {
    api.canvasProps(props);
    return (
      <button type="button" onClick={() => props.onActiveCells(new Set(["2,4", "3,4"]))}>
        소스 캔버스 선택
      </button>
    );
  },
}));
vi.mock("./MapMinimap", () => ({
  MapMinimap: (props: { renderSource: { key: string } }) => {
    api.minimapProps(props);
    return <div>소스 미니맵</div>;
  },
}));

import MapImportApp from "./MapImportApp";

const destination = {
  projectId: "project",
  displayName: "destination.scx",
  fileSha256: "d".repeat(64),
  tileset: "jungle",
  width: 96,
  height: 96,
};
const source = {
  sourceId: "source-id",
  displayName: "source.scx",
  fileSha256: "a".repeat(64),
  chkSha256: "b".repeat(64),
  tileset: "jungle",
  width: 128,
  height: 64,
  fileSize: 1024,
};

describe("Map Importer", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    api.bootstrap.mockResolvedValue({ destination });
    api.pick.mockResolvedValue(source);
    api.objects.mockResolvedValue({ layer: "units", offset: 0, total: 0, items: [] });
    api.list.mockResolvedValue([]);
    api.save.mockResolvedValue({ id: "import-a" });
    api.renderSource.mockReturnValue({ key: "import-source", render: vi.fn() });
  });

  it("loads a pinned source into the shared canvas/minimap and saves canonical rows with all six layers", async () => {
    render(<MapImportApp />);
    await userEvent.click((await screen.findAllByRole("button", { name: "SCX/SCM 선택" }))[0]);

    expect(await screen.findByText("source.scx")).toBeInTheDocument();
    expect(screen.getByText(/jungle · 128×64/)).toBeInTheDocument();
    expect(screen.getByText("소스 미니맵")).toBeInTheDocument();
    expect(api.canvasProps.mock.calls.at(-1)?.[0].renderSource.key).toBe("import-source");
    expect(api.minimapProps.mock.calls.at(-1)?.[0].renderSource.key).toBe("import-source");

    await userEvent.click(screen.getByRole("button", { name: "자유 마스크" }));
    await userEvent.click(screen.getByRole("button", { name: "추가" }));
    expect(api.canvasProps.mock.calls.at(-1)?.[0]).toEqual(
      expect.objectContaining({ selectionShape: "free", selectionOperation: "add" }),
    );
    await userEvent.click(screen.getByRole("button", { name: "소스 캔버스 선택" }));
    await userEvent.click(screen.getByRole("button", { name: "프로젝트 팔레트에 추가" }));

    expect(api.save).toHaveBeenCalledWith({
      sourceId: "source-id",
      label: "가져온 영역 A",
      rows: [{ y: 4, spans: [[2, 4]] }],
      layers: ["terrain", "units", "buildings", "doodads", "sprites", "locations"],
    });
    expect(api.list).toHaveBeenCalledTimes(2);
  });

  it("keeps a different-tileset source visible while disabling palette save", async () => {
    api.pick.mockResolvedValue({ ...source, tileset: "desert" });
    render(<MapImportApp />);
    await userEvent.click((await screen.findAllByRole("button", { name: "SCX/SCM 선택" }))[0]);
    await userEvent.click(screen.getByRole("button", { name: "소스 캔버스 선택" }));

    expect(screen.getByText("타일셋 불일치")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "프로젝트 팔레트에 추가" }),
    ).toBeDisabled();
    await waitFor(() => expect(api.save).not.toHaveBeenCalled());
  });
});
