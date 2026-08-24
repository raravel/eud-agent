import { beforeEach, describe, expect, it, vi } from "vitest";

const tauri = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: tauri.invoke }));

import {
  mapAgentImportOpen,
  mapImportSourceRender,
  mapImportStampSave,
} from "./importProtocol";

describe("Map Importer IPC", () => {
  beforeEach(() => {
    tauri.invoke.mockReset();
  });

  it("opens the singleton importer without a path or routing argument", async () => {
    tauri.invoke.mockResolvedValue(undefined);
    await mapAgentImportOpen();
    expect(tauri.invoke).toHaveBeenCalledWith("map_agent_import_open");
  });

  it("renders through opaque sourceId binary IPC", async () => {
    tauri.invoke.mockResolvedValue(
      new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    );
    await mapImportSourceRender({
      sourceId: "source-id",
      x: 1,
      y: 2,
      width: 3,
      height: 4,
      scale: 2,
      layers: ["terrain", "locations"],
    });
    expect(tauri.invoke).toHaveBeenCalledWith("map_import_source_render", {
      command: {
        sourceId: "source-id",
        x: 1,
        y: 2,
        width: 3,
        height: 4,
        scale: 2,
        layers: ["terrain", "locations"],
      },
    });
  });

  it("saves canonical geometry and layer names without filesystem or raw map fields", async () => {
    tauri.invoke.mockResolvedValue({});
    await mapImportStampSave({
      sourceId: "source-id",
      label: "언덕 입구",
      rows: [{ y: 4, spans: [[2, 5]] }],
      layers: ["terrain", "units", "buildings", "doodads", "sprites", "locations"],
    });
    const [, payload] = tauri.invoke.mock.calls[0];
    expect(payload).toEqual({
      command: {
        sourceId: "source-id",
        label: "언덕 입구",
        rows: [{ y: 4, spans: [[2, 5]] }],
        layers: ["terrain", "units", "buildings", "doodads", "sprites", "locations"],
      },
    });
    expect(JSON.stringify(payload).toLowerCase()).not.toMatch(
      /path|picker|blob|scenario\.chk|mtxm/,
    );
  });
});
