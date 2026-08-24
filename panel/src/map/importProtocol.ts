import { invoke } from "@tauri-apps/api/core";

import {
  binaryBytes,
  pngBlob,
  type MapLayer,
  type MapObjectPage,
  type MapRenderSource,
  type RowSpan,
  type TileRect,
  type Tileset,
} from "./mapProtocol";

export interface MapImportDestination {
  projectId: string;
  displayName: string;
  fileSha256: string;
  tileset: Tileset;
  width: number;
  height: number;
}

export interface MapImportBootstrap {
  destination: MapImportDestination;
}

export interface MapImportSource {
  sourceId: string;
  displayName: string;
  fileSha256: string;
  chkSha256: string;
  tileset: Tileset;
  width: number;
  height: number;
  fileSize: number;
}

export interface ImportedStamp {
  id: string;
  label: string;
  snapshotHash: string;
  sourceDisplayName: string;
  sourceFileSha256: string;
  sourceChkSha256: string;
  sourceExtension: "scx" | "scm";
  sourceTileset: Tileset;
  sourceWidth: number;
  sourceHeight: number;
  bounds: TileRect;
  selectedCells: number;
  rows: RowSpan[];
  layers: MapLayer[];
  createdAt: string;
}

export interface ImportedStampView extends ImportedStamp {
  available: boolean;
  compatible: boolean;
  unavailableReason?: string;
}

export function mapAgentImportOpen(): Promise<void> {
  return invoke("map_agent_import_open");
}

export function mapImportBootstrap(): Promise<MapImportBootstrap> {
  return invoke("map_import_bootstrap");
}

export function mapImportSourcePick(): Promise<MapImportSource | null> {
  return invoke("map_import_source_pick");
}

export async function mapImportSourceRender(command: {
  sourceId: string;
  x: number;
  y: number;
  width: number;
  height: number;
  scale: number;
  layers: MapLayer[];
}): Promise<Blob> {
  return pngBlob(
    binaryBytes(await invoke("map_import_source_render", { command })),
  );
}

export function mapImportRenderSource(source: MapImportSource): MapRenderSource {
  return {
    key: `imported-source|${source.sourceId}|${source.fileSha256}`,
    render: (command) =>
      mapImportSourceRender({ sourceId: source.sourceId, ...command }),
  };
}

export function mapImportSourceObjects(command: {
  sourceId: string;
  layer: Exclude<MapLayer, "terrain">;
  offset?: number;
  limit?: number;
}): Promise<MapObjectPage> {
  return invoke("map_import_source_objects", { command });
}

export function mapImportStampSave(command: {
  sourceId: string;
  label: string;
  rows: RowSpan[];
  layers: MapLayer[];
}): Promise<ImportedStampView> {
  return invoke("map_import_stamp_save", { command });
}

export function mapImportStampList(): Promise<ImportedStampView[]> {
  return invoke("map_import_stamp_list");
}

export async function mapImportStampThumbnail(
  importId: string,
  scale = 2,
): Promise<Blob> {
  return pngBlob(
    binaryBytes(
      await invoke("map_import_stamp_thumbnail", {
        command: { importId, scale },
      }),
    ),
  );
}

export function mapImportStampDelete(importId: string): Promise<void> {
  return invoke("map_import_stamp_delete", { importId });
}
