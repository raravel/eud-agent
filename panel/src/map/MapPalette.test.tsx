import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { MapPalette } from "./MapPalette";

const api = vi.hoisted(() => ({
  mapCatalog: vi.fn(),
  mapThumbnail: vi.fn(),
}));

vi.mock("./mapProtocol", () => ({
  mapCatalog: api.mapCatalog,
  mapThumbnail: api.mapThumbnail,
}));

describe("Map palette paging", () => {
  beforeEach(() => {
    api.mapCatalog.mockReset();
    api.mapThumbnail.mockReset();
    api.mapThumbnail.mockRejectedValue(new Error("thumbnail omitted by test"));
    api.mapCatalog.mockImplementation(
      async ({ kind, offset }: { kind: string; offset: number }) => ({
        schema: "eud-map-catalog/1",
        kind,
        tileset: "jungle",
        total: 250,
        offset,
        entries: [
          {
            id: offset,
            name: `Entry ${offset}`,
            fingerprint: `entry-${offset}`,
            group: Math.floor(offset / 16),
            variant: offset % 16,
            graphicsValid: true,
          },
        ],
      }),
    );
  });

  it("requests bounded pages and exposes keyboard-operable previous/next controls", async () => {
    render(
      <MapPalette
        sessionId="map-session"
        tileset="jungle"
        locations={[]}
        selections={[]}
        onMention={vi.fn()}
        onStampMention={vi.fn()}
        onStampPlace={vi.fn()}
        onLocation={vi.fn()}
        onNewLocation={vi.fn()}
      />,
    );

    expect(
      await screen.findByRole("button", { name: /Entry 0, 그룹 0, 변형 0 프롬프트에 추가/ }),
    ).toBeInTheDocument();
    expect(api.mapCatalog).toHaveBeenLastCalledWith(
      expect.objectContaining({ kind: "tiles", offset: 0, limit: 100 }),
    );
    expect(screen.getByText("1–1 / 250")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "이전" })).toBeDisabled();

    await userEvent.click(screen.getByRole("button", { name: "다음" }));
    expect(
      await screen.findByRole("button", { name: /Entry 100, 그룹 6, 변형 4 프롬프트에 추가/ }),
    ).toBeInTheDocument();
    await waitFor(() =>
      expect(api.mapCatalog).toHaveBeenLastCalledWith(
        expect.objectContaining({ kind: "tiles", offset: 100, limit: 100 }),
      ),
    );
    expect(screen.getByRole("button", { name: "이전" })).toBeEnabled();
  });

  it("runs at most one visible thumbnail render at a time", async () => {
    const first = Promise.withResolvers<Blob>();
    const second = Promise.withResolvers<Blob>();
    api.mapCatalog.mockResolvedValue({
      schema: "eud-map-catalog/1",
      kind: "tiles",
      tileset: "jungle",
      total: 2,
      offset: 0,
      entries: [
        { id: 1, name: "Entry 1", fingerprint: "entry-1", group: 0, variant: 1, graphicsValid: true },
        { id: 2, name: "Entry 2", fingerprint: "entry-2", group: 0, variant: 2, graphicsValid: true },
      ],
    });
    api.mapThumbnail
      .mockImplementationOnce(() => first.promise)
      .mockImplementationOnce(() => second.promise);

    render(
      <MapPalette
        sessionId="map-session"
        tileset="jungle"
        locations={[]}
        selections={[]}
        onMention={vi.fn()}
        onStampMention={vi.fn()}
        onStampPlace={vi.fn()}
        onLocation={vi.fn()}
        onNewLocation={vi.fn()}
      />,
    );

    expect(
      await screen.findByRole("button", { name: /Entry 1, 그룹 0, 변형 1 프롬프트에 추가/ }),
    ).toBeInTheDocument();
    await waitFor(() => expect(api.mapThumbnail).toHaveBeenCalledTimes(1));
    first.resolve(new Blob());
    await waitFor(() => expect(api.mapThumbnail).toHaveBeenCalledTimes(2));
    second.resolve(new Blob());
  });

  it("adds exact tiles by default and exposes semantic brushes as a separate mode", async () => {
    const exactTile = {
      id: 33,
      name: "Tile 33",
      fingerprint: "tile-33",
      group: 2,
      variant: 1,
      graphicsValid: true,
    };
    const semanticBrush = {
      id: 4,
      name: "High Dirt",
      fingerprint: "brush-4",
      previewTile: 64,
      graphicsValid: true,
    };
    api.mapCatalog.mockImplementation(async ({ kind }: { kind: string }) => ({
      schema: "eud-map-catalog/1",
      kind,
      tileset: "jungle",
      total: 1,
      offset: 0,
      entries: [kind === "tiles" ? exactTile : semanticBrush],
    }));
    const onMention = vi.fn();

    render(
      <MapPalette
        sessionId="map-session"
        tileset="jungle"
        locations={[]}
        selections={[]}
        onMention={onMention}
        onStampMention={vi.fn()}
        onStampPlace={vi.fn()}
        onLocation={vi.fn()}
        onNewLocation={vi.fn()}
      />,
    );

    await userEvent.click(
      await screen.findByRole("button", {
        name: /Tile 33, 그룹 2, 변형 1 프롬프트에 추가/,
      }),
    );
    expect(onMention).toHaveBeenCalledWith(exactTile, "terrain", "exactTile");

    await userEvent.click(screen.getByRole("button", { name: "지형 브러시" }));
    await waitFor(() =>
      expect(api.mapCatalog).toHaveBeenLastCalledWith(
        expect.objectContaining({ kind: "brushes", offset: 0, limit: 100 }),
      ),
    );
    expect(await screen.findByText("High Dirt")).toBeInTheDocument();
  });

  it("registers saved regions as live stamp palette entries with place and mention actions", async () => {
    const selection = {
      id: "selection-a",
      label: "영역 A",
      sourceRevision: "r1:hash",
      role: "target" as const,
      layers: ["terrain" as const, "units" as const],
      bounds: { left: 5, top: 5, right: 43, bottom: 35 },
      selectedCells: 1_140,
      rows: [{ y: 5, spans: [[5, 43] as [number, number]] }],
      snapshotHash: "snapshot-a",
    };
    const onStampPlace = vi.fn();
    const onStampMention = vi.fn();
    render(
      <MapPalette
        sessionId="map-session"
        tileset="jungle"
        locations={[]}
        selections={[selection]}
        onMention={vi.fn()}
        onStampMention={onStampMention}
        onStampPlace={onStampPlace}
        onLocation={vi.fn()}
        onNewLocation={vi.fn()}
      />,
    );

    expect(screen.getByText("38×30 · 1,140셀")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "영역 A 스탬프 배치" }));
    expect(onStampPlace).toHaveBeenCalledWith(selection);
    await userEvent.click(
      screen.getByRole("button", { name: "영역 A 스탬프를 프롬프트에 추가" }),
    );
    expect(onStampMention).toHaveBeenCalledWith(selection);
  });
});
