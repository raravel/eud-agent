import { invoke } from "@tauri-apps/api/core";
import type { SessionMeta, SessionRecord } from "@/lib/protocol";

export type Tileset =
  | "badlands"
  | "platform"
  | "installation"
  | "ashworld"
  | "jungle"
  | "desert"
  | "arctic"
  | "twilight";

export type MapLayer =
  | "terrain"
  | "units"
  | "buildings"
  | "doodads"
  | "sprites"
  | "locations";

export type SelectionRole = "target" | "reference" | "protect" | "anchor";
export type SelectionOperation = "replace" | "add" | "subtract" | "invert" | "clear";
export type SelectionShape = "rectangle" | "free";
export type MapView = "original" | "candidate" | "diff" | "draft";

export interface TileRect {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

export interface RowSpan {
  y: number;
  spans: [number, number][];
}

export interface SelectionMask {
  id: string;
  label: string;
  sourceRevision: string;
  role: SelectionRole;
  layers: MapLayer[];
  bounds: TileRect;
  selectedCells: number;
  rows: RowSpan[];
}
export interface SavedSelection extends SelectionMask {
  snapshotHash: string;
}


export interface MapRevision {
  projectId: string;
  sourcePath: string;
  fileSha256: string;
  chkSha256: string;
  mtimeNs: string;
  tileset: Tileset;
  width: number;
  height: number;
}

export interface MapHeader {
  width: number;
  height: number;
  tileset: string;
}

export interface MapUnit {
  type: string;
  typeId: number;
  owner: string;
  x: number;
  y: number;
  tileX: number;
  tileY: number;
  hpPercent: number;
  shieldPercent: number;
  energyPercent: number;
  resources: number;
  stateFlags: number;
  invincible: boolean;
}

export interface MapDoodad {
  ordinal: number;
  typeId: number;
  x: number;
  y: number;
  tileX: number;
  tileY: number;
  owner: number;
  disabled: boolean;
}

export interface MapSprite {
  ordinal: number;
  typeId: number;
  x: number;
  y: number;
  tileX: number;
  tileY: number;
  owner: number;
  flags: number;
  drawAsSprite: boolean;
  disabled: boolean;
}

export interface MapLocation {
  id: number;
  name: string;
  left: number;
  top: number;
  right: number;
  bottom: number;
  tileRect: [number, number, number, number];
  elevationFlags: number;
  inverted?: string;
  anywhere?: boolean;
}

export interface MapDigest {
  map: MapHeader;
  units: MapUnit[];
  doodads: MapDoodad[];
  sprites: MapSprite[];
  locations: MapLocation[];
  startLocations: Array<{ player: string; x: number; y: number; tileX: number; tileY: number }>;
}

export interface MapContextSnapshot {
  revision: MapRevision;
  sourceFileSize: number;
  savedSourceNotice: string;
  starcraftPath: string;
  digest: MapDigest;
}

export interface MapSourceProbe {
  projectId: string;
  sourcePath: string;
  mtimeNs: string;
  fileSize: number;
}

export interface LayerDiffCount {
  added: number;
  removed: number;
  moved: number;
  changed: number;
}

export interface MapDiff {
  terrainCells: number;
  terrainBounds?: TileRect;
  units: LayerDiffCount;
  buildings: LayerDiffCount;
  doodads: LayerDiffCount;
  sprites: LayerDiffCount;
  locations: LayerDiffCount;
  outsideTarget: number;
  protected: number;
  unsupportedSectionChanges: string[];
}

export interface VerificationReport {
  valid: boolean;
  errors: string[];
  warnings: string[];
  diff: MapDiff;
  candidateSha256: string;
  canonicalDigest: string;
  extraAssetsDigest: string;
}

export interface CandidateRevisionView {
  revision: number;
  parent: number;
  requestId: string;
  mapSha256: string;
  diff: MapDiff;
  verification: VerificationReport;
}

export interface CandidateStateView {
  sessionId: string;
  baseline: MapRevision;
  currentRevision: number;
  currentHash: string;
  revisionKey: string;
  revisions: CandidateRevisionView[];
  selections: SavedSelection[];
  stale: boolean;
  canApply: boolean;
  canUndo: boolean;
}

export interface MapBootstrapResponse {
  context: MapContextSnapshot;
  candidate: CandidateStateView;
  session: SessionRecord;
}

export type MapObjectKind = "unit" | "building" | "doodad" | "sprite";
export interface MapObjectRef {
  kind: MapObjectKind;
  ordinal: number;
  semanticFingerprint: string;
  revisionKey: string;
  baselineHash: string;
  candidateId?: string;
}

export type PaletteKind =
  | "semanticTerrain"
  | "exactTile"
  | "unit"
  | "building"
  | "doodad"
  | "sprite"
  | "newLocation";

export interface PaletteRef {
  layer: MapLayer;
  kind: PaletteKind;
  entryId: number;
  tileset: Tileset;
  fingerprint: string;
}

export interface MentionQualifiers {
  owner?: number;
  count?: number;
  facing?: number;
  hpPercent?: number;
  shieldPercent?: number;
  energyPercent?: number;
  resourceAmount?: number;
  invincible?: boolean;
  locationName?: string;
  locationSelection?: {
    selectionId: string;
    snapshotHash: string;
    sourceRevision: string;
  };
  locationBounds?: TileRect;
}

export type MapMentionSnapshot =
  | { kind: "region"; selectionId: string; snapshotHash: string; sourceRevision: string }
  | { kind: "object"; objectRef: MapObjectRef; role: "subject" | "reference" | "protect" | "anchor" }
  | { kind: "palette"; entry: PaletteRef; qualifiers: MentionQualifiers }
  | { kind: "stamp"; selectionId: string; snapshotHash: string }
  | { kind: "importedStamp"; importId: string; snapshotHash: string }
  | { kind: "location"; locationId: number; revisionKey: string; baselineHash: string };

export interface MentionChip {
  id: string;
  label: string;
  mention: MapMentionSnapshot;
  stale?: boolean;
}

export interface PaletteEntry {
  id: number;
  name: string;
  fingerprint: string;
  previewTile?: number;
  graphicsValid?: boolean;
  terrainType?: number;
  group?: number;
  variant?: number;
  buildability?: number;
  groundHeight?: number;
  walkability?: string;
  width?: number;
  megaTile?: number;
  ramp?: boolean;
  blocksView?: boolean;
  highMinitiles?: number;
  midMinitiles?: number;
  height?: number;
  overlay?: boolean;
  placementWidth?: number;
  placementHeight?: number;
}

export interface CatalogResult {
  schema: string;
  kind: string;
  tileset: string;
  total: number;
  offset: number;
  entries: PaletteEntry[];
}

export interface MapObjectItem {
  object?: MapUnit | MapDoodad | MapSprite;
  objectRef?: MapObjectRef;
  location?: MapLocation;
  revisionKey?: string;
  baselineHash?: string;
}

export interface MapObjectPage {
  layer: string;
  offset: number;
  total: number;
  items: MapObjectItem[];
}
export interface MapDiffMarker {
  layer: MapLayer;
  change: "added" | "removed" | "moved" | "changed";
  ordinal: number;
  bounds: TileRect;
}

export interface MapDiffDetails {
  terrainRows: RowSpan[];
  markers: MapDiffMarker[];
}

export interface MapImageDimensions {
  width: number;
  height: number;
}

export interface MapImagePlacement {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface MapImageDescriptor {
  attachmentId: string;
  name: string;
  mime: string;
  attachmentSha256: string;
  sourceDimensions: MapImageDimensions;
}

export interface MapImageConversionReport {
  sourceDimensions: MapImageDimensions;
  placement: MapImagePlacement;
  changedCells: number;
  changedRows: RowSpan[];
  uniqueTileCount: number;
  walkabilityChangedCells: number;
  heightChangedCells: number;
  protectedConflicts: number;
  outsideAuthorityConflicts: number;
  tileGridSha256: string;
  quantizerVersion: string;
}

export interface MapImagePreviewHeader {
  previewSequence: number;
  descriptor: MapImageDescriptor;
  report: MapImageConversionReport;
  pngByteLength: number;
}

export interface MapImagePreviewResult {
  header: MapImagePreviewHeader;
  preview: Blob;
}

export interface MapImageConfirmResponse {
  previewSequence: number;
  candidate: CandidateStateView;
  report: MapImageConversionReport;
}

export type StampCollisionPolicy = "merge" | "replace";
export type MapStampSourceRef =
  | { kind: "candidateSelection"; selectionId: string; snapshotHash: string }
  | { kind: "imported"; importId: string; snapshotHash: string };


export interface StampDestination {
  x: number;
  y: number;
}

export interface StampLayerCounts {
  units: number;
  buildings: number;
  doodads: number;
  sprites: number;
  locations: number;
}

export interface StampPlacementReport {
  selectionId: string;
  label: string;
  width: number;
  height: number;
  layers: MapLayer[];
  destinations: StampDestination[];
  terrainCellsPerDestination: number;
  source: StampLayerCounts;
  collisions: StampLayerCounts;
  partialCollisions: StampLayerCounts;
  outsideAuthorityCells: number;
  protectedCells: number;
  requiredLocationSlots: number;
  availableLocationSlots: number;
}

export interface MapStampConfirmResponse {
  candidate: CandidateStateView;
  report: StampPlacementReport;
}


export interface MapCropRequest {
  x: number;
  y: number;
  width: number;
  height: number;
  scale: number;
  layers: MapLayer[];
}

export interface MapRenderSource {
  key: string;
  render(command: MapCropRequest): Promise<Blob>;
}

export interface RenderCommand extends MapCropRequest {
  sessionId: string;
  view: MapView;
  requestId?: string;
}

export function binaryBytes(value: unknown): Uint8Array {
  if (value instanceof ArrayBuffer) return new Uint8Array(value);
  if (value instanceof Uint8Array) return value;
  if (ArrayBuffer.isView(value)) {
    return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  }
  throw new Error("Map Agent binary IPC did not return an ArrayBuffer");
}
export function pngBlob(bytes: Uint8Array): Blob {
  const buffer = new ArrayBuffer(bytes.byteLength);
  new Uint8Array(buffer).set(bytes);
  return new Blob([buffer], { type: "image/png" });
}

export function parseImagePreviewEnvelope(value: unknown): MapImagePreviewResult {
  const bytes = binaryBytes(value);
  if (
    bytes.byteLength < 16 ||
    new TextDecoder().decode(bytes.subarray(0, 4)) !== "MIP1"
  ) {
    throw new Error("Map image preview returned an invalid binary envelope");
  }
  const headerLength = new DataView(
    bytes.buffer,
    bytes.byteOffset + 4,
    4,
  ).getUint32(0, true);
  const headerEnd = 8 + headerLength;
  if (headerEnd > bytes.byteLength) {
    throw new Error("Map image preview JSON header is truncated");
  }
  const header = JSON.parse(
    new TextDecoder().decode(bytes.subarray(8, headerEnd)),
  ) as MapImagePreviewHeader;
  const png = bytes.subarray(headerEnd);
  if (
    header.pngByteLength !== png.byteLength ||
    png.byteLength < 8 ||
    png[0] !== 0x89 ||
    png[1] !== 0x50 ||
    png[2] !== 0x4e ||
    png[3] !== 0x47
  ) {
    throw new Error("Map image preview PNG payload is invalid");
  }
  return { header, preview: pngBlob(png) };
}


export async function mapBootstrap(): Promise<MapBootstrapResponse> {
  return invoke<MapBootstrapResponse>("map_agent_bootstrap");
}
export function mapSessionList(): Promise<SessionMeta[]> {
  return invoke("map_agent_session_list");
}

export function mapSessionCreate(): Promise<MapBootstrapResponse> {
  return invoke("map_agent_session_create");
}

export function mapSessionLoad(sessionId: string): Promise<MapBootstrapResponse> {
  return invoke("map_agent_session_load", { sessionId });
}

export function mapSessionRename(
  sessionId: string,
  name: string,
): Promise<SessionMeta> {
  return invoke("map_agent_session_rename", { sessionId, name });
}

export function mapSessionDelete(sessionId: string): Promise<void> {
  return invoke("map_agent_session_delete", { sessionId });
}


export async function mapSourceState(): Promise<MapSourceProbe> {
  return invoke<MapSourceProbe>("map_agent_source_state");
}

export async function mapRender(command: RenderCommand): Promise<Blob> {
  const bytes = binaryBytes(await invoke("map_agent_render", { command }));
  return pngBlob(bytes);
}

export function candidateMapRenderSource(input: {
  sessionId: string;
  revisionKey: string;
  view: MapView;
  requestId?: string;
}): MapRenderSource {
  const view = input.view === "diff" ? "candidate" : input.view;
  return {
    key: `${input.sessionId}|${input.revisionKey}|${view}|${input.requestId ?? ""}`,
    render: (command) =>
      mapRender({
        sessionId: input.sessionId,
        view,
        requestId: input.requestId,
        ...command,
      }),
  };
}

export async function mapThumbnail(command: {
  sessionId: string;
  layer: MapLayer;
  id: number;
  owner?: number;
}): Promise<Blob> {
  const bytes = binaryBytes(await invoke("map_agent_thumbnail", { command }));
  return pngBlob(bytes);
}

export async function mapImagePreview(command: {
  sessionId: string;
  attachmentId: string;
  revisionKey: string;
  placement: MapImagePlacement;
  previewSequence: number;
}): Promise<MapImagePreviewResult> {
  return parseImagePreviewEnvelope(
    await invoke("map_agent_image_preview", { command }),
  );
}

export function mapImageConfirm(command: {
  sessionId: string;
  attachmentId: string;
  revisionKey: string;
  placement: MapImagePlacement;
  previewDigest: string;
  previewSequence: number;
}): Promise<MapImageConfirmResponse> {
  return invoke("map_agent_image_confirm", { command });
}

export function mapStampPreview(command: {
  sessionId: string;
  revisionKey: string;
  source: MapStampSourceRef;
  destinations: StampDestination[];
}): Promise<StampPlacementReport> {
  return invoke("map_agent_stamp_preview", { command });
}

export function mapStampConfirm(command: {
  sessionId: string;
  revisionKey: string;
  source: MapStampSourceRef;
  destinations: StampDestination[];
  collisionPolicy: StampCollisionPolicy;
}): Promise<MapStampConfirmResponse> {
  return invoke("map_agent_stamp_confirm", { command });
}

export function mapImageCancel(sessionId: string): Promise<void> {
  return invoke("map_agent_image_cancel", { sessionId });
}

export function mapCatalog(command: {
  sessionId: string;
  kind: string;
  query?: string;
  offset?: number;
  limit?: number;
}): Promise<CatalogResult> {
  return invoke("map_agent_catalog", { command });
}

export function mapObjects(command: {
  sessionId: string;
  layer: string;
  view?: "candidate" | "draft";
  requestId?: string;
  draftGeneration?: number;
  offset?: number;
  limit?: number;
}): Promise<MapObjectPage> {
  return invoke("map_agent_objects", { command });
}
export function mapDiffDetails(sessionId: string): Promise<MapDiffDetails> {
  return invoke("map_agent_diff_details", { sessionId });
}


export function saveSelection(
  sessionId: string,
  selection: SelectionMask,
): Promise<CandidateStateView> {
  return invoke("map_agent_selection_save", { sessionId, selection });
}

export function deleteSelection(
  sessionId: string,
  selectionId: string,
): Promise<CandidateStateView> {
  return invoke("map_agent_selection_delete", { sessionId, selectionId });
}

export function mapChat(command: {
  sessionId: string;
  text: string;
  attachments: string[];
  candidateRevision: number;
  mentions: MapMentionSnapshot[];
}): Promise<CandidateStateView> {
  return invoke("map_agent_chat", { command });
}

export function mapCancel(sessionId: string): Promise<void> {
  return invoke("map_agent_cancel", { sessionId });
}

export function candidateRevert(
  sessionId: string,
  revision: number,
): Promise<CandidateStateView> {
  return invoke("map_agent_candidate_revert", { sessionId, revision });
}

export function candidateDiscard(sessionId: string): Promise<void> {
  return invoke("map_agent_candidate_discard", { sessionId });
}

export function candidateApply(sessionId: string): Promise<CandidateStateView> {
  return invoke("map_agent_candidate_apply", { sessionId });
}

export function applyUndo(sessionId: string): Promise<CandidateStateView> {
  return invoke("map_agent_apply_undo", { sessionId });
}
