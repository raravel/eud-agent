import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { PhysicalPosition, PhysicalSize } from "@tauri-apps/api/dpi";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getAllWindows, getCurrentWindow } from "@tauri-apps/api/window";
import { Box, Layers3, LocateFixed, RefreshCw, X } from "lucide-react";

import type {
  AgentEventData,
  AgentTool,
  LogKind,
  TurnState,
} from "@/state/store";
import { Button } from "@/components/ui/button";
import type {
  AskAnswer,
  AskQuestion,
  BackendSessionActivity,
  SessionMeta,
} from "@/lib/protocol";
import { discardAttachment, stageAttachment } from "@/lib/attachments";
import {
  attentionNotify,
  codexModelSettingsGet,
  codexModelSettingsSave,
  compactSession,
  isAgentTurnEndTransition,
  type ChatAttachment,
  type CodexModelSettings,
  type ContextUsage,
} from "@/lib/ipc";
import { MapAgentPanel, type MapConversationEntry } from "./MapAgentPanel";
import { MapCanvas } from "./MapCanvas";
import { MapMinimap } from "./MapMinimap";
import { MapPalette } from "./MapPalette";
import { MapToolbar } from "./MapToolbar";
import { MapWorkbench } from "./MapWorkbench";
import { MapSessionHistoryDialog } from "./MapSessionHistoryDialog";
import { SelectionToolbar } from "./SelectionToolbar";
import { ImagePlacementControls } from "./ImagePlacementControls";
import { StampPlacementControls } from "./StampPlacementControls";
import { CandidateControls } from "./CandidateControls";
import { buildSelectionMask, cellsToRows, rowsToCells } from "./selectionMask";
import {
  initialImagePlacement,
  sameImagePlacement,
} from "./imagePlacement";
import type { TileViewport } from "./canvasTransform";
import type { SpatialObject } from "./spatialIndex";
import {
  applyUndo,
  candidateApply,
  candidateDiscard,
  candidateRevert,
  deleteSelection,
  mapBootstrap,
  mapSessionCreate,
  mapSessionDelete,
  mapSessionList,
  mapSessionLoad,
  mapSessionRename,
  mapCancel,
  mapChat,
  mapDiffDetails,
  mapImageConfirm,
  mapImageCancel,
  mapImagePreview,
  mapStampConfirm,
  mapStampPreview,
  mapObjects,
  mapSourceState,
  saveSelection,
  type CandidateStateView,
  type MapBootstrapResponse,
  type MapContextSnapshot,
  type MapDiffDetails,
  type MapLayer,
  type MapImageConversionReport,
  type MapImagePlacement,
  type MapMentionSnapshot,
  type MapLocation,
  type MapObjectItem,
  type MapView,
  type MapSourceProbe,
  type MentionChip,
  type MentionQualifiers,
  type PaletteEntry,
  type PaletteKind,
  type StampCollisionPolicy,
  type StampDestination,
  type StampLayerCounts,
  type StampPlacementReport,
  type SavedSelection,
  type SelectionOperation,
  type SelectionRole,
  type SelectionShape,
} from "./mapProtocol";

const allLayers: MapLayer[] = [
  "terrain",
  "doodads",
  "sprites",
  "units",
  "buildings",
  "locations",
];
function stampLayerCountTotal(counts: StampLayerCounts): number {
  return counts.units + counts.buildings + counts.doodads + counts.sprites + counts.locations;
}


interface AskState {
  requestId: string;
  questions: AskQuestion[];
  submitting: boolean;
}

interface DirectImagePlacement {
  attachment: ChatAttachment;
  sourceBitmap: ImageBitmap;
  resultBitmap: ImageBitmap | null;
  sourceDimensions: { width: number; height: number };
  placement: MapImagePlacement;
  previewMode: "original" | "result";
  report?: MapImageConversionReport;
  previewPlacement?: MapImagePlacement;
  previewRevisionKey?: string;
  requestedSequence: number;
  acceptedSequence: number;
  previewLoading: boolean;
  confirming: boolean;
  error?: string;
}
interface DirectStampPlacement {
  selection: SavedSelection;
  destination: StampDestination;
  report?: StampPlacementReport;
  requestedSequence: number;
  acceptedSequence: number;
  previewLoading: boolean;
  confirming: boolean;
  error?: string;
}


export function imagePlacementPreviewIsFresh(
  placement: Pick<
    DirectImagePlacement,
    | "placement"
    | "previewPlacement"
    | "previewRevisionKey"
    | "requestedSequence"
    | "acceptedSequence"
  >,
  revisionKey: string,
): boolean {
  return (
    placement.previewPlacement !== undefined &&
    sameImagePlacement(placement.placement, placement.previewPlacement) &&
    placement.previewRevisionKey === revisionKey &&
    placement.requestedSequence === placement.acceptedSequence
  );
}

export function imagePlacementPreviewResponseIsCurrent(
  responseSequence: number,
  latestSequence: number,
  responsePlacement: MapImagePlacement,
  currentPlacement: MapImagePlacement,
  responseRevisionKey: string,
  currentRevisionKey: string,
): boolean {
  return (
    responseSequence === latestSequence &&
    sameImagePlacement(responsePlacement, currentPlacement) &&
    responseRevisionKey === currentRevisionKey
  );
}

interface PendingAskSnapshot extends AskState {
  sessionId: string;
  candidateRevision?: string;
}

interface PersistedSurfaceState {
  view?: MapView;
  layers?: MapLayer[];
  interactionMode?: "select" | "inspect" | "pan";
}

function loadSurfaceState(): PersistedSurfaceState {
  try {
    return JSON.parse(localStorage.getItem("map-agent.surface/1") ?? "{}") as PersistedSurfaceState;
  } catch {
    return {};
  }
}
interface WindowGeometry {
  width?: number;
  height?: number;
  x?: number;
  y?: number;
}

function loadWindowGeometry(): WindowGeometry {
  try {
    return JSON.parse(localStorage.getItem("map-agent.window/1") ?? "{}") as WindowGeometry;
  } catch {
    return {};
  }
}

const MAP_LOG_KINDS: Record<LogKind, true> = {
  info: true,
  you: true,
  agent: true,
  progress: true,
  ok: true,
  warn: true,
  error: true,
};

function isMapLogKind(kind: string): kind is LogKind {
  return Object.prototype.hasOwnProperty.call(MAP_LOG_KINDS, kind);
}

function conversationFromSession(
  bootstrap: MapBootstrapResponse,
): MapConversationEntry[] {
  const log = bootstrap.session.panelLog?.log ?? [];
  return log.flatMap((entry) => {
    if (!isMapLogKind(entry.kind)) return [];
    const persisted = entry as typeof entry & {
      mapMentions?: MapMentionSnapshot[];
    };
    const tools: AgentTool[] | undefined = entry.tools?.map((tool) => ({
      id: tool.id,
      name: tool.name,
      state:
        tool.state === "failed"
          ? "failed"
          : tool.state === "running"
            ? "running"
            : "done",
      args: tool.args,
      detail: tool.detail,
    }));
    return [
      {
        id: entry.id,
        kind: entry.kind,
        text: entry.text,
        mapMentions: persisted.mapMentions,
        attachments: entry.attachments,
        ...(tools && tools.length > 0 ? { tools } : {}),
      },
    ];
  });
}

function panelLogFromConversation(
  conversation: MapConversationEntry[],
  logSeq: number,
) {
  return {
    schemaVersion: 2,
    logSeq,
    log: conversation.map(({ mapMentions, ...entry }) => ({
      ...entry,
      mapMentions,
    })),
  };
}

export function mapSourceChanged(
  current: MapBootstrapResponse,
  next: MapBootstrapResponse,
): boolean {
  return (
    current.context.revision.projectId !== next.context.revision.projectId ||
    current.context.revision.sourcePath !== next.context.revision.sourcePath
  );
}

function sourceProbeFromContext(context: MapContextSnapshot): MapSourceProbe {
  return {
    projectId: context.revision.projectId,
    sourcePath: context.revision.sourcePath,
    mtimeNs: context.revision.mtimeNs,
    fileSize: context.sourceFileSize,
  };
}

function sameSourceProbe(left: MapSourceProbe, right: MapSourceProbe): boolean {
  return (
    left.projectId === right.projectId &&
    left.sourcePath === right.sourcePath &&
    left.mtimeNs === right.mtimeNs &&
    left.fileSize === right.fileSize
  );
}

export function mapSourceProbeChanged(
  context: MapContextSnapshot,
  current: MapSourceProbe,
): boolean {
  return !sameSourceProbe(sourceProbeFromContext(context), current);
}
function alphabeticLabel(index: number): string {
  let value = "";
  for (let remaining = index + 1; remaining > 0; remaining = Math.floor((remaining - 1) / 26)) {
    value = String.fromCharCode(65 + ((remaining - 1) % 26)) + value;
  }
  return `영역 ${value}`;
}
export function nextSelectionLabel(
  selections: Array<Pick<SavedSelection, "label">>,
): string {
  const labels = new Set(selections.map((selection) => selection.label));
  for (let index = 0; ; index += 1) {
    const candidate = alphabeticLabel(index);
    if (!labels.has(candidate)) return candidate;
  }
}



async function loadAllObjects(
  sessionId: string,
  draft?: Pick<LiveDraftPreview, "requestId" | "generation">,
): Promise<MapObjectItem[]> {
  const layers = ["units", "buildings", "doodads", "sprites", "locations"];
  const pages = await Promise.all(
    layers.map(async (layer) => {
      const items: MapObjectItem[] = [];
      let offset = 0;
      while (true) {
        const page = await mapObjects({
          sessionId,
          layer,
          offset,
          limit: 500,
          ...(draft
            ? {
                view: "draft" as const,
                requestId: draft.requestId,
                draftGeneration: draft.generation,
              }
            : {}),
        });
        items.push(...page.items);
        offset += page.items.length;
        if (offset >= page.total || page.items.length === 0) break;
      }
      return items;
    }),
  );
  return pages.flat();
}

function staleMentions(chips: MentionChip[], candidate: CandidateStateView): MentionChip[] {
  return chips.map((chip) => {
    let stale = false;
    const mention = chip.mention;
    if (mention.kind === "region") {
      const selection = candidate.selections.find(
        (item) => item.id === mention.selectionId,
      );
      stale =
        !selection ||
        selection.snapshotHash !== mention.snapshotHash ||
        selection.sourceRevision !== candidate.revisionKey;
    } else if (mention.kind === "stamp") {
      const selection = candidate.selections.find(
        (item) => item.id === mention.selectionId,
      );
      return {
        ...chip,
        stale: !selection,
        mention: selection
          ? { ...mention, snapshotHash: selection.snapshotHash }
          : mention,
      };
    } else if (mention.kind === "object") {
      stale =
        mention.objectRef.revisionKey !== candidate.revisionKey ||
        mention.objectRef.baselineHash !== candidate.baseline.fileSha256;
    } else if (mention.kind === "location") {
      stale =
        mention.revisionKey !== candidate.revisionKey ||
        mention.baselineHash !== candidate.baseline.fileSha256;
    }
    return { ...chip, stale };
  });
}

export interface LiveDraftPreview {
  requestId: string;
  candidateRevision: string;
  generation: number;
}

interface LiveDraftEvent {
  kind: string;
  detail: string;
  status?: string;
  requestId?: string;
  candidateRevision?: string;
}
const LIVE_DRAFT_MUTATION_TOOLS: Record<string, true> = {
  map_draft_patch: true,
  map_image_place: true,
  map_stamp_place: true,
  map_draft_reset: true,
};

export function advanceLiveDraftPreview(
  current: LiveDraftPreview | null,
  event: LiveDraftEvent,
): LiveDraftPreview | null {
  if (
    event.kind !== "tool_result" ||
    event.status !== "completed" ||
    !Object.prototype.hasOwnProperty.call(LIVE_DRAFT_MUTATION_TOOLS, event.detail) ||
    !event.requestId ||
    !event.candidateRevision
  ) {
    return current;
  }
  return {
    requestId: event.requestId,
    candidateRevision: event.candidateRevision,
    generation:
      current?.requestId === event.requestId &&
      current.candidateRevision === event.candidateRevision
        ? current.generation + 1
        : 1,
  };
}

export interface MapTurnCursor {
  toolSequence: number;
  blockSequence: number;
  nextTextBlockBreak: boolean;
}

export function createMapTurn(): TurnState {
  return {
    reasoning: "",
    answer: "",
    answerStarted: false,
    tools: [],
    blocks: [],
  };
}

export function createMapTurnCursor(): MapTurnCursor {
  return {
    toolSequence: 0,
    blockSequence: 0,
    nextTextBlockBreak: false,
  };
}

export function reduceMapTurnEvent(
  turn: TurnState,
  cursor: MapTurnCursor,
  kind: string,
  detail: string,
  data: AgentEventData = {},
): { turn: TurnState; cursor: MapTurnCursor } {
  const nextCursor = { ...cursor };
  if (kind === "reasoning") {
    return {
      turn: { ...turn, reasoning: turn.reasoning + detail },
      cursor: nextCursor,
    };
  }
  if (kind === "delta") {
    const blocks = turn.blocks.slice();
    const last = blocks[blocks.length - 1];
    if (
      last !== undefined &&
      last.type === "text" &&
      !nextCursor.nextTextBlockBreak
    ) {
      blocks[blocks.length - 1] = { ...last, text: last.text + detail };
    } else {
      nextCursor.blockSequence += 1;
      blocks.push({
        id: nextCursor.blockSequence,
        type: "text",
        text: detail,
      });
    }
    nextCursor.nextTextBlockBreak = false;
    return {
      turn: {
        ...turn,
        answer: turn.answer + detail,
        answerStarted: true,
        blocks,
      },
      cursor: nextCursor,
    };
  }
  if (kind === "tool_call") {
    nextCursor.toolSequence += 1;
    const tool: AgentTool = {
      id: `map-tool-${nextCursor.toolSequence}`,
      name: detail || "tool",
      state: "running",
      ...(data.args ? { args: data.args } : {}),
    };
    const blocks = turn.blocks.slice();
    const last = blocks[blocks.length - 1];
    if (last !== undefined && last.type === "tools") {
      blocks[blocks.length - 1] = {
        ...last,
        tools: [...last.tools, tool],
      };
    } else {
      nextCursor.blockSequence += 1;
      blocks.push({
        id: nextCursor.blockSequence,
        type: "tools",
        tools: [tool],
      });
    }
    nextCursor.nextTextBlockBreak = true;
    return {
      turn: { ...turn, tools: [...turn.tools, tool], blocks },
      cursor: nextCursor,
    };
  }
  if (kind === "tool_result") {
    const failed = data.status !== undefined && data.status !== "completed";
    const complete = (tool: AgentTool): AgentTool => ({
      ...tool,
      state: failed ? "failed" : "done",
      ...(data.result ? { detail: data.result } : {}),
    });
    const tools = turn.tools.slice();
    for (let index = tools.length - 1; index >= 0; index -= 1) {
      if (tools[index].state === "running") {
        tools[index] = complete(tools[index]);
        break;
      }
    }
    const blocks = turn.blocks.slice();
    let completed = false;
    for (
      let blockIndex = blocks.length - 1;
      blockIndex >= 0 && !completed;
      blockIndex -= 1
    ) {
      const block = blocks[blockIndex];
      if (block.type !== "tools") continue;
      for (
        let toolIndex = block.tools.length - 1;
        toolIndex >= 0;
        toolIndex -= 1
      ) {
        if (block.tools[toolIndex].state !== "running") continue;
        const blockTools = block.tools.slice();
        blockTools[toolIndex] = complete(blockTools[toolIndex]);
        blocks[blockIndex] = { ...block, tools: blockTools };
        completed = true;
        break;
      }
    }
    return { turn: { ...turn, tools, blocks }, cursor: nextCursor };
  }
  if (kind === "item_started") {
    nextCursor.nextTextBlockBreak = true;
  }
  return { turn, cursor: nextCursor };
}

export function archiveMapTurn(
  turn: TurnState,
  fallbackText: string,
  logSequence: number,
): { entries: MapConversationEntry[]; logSequence: number } {
  const entries: MapConversationEntry[] = [];
  let sequence = logSequence;
  let hasStreamedText = false;
  for (const block of turn.blocks) {
    if (block.type === "tools") {
      const counts = new Map<string, number>();
      for (const tool of block.tools) {
        counts.set(tool.name, (counts.get(tool.name) ?? 0) + 1);
      }
      const summary = [...counts]
        .map(([name, count]) => (count > 1 ? `${name}×${count}` : name))
        .join(", ");
      sequence += 1;
      entries.push({
        id: sequence,
        kind: "info",
        text: `도구 호출 ${block.tools.length}건 — ${summary}`,
        tools: block.tools,
      });
    } else if (block.text.trim().length > 0) {
      hasStreamedText = true;
      sequence += 1;
      entries.push({
        id: sequence,
        kind: "agent",
        text: block.text,
      });
    }
  }
  if (!hasStreamedText && fallbackText.trim().length > 0) {
    sequence += 1;
    entries.push({
      id: sequence,
      kind: "agent",
      text: fallbackText,
    });
  }
  return { entries, logSequence: sequence };
}

export default function MapAgentApp() {
  const persisted = useMemo(loadSurfaceState, []);
  const [bootstrap, setBootstrap] = useState<MapBootstrapResponse | null>(null);
  const [candidate, setCandidate] = useState<CandidateStateView | null>(null);
  const [changedSource, setChangedSource] = useState<MapSourceProbe | null>(null);
  const [draftObjects, setDraftObjects] = useState<MapObjectItem[]>([]);
  const [objects, setObjects] = useState<MapObjectItem[]>([]);
  const [diffDetails, setDiffDetails] = useState<MapDiffDetails>({
    terrainRows: [],
    markers: [],
  });
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [sessionHistoryOpen, setSessionHistoryOpen] = useState(false);
  const [sessionHistoryLoading, setSessionHistoryLoading] = useState(false);
  const [sessionActionBusy, setSessionActionBusy] = useState(false);
  const [mapSessions, setMapSessions] = useState<SessionMeta[]>([]);
  const [codexSettings, setCodexSettings] =
    useState<CodexModelSettings | null>(null);
  const [codexSettingsBusy, setCodexSettingsBusy] = useState(false);
  const [contextUsage, setContextUsage] = useState<ContextUsage | null>(null);
  const [view, setView] = useState<MapView>(persisted.view ?? "candidate");
  const [layers, setLayers] = useState<MapLayer[]>(persisted.layers ?? allLayers);
  const [interactionMode, setInteractionMode] = useState<"select" | "inspect" | "pan">(
    persisted.interactionMode ?? "select",
  );
  const [activeCells, setActiveCells] = useState(new Set<string>());
  const [selectionShape, setSelectionShape] = useState<SelectionShape>("rectangle");
  const [selectionOperation, setSelectionOperation] = useState<SelectionOperation>("replace");
  const [selectionRole, setSelectionRole] = useState<SelectionRole>("target");
  const [selectionLayers, setSelectionLayers] = useState<MapLayer[]>(["terrain"]);
  const [selectionLabel, setSelectionLabel] = useState("영역 A");
  const [cursor, setCursor] = useState<{ x: number; y: number } | null>(null);
  const [zoom, setZoom] = useState(0.25);
  const [selectedObject, setSelectedObject] = useState<SpatialObject | null>(null);
  const [highlightedObjectId, setHighlightedObjectId] = useState<string>();
  const [highlightedSelectionId, setHighlightedSelectionId] = useState<string>();
  const [focusTarget, setFocusTarget] = useState<{
    bounds?: SavedSelection["bounds"];
    objectId?: string;
    sequence: number;
  }>();
  const [selectionAnchor, setSelectionAnchor] = useState<{ x: number; y: number } | null>(null);
  const [mapViewport, setMapViewport] = useState<TileViewport | null>(null);
  const [viewportTarget, setViewportTarget] = useState<{
    x: number;
    y: number;
    sequence: number;
  }>();
  const [mentions, setMentions] = useState<MentionChip[]>([]);
  const [selectedMentionId, setSelectedMentionId] = useState<string>();
  const [prompt, setPrompt] = useState("");
  const [conversation, setConversation] = useState<MapConversationEntry[]>([]);
  const [turn, setTurn] = useState<TurnState>(createMapTurn);
  const [turnInFlight, setTurnInFlight] = useState(false);
  const [ask, setAsk] = useState<AskState>();
  const [liveDraft, setLiveDraft] = useState<LiveDraftPreview | null>(null);
  const [imagePlacement, setImagePlacement] = useState<DirectImagePlacement | null>(null);
  const [stampPlacement, setStampPlacement] = useState<DirectStampPlacement | null>(null);
  const minimapActiveRows = useMemo(() => cellsToRows(activeCells), [activeCells]);
  const logSequenceRef = useRef(0);
  const sessionIdRef = useRef("");
  const bootstrapRef = useRef<MapBootstrapResponse | null>(null);
  const candidateRef = useRef<CandidateStateView | null>(null);
  const imagePlacementRef = useRef<DirectImagePlacement | null>(null);
  const stampPlacementRef = useRef<DirectStampPlacement | null>(null);
  const imageFileInputRef = useRef<HTMLInputElement>(null);
  const imagePreviewSequenceRef = useRef(0);
  const stampPreviewSequenceRef = useRef(0);
  const draftOverlayRefreshRef = useRef(0);
  const overlayRefreshRef = useRef(0);
  const sourceProbeInFlightRef = useRef(false);
  const eventRevisionRef = useRef("");
  const turnRef = useRef(turn);
  const turnCursorRef = useRef<MapTurnCursor>(createMapTurnCursor());
  const turnInFlightRef = useRef(false);
  const turnEndedRef = useRef(false);
  const recoveredTurnRef = useRef(false);
  const notificationActivityRef = useRef<BackendSessionActivity>("idle");
  const notifiedAskRequestRef = useRef<string | undefined>(undefined);

  const markTurnInFlight = useCallback((active: boolean) => {
    turnInFlightRef.current = active;
    setTurnInFlight(active);
  }, []);

  const resetTurn = useCallback(() => {
    const next = createMapTurn();
    turnRef.current = next;
    turnCursorRef.current = createMapTurnCursor();
    setTurn(next);
  }, []);

  const archiveCurrentTurn = useCallback(
    (fallbackText = "") => {
      const archived = archiveMapTurn(
        turnRef.current,
        fallbackText,
        logSequenceRef.current,
      );
      logSequenceRef.current = archived.logSequence;
      if (archived.entries.length > 0) {
        setConversation((entries) => [...entries, ...archived.entries]);
      }
      resetTurn();
    },
    [resetTurn],
  );

  useEffect(() => {
    bootstrapRef.current = bootstrap;
  }, [bootstrap]);

  useEffect(() => {
    candidateRef.current = candidate;
  }, [candidate]);

  useEffect(() => {
    imagePlacementRef.current = imagePlacement;
  }, [imagePlacement]);
  useEffect(() => {
    stampPlacementRef.current = stampPlacement;
  }, [stampPlacement]);


  const clearImagePlacement = useCallback((discardStaged: boolean) => {
    imagePreviewSequenceRef.current += 1;
    const current = imagePlacementRef.current;
    imagePlacementRef.current = null;
    setImagePlacement(null);
    if (!current) return;
    const sessionId = bootstrapRef.current?.session.id;
    if (sessionId) {
      void mapImageCancel(sessionId).catch(() => undefined);
    }
    current.sourceBitmap.close();
    current.resultBitmap?.close();
    if (discardStaged) {
      void discardAttachment(current.attachment.id).catch(() => {
        // A preview binds the attachment to this session; session cleanup owns it then.
      });
    }
  }, []);
  const clearStampPlacement = useCallback(() => {
    stampPreviewSequenceRef.current += 1;
    stampPlacementRef.current = null;
    setStampPlacement(null);
  }, []);


  useEffect(
    () => () => {
      const current = imagePlacementRef.current;
      current?.sourceBitmap.close();
      current?.resultBitmap?.close();
      const sessionId = bootstrapRef.current?.session.id;
      if (current && sessionId) {
        void mapImageCancel(sessionId).catch(() => undefined);
      }
    },
    [],
  );

  const refreshObjects = useCallback(async (sessionId: string) => {
    const refresh = ++overlayRefreshRef.current;
    setObjects([]);
    setDiffDetails({ terrainRows: [], markers: [] });
    setSelectedObject(null);
    setHighlightedObjectId(undefined);
    const [nextObjects, nextDiff] = await Promise.all([
      loadAllObjects(sessionId),
      mapDiffDetails(sessionId),
    ]);
    if (sessionIdRef.current === sessionId && overlayRefreshRef.current === refresh) {
      setObjects(nextObjects);
      setDiffDetails(nextDiff);
    }
  }, []);

  const clearLiveDraftPreview = useCallback(() => {
    draftOverlayRefreshRef.current += 1;
    setLiveDraft(null);
    setDraftObjects([]);
  }, []);

  const refreshDraftObjects = useCallback(
    async (sessionId: string, draft: LiveDraftPreview) => {
      const refresh = ++draftOverlayRefreshRef.current;
      setDraftObjects([]);
      try {
        const nextObjects = await loadAllObjects(sessionId, draft);
        if (
          sessionIdRef.current === sessionId &&
          draftOverlayRefreshRef.current === refresh
        ) {
          setDraftObjects(nextObjects);
        }
      } catch {
        if (draftOverlayRefreshRef.current === refresh) {
          setDraftObjects([]);
        }
      }
    },
    [],
  );

  useEffect(() => {
    if (!liveDraft) return;
    const sessionId = sessionIdRef.current;
    if (!sessionId) return;
    setSelectedObject(null);
    setHighlightedObjectId(undefined);
    void refreshDraftObjects(sessionId, liveDraft);
  }, [liveDraft, refreshDraftObjects]);

  const loadCodexModelSettings = useCallback(async () => {
    setCodexSettingsBusy(true);
    try {
      setCodexSettings(await codexModelSettingsGet());
    } catch {
      setCodexSettings(null);
      setError("Codex 모델 목록을 불러오지 못했습니다.");
    } finally {
      setCodexSettingsBusy(false);
    }
  }, []);

  const handleCodexSettingsChange = useCallback(
    async (model: string, reasoningEffort: string) => {
      setCodexSettingsBusy(true);
      try {
        setCodexSettings(
          await codexModelSettingsSave(model, reasoningEffort),
        );
      } catch {
        setError("Codex 모델 설정을 저장하지 못했습니다.");
      } finally {
        setCodexSettingsBusy(false);
      }
    },
    [],
  );

  useEffect(() => {
    void loadCodexModelSettings();
  }, [loadCodexModelSettings]);

  const applyBootstrap = useCallback(
    async (next: MapBootstrapResponse) => {
      const currentBootstrap = bootstrapRef.current;
      const currentCandidate = candidateRef.current;
      const sourceChanged = Boolean(
        currentBootstrap && mapSourceChanged(currentBootstrap, next),
      );
      const sessionChanged =
        currentBootstrap?.session.id !== undefined &&
        currentBootstrap.session.id !== next.session.id;
      if (
        currentBootstrap &&
        sourceChanged &&
        currentCandidate &&
        currentCandidate.currentRevision > 0
      ) {
        const discard = window.confirm(
          "현재 맵의 미적용 후보가 있습니다. 기존 후보를 폐기하고 새 맵을 열까요?",
        );
        if (!discard) {
          throw new Error(
            "맵 전환이 취소되었습니다. 이전 OpenMapName으로 돌아간 뒤 다시 시도하세요.",
          );
        }
        await candidateDiscard(currentCandidate.sessionId);
      }
      if (
        imagePlacementRef.current &&
        (sessionChanged ||
          sourceChanged ||
          currentCandidate?.revisionKey !== next.candidate.revisionKey)
      ) {
        clearImagePlacement(true);
      }
      if (
        sessionChanged ||
        sourceChanged ||
        (currentCandidate !== null &&
          currentCandidate.revisionKey !== next.candidate.revisionKey)
      ) {
        clearLiveDraftPreview();
      }
      const restoredConversation = conversationFromSession(next);
      sessionIdRef.current = next.session.id;
      bootstrapRef.current = next;
      candidateRef.current = next.candidate;
      setBootstrap(next);
      setCandidate(next.candidate);
      setChangedSource(
        next.candidate.stale ? sourceProbeFromContext(next.context) : null,
      );
      setContextUsage(next.session.contextUsage ?? null);
      setConversation(restoredConversation);
      logSequenceRef.current = Math.max(
        0,
        ...restoredConversation.map((entry) => entry.id),
      );
      setMapSessions((sessions) =>
        [next.session, ...sessions.filter((item) => item.id !== next.session.id)].sort(
          (left, right) =>
            right.lastConversationAt - left.lastConversationAt ||
            right.createdAt - left.createdAt,
        ),
      );
      if (sessionChanged || sourceChanged) {
        setMentions([]);
        setSelectedMentionId(undefined);
        setPrompt("");
        resetTurn();
        markTurnInFlight(false);
        setAsk(undefined);
        setActiveCells(new Set());
        setSelectedObject(null);
        setHighlightedObjectId(undefined);
        setHighlightedSelectionId(undefined);
        setMapViewport(null);
        setViewportTarget(undefined);
      } else {
        setMentions((chips) => staleMentions(chips, next.candidate));
      }
      await refreshObjects(next.session.id);
    },
    [
      clearImagePlacement,
      clearLiveDraftPreview,
      markTurnInFlight,
      refreshObjects,
      resetTurn,
    ],
  );

  const reload = useCallback(async () => {
    setLoading(true);
    try {
      let next: MapBootstrapResponse;
      if (sessionIdRef.current) {
        try {
          next = await mapSessionLoad(sessionIdRef.current);
        } catch {
          next = await mapBootstrap();
        }
      } else {
        next = await mapBootstrap();
      }
      await applyBootstrap(next);
      setError("");
    } catch (reason) {
      overlayRefreshRef.current += 1;
      sessionIdRef.current = "";
      bootstrapRef.current = null;
      candidateRef.current = null;
      setBootstrap(null);
      setCandidate(null);
      setObjects([]);
      clearLiveDraftPreview();
      setContextUsage(null);
      setDiffDetails({ terrainRows: [], markers: [] });
      setError(String(reason));
    } finally {
      setLoading(false);
    }
  }, [applyBootstrap, clearLiveDraftPreview]);

  useEffect(() => {
    void reload();
  }, [reload]);

  useEffect(() => {
    localStorage.setItem(
      "map-agent.surface/1",
      JSON.stringify({ view, layers, interactionMode }),
    );
  }, [interactionMode, layers, view]);

  useEffect(() => {
    if (!bootstrap || conversation.length === 0) return;
    const panelLog = panelLogFromConversation(
      conversation,
      logSequenceRef.current,
    );
    const timer = window.setTimeout(() => {
      void invoke("session_update_log", { id: bootstrap.session.id, panelLog });
    }, 120);
    return () => window.clearTimeout(timer);
  }, [bootstrap, conversation]);

  const refreshSessionHistory = useCallback(async () => {
    setSessionHistoryLoading(true);
    try {
      setMapSessions(await mapSessionList());
    } catch (reason) {
      setError(`맵 작업 히스토리를 불러오지 못했습니다: ${String(reason)}`);
    } finally {
      setSessionHistoryLoading(false);
    }
  }, []);

  const persistCurrentConversation = useCallback(async () => {
    const current = bootstrapRef.current;
    if (!current) return;
    await invoke("session_update_log", {
      id: current.session.id,
      panelLog: panelLogFromConversation(conversation, logSequenceRef.current),
    });
  }, [conversation]);

  const createSession = useCallback(async () => {
    if (busy || turnInFlight || sessionActionBusy) return;
    setSessionActionBusy(true);
    try {
      await persistCurrentConversation();
      await applyBootstrap(await mapSessionCreate());
      setSessionHistoryOpen(false);
      await refreshSessionHistory();
      setError("");
    } catch (reason) {
      setError(`새 맵 작업을 만들지 못했습니다: ${String(reason)}`);
    } finally {
      setSessionActionBusy(false);
    }
  }, [
    applyBootstrap,
    busy,
    turnInFlight,
    persistCurrentConversation,
    refreshSessionHistory,
    sessionActionBusy,
  ]);

  const loadSession = useCallback(
    async (sessionId: string) => {
      if (
        busy ||
        turnInFlight ||
        sessionActionBusy ||
        sessionId === sessionIdRef.current
      ) {
        return;
      }
      setSessionActionBusy(true);
      try {
        await persistCurrentConversation();
        await applyBootstrap(await mapSessionLoad(sessionId));
        setSessionHistoryOpen(false);
        await refreshSessionHistory();
        setError("");
      } catch (reason) {
        setError(`맵 작업을 불러오지 못했습니다: ${String(reason)}`);
      } finally {
        setSessionActionBusy(false);
      }
    },
    [
      applyBootstrap,
      busy,
      turnInFlight,
      persistCurrentConversation,
      refreshSessionHistory,
      sessionActionBusy,
    ],
  );

  const renameSession = useCallback(async (sessionId: string, name: string) => {
    try {
      const meta = await mapSessionRename(sessionId, name);
      setMapSessions((sessions) =>
        sessions.map((session) => (session.id === sessionId ? meta : session)),
      );
      if (bootstrapRef.current?.session.id === sessionId) {
        const next = {
          ...bootstrapRef.current,
          session: { ...bootstrapRef.current.session, ...meta },
        };
        bootstrapRef.current = next;
        setBootstrap(next);
      }
    } catch (reason) {
      setError(`맵 작업 이름을 바꾸지 못했습니다: ${String(reason)}`);
    }
  }, []);

  const deleteSession = useCallback(async (sessionId: string) => {
    if (sessionId === sessionIdRef.current) return;
    try {
      await mapSessionDelete(sessionId);
      setMapSessions((sessions) =>
        sessions.filter((session) => session.id !== sessionId),
      );
    } catch (reason) {
      setError(`맵 작업을 삭제하지 못했습니다: ${String(reason)}`);
    }
  }, []);

  useEffect(() => {
    if (!bootstrap) return;
    const sessionId = bootstrap.session.id;
    const unlisteners: UnlistenFn[] = [];
    let disposed = false;
    notificationActivityRef.current = "idle";
    notifiedAskRequestRef.current = undefined;
    const register = async () => {
      unlisteners.push(
        await listen<Record<string, unknown>>(
          "session_activity",
          ({ payload }) => {
            if (payload.sessionId !== sessionId) return;
            const activity = payload.activity;
            if (
              activity !== "idle" &&
              activity !== "running_read" &&
              activity !== "waiting_input" &&
              activity !== "running_write" &&
              activity !== "review" &&
              activity !== "error"
            ) {
              return;
            }
            const previous = notificationActivityRef.current;
            notificationActivityRef.current = activity;
            if (isAgentTurnEndTransition(previous, activity)) {
              void attentionNotify(
                "agentTurnComplete",
                !document.hasFocus(),
                sessionId,
              ).catch(() => {
                // Delivery is best-effort and must not disturb settled turn state.
              });
            }
          },
        ),
      );
      unlisteners.push(
        await listen<Record<string, unknown>>(
          "notification_activated",
          ({ payload }) => {
            if (payload.sessionId !== sessionId) return;
            const windowHandle = getCurrentWindow();
            void windowHandle
              .show()
              .then(() => windowHandle.setFocus())
              .catch(() => {
                // Activation focus is best-effort; notification delivery already succeeded.
              });
          },
        ),
      );
      unlisteners.push(
        await listen<Record<string, unknown>>("agent_event", ({ payload }) => {
          if (payload.sessionId !== sessionId) return;
          if (payload.candidateRevision !== eventRevisionRef.current) return;
          if (!turnInFlightRef.current || turnEndedRef.current) return;
          const kind = String(payload.kind ?? "");
          const detail = String(payload.detail ?? "");
          const rawData = (payload.data ?? {}) as Record<string, unknown>;
          const data: AgentEventData = {
            ...(typeof rawData.args === "string"
              ? { args: rawData.args }
              : {}),
            ...(typeof rawData.result === "string"
              ? { result: rawData.result }
              : {}),
            ...(typeof rawData.status === "string"
              ? { status: rawData.status }
              : {}),
          };
          setLiveDraft((current) =>
            advanceLiveDraftPreview(current, {
              kind,
              detail,
              status: data.status,
              requestId:
                typeof payload.requestId === "string"
                  ? payload.requestId
                  : undefined,
              candidateRevision:
                typeof payload.candidateRevision === "string"
                  ? payload.candidateRevision
                  : undefined,
            }),
          );
          const next = reduceMapTurnEvent(
            turnRef.current,
            turnCursorRef.current,
            kind,
            detail,
            data,
          );
          turnRef.current = next.turn;
          turnCursorRef.current = next.cursor;
          setTurn(next.turn);
        }),
      );
      unlisteners.push(
        await listen<Record<string, unknown>>("context_usage", ({ payload }) => {
          if (payload.sessionId !== sessionId) return;
          if (payload.candidateRevision !== eventRevisionRef.current) return;
          setContextUsage((payload.tokenUsage ?? null) as ContextUsage | null);
        }),
      );
      unlisteners.push(
        await listen<Record<string, unknown>>("answer", ({ payload }) => {
          if (payload.sessionId !== sessionId) return;
          if (payload.candidateRevision !== eventRevisionRef.current) return;
          if (turnEndedRef.current) return;
          turnEndedRef.current = true;
          archiveCurrentTurn(String(payload.text ?? ""));
          setAsk(undefined);
          if (recoveredTurnRef.current) {
            recoveredTurnRef.current = false;
            eventRevisionRef.current = "";
            markTurnInFlight(false);
          }
        }),
      );
      unlisteners.push(
        await listen<Record<string, unknown>>("ask", ({ payload }) => {
          if (payload.sessionId !== sessionId) return;
          if (payload.candidateRevision !== eventRevisionRef.current) return;
          if (!turnInFlightRef.current || turnEndedRef.current) return;
          const requestId = String(payload.requestId);
          if (notifiedAskRequestRef.current !== requestId) {
            notifiedAskRequestRef.current = requestId;
            void attentionNotify(
              "askResponseRequired",
              !document.hasFocus(),
              sessionId,
            ).catch(() => {
              // Delivery is best-effort and must not disturb the pending ASK.
            });
          }
          setAsk({
            requestId,
            questions: (payload.questions ?? []) as AskQuestion[],
            submitting: false,
          });
        }),
      );
      unlisteners.push(
        await listen<Record<string, unknown>>("error", ({ payload }) => {
          if (payload.sessionId !== sessionId) return;
          if (payload.candidateRevision !== eventRevisionRef.current) return;
          if (turnEndedRef.current) return;
          turnEndedRef.current = true;
          archiveCurrentTurn();
          logSequenceRef.current += 1;
          setConversation((entries) => [
            ...entries,
            {
              id: logSequenceRef.current,
              kind: "error",
              text: String(
                payload.message ?? "Map Agent 요청이 실패했습니다.",
              ),
            },
          ]);
          setAsk(undefined);
          if (recoveredTurnRef.current) {
            recoveredTurnRef.current = false;
            eventRevisionRef.current = "";
            markTurnInFlight(false);
          }
        }),
      );
      unlisteners.push(
        await listen<CandidateStateView>("map_candidate_state", ({ payload }) => {
          if (payload.sessionId !== sessionId) return;
          candidateRef.current = payload;
          clearLiveDraftPreview();
          setCandidate(payload);
          setMentions((chips) => staleMentions(chips, payload));
          void refreshObjects(sessionId);
        }),
      );
      const pending = await invoke<PendingAskSnapshot | null>("ask_pending", {
        sessionId,
      });
      if (disposed || pending === null || pending.sessionId !== sessionId) return;
      const candidateRevision =
        pending.candidateRevision ?? bootstrap.candidate.revisionKey;
      if (candidateRevision !== bootstrap.candidate.revisionKey) return;
      eventRevisionRef.current = candidateRevision;
      turnEndedRef.current = false;
      recoveredTurnRef.current = true;
      notifiedAskRequestRef.current = pending.requestId;
      markTurnInFlight(true);
      setAsk({
        requestId: pending.requestId,
        questions: pending.questions,
        submitting: false,
      });
    };
    void register().catch((reason) => {
      if (!disposed) setError(String(reason));
    });
    return () => {
      disposed = true;
      for (const unlisten of unlisteners) unlisten();
    };
  }, [
    archiveCurrentTurn,
    bootstrap,
    clearLiveDraftPreview,
    markTurnInFlight,
    refreshObjects,
  ]);

  useEffect(() => {
    const windowHandle = getCurrentWindow();
    const saved = loadWindowGeometry();
    if (saved.width && saved.height) {
      void windowHandle.setSize(new PhysicalSize(saved.width, saved.height));
    }
    if (saved.x !== undefined && saved.y !== undefined) {
      void windowHandle.setPosition(new PhysicalPosition(saved.x, saved.y));
    }
    const unlisteners: UnlistenFn[] = [];
    void windowHandle.onResized(({ payload }) => {
      const current = loadWindowGeometry();
      localStorage.setItem(
        "map-agent.window/1",
        JSON.stringify({ ...current, width: payload.width, height: payload.height }),
      );
    }).then((unlisten) => unlisteners.push(unlisten));
    void windowHandle.onMoved(({ payload }) => {
      const current = loadWindowGeometry();
      localStorage.setItem(
        "map-agent.window/1",
        JSON.stringify({ ...current, x: payload.x, y: payload.y }),
      );
    }).then((unlisten) => unlisteners.push(unlisten));
    return () => {
      for (const unlisten of unlisteners) unlisten();
    };
  }, []);

  const probeSource = useCallback(async () => {
    const context = bootstrapRef.current?.context;
    if (!context || sourceProbeInFlightRef.current) return;
    sourceProbeInFlightRef.current = true;
    try {
      const current = await mapSourceState();
      if (
        bootstrapRef.current?.context !== context ||
        !mapSourceProbeChanged(context, current)
      ) {
        return;
      }
      setChangedSource((previous) =>
        previous && sameSourceProbe(previous, current) ? previous : current,
      );
      setCandidate((value) =>
        value === null || value.stale ? value : { ...value, stale: true },
      );
    } catch {
      // A transient bridge failure must not tear down the usable workbench.
      // Original-file writes still verify the full source hash in the backend.
    } finally {
      sourceProbeInFlightRef.current = false;
    }
  }, []);

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    void getCurrentWindow()
      .onFocusChanged(({ payload }) => {
        if (payload) void probeSource();
      })
      .then((dispose) => {
        unlisten = dispose;
      });
    return () => unlisten?.();
  }, [probeSource]);

  useEffect(() => {
    if (!bootstrap) return;
    const timer = window.setInterval(() => void probeSource(), 2_000);
    return () => window.clearInterval(timer);
  }, [bootstrap, probeSource]);

  const saveActiveSelection = useCallback(async (): Promise<SavedSelection> => {
    if (!bootstrap || !candidate) throw new Error("Map Agent가 준비되지 않았습니다.");
    const id = crypto.randomUUID();
    const selection = buildSelectionMask({
      id,
      label: selectionLabel,
      sourceRevision: candidate.revisionKey,
      role: selectionRole,
      layers: selectionLayers,
      cells: activeCells,
      width: candidate.baseline.width,
      height: candidate.baseline.height,
    });
    const next = await saveSelection(bootstrap.session.id, selection);
    setCandidate(next);
    setSelectionLabel(nextSelectionLabel(next.selections));
    const saved = next.selections.find((item) => item.id === id);
    if (!saved) throw new Error("저장된 선택 스냅샷을 찾지 못했습니다.");
    return saved;
  }, [
    activeCells,
    bootstrap,
    candidate,
    selectionLabel,
    selectionLayers,
    selectionRole,
  ]);

  const addRegionMention = useCallback((selection: SavedSelection) => {
    const chip: MentionChip = {
      id: crypto.randomUUID(),
      label: `${selection.role}:${selection.label}`,
      mention: {
        kind: "region",
        selectionId: selection.id,
        snapshotHash: selection.snapshotHash,
        sourceRevision: selection.sourceRevision,
      },
    };
    setMentions((chips) => [...chips, chip]);
    setSelectedMentionId(chip.id);
  }, []);

  const addStampMention = useCallback((selection: SavedSelection) => {
    const chip: MentionChip = {
      id: crypto.randomUUID(),
      label: `stamp:${selection.label}`,
      mention: {
        kind: "stamp",
        selectionId: selection.id,
        snapshotHash: selection.snapshotHash,
      },
    };
    setMentions((chips) => [...chips, chip]);
    setSelectedMentionId(chip.id);
  }, []);

  const handlePaletteMention = useCallback(
    (entry: PaletteEntry, layer: MapLayer, kind: PaletteKind) => {
      if (!bootstrap) return;
      const chip: MentionChip = {
        id: crypto.randomUUID(),
        label: `type:${entry.name}`,
        mention: {
          kind: "palette",
          entry: {
            layer,
            kind,
            entryId: entry.id,
            tileset: bootstrap.context.revision.tileset,
            fingerprint: entry.fingerprint,
          },
          qualifiers: {},
        },
      };
      setMentions((chips) => [...chips, chip]);
      setSelectedMentionId(chip.id);
    },
    [bootstrap],
  );

  const handleLocationMention = useCallback(
    (location: MapLocation) => {
      if (!candidate) return;
      const chip: MentionChip = {
        id: crypto.randomUUID(),
        label: `location:#${location.id} ${location.name}`,
        mention: {
          kind: "location",
          locationId: location.id,
          revisionKey: candidate.revisionKey,
          baselineHash: candidate.baseline.fileSha256,
        },
      };
      setMentions((chips) => [...chips, chip]);
      setSelectedMentionId(chip.id);
    },
    [candidate],
  );

  const handleNewLocation = useCallback(() => {
    if (!bootstrap) return;
    const chip: MentionChip = {
      id: crypto.randomUUID(),
      label: "type:새 로케이션",
      mention: {
        kind: "palette",
        entry: {
          layer: "locations",
          kind: "newLocation",
          entryId: 0,
          tileset: bootstrap.context.revision.tileset,
          fingerprint: "new-location/1",
        },
        qualifiers: {},
      },
    };
    setMentions((chips) => [...chips, chip]);
    setSelectedMentionId(chip.id);
  }, [bootstrap]);

  const addSelectedObjectMention = useCallback(() => {
    if (!selectedObject || !candidate) return;
    const item = selectedObject.item;
    let chip: MentionChip | null = null;
    if (item.objectRef) {
      chip = {
        id: crypto.randomUUID(),
        label: `instance:${selectedObject.kind} #${item.objectRef.ordinal}`,
        mention: { kind: "object", objectRef: item.objectRef, role: "subject" },
      };
    } else if (item.location) {
      chip = {
        id: crypto.randomUUID(),
        label: `location:#${item.location.id} ${item.location.name}`,
        mention: {
          kind: "location",
          locationId: item.location.id,
          revisionKey: candidate.revisionKey,
          baselineHash: candidate.baseline.fileSha256,
        },
      };
    }
    if (!chip) return;
    setMentions((chips) => [...chips, chip!]);
    setSelectedMentionId(chip.id);
  }, [candidate, selectedObject]);

  const requestImagePlacementPreview = useCallback(
    async (placement: MapImagePlacement) => {
      const current = imagePlacementRef.current;
      const currentBootstrap = bootstrapRef.current;
      const currentCandidate = candidateRef.current;
      if (!current || !currentBootstrap || !currentCandidate) return;
      const sequence = ++imagePreviewSequenceRef.current;
      const pending: DirectImagePlacement = {
        ...current,
        placement,
        requestedSequence: sequence,
        previewLoading: true,
        error: undefined,
      };
      imagePlacementRef.current = pending;
      setImagePlacement(pending);
      try {
        const result = await mapImagePreview({
          sessionId: currentBootstrap.session.id,
          attachmentId: current.attachment.id,
          revisionKey: currentCandidate.revisionKey,
          placement,
          previewSequence: sequence,
        });
        if (
          !imagePlacementPreviewResponseIsCurrent(
            result.header.previewSequence,
            imagePreviewSequenceRef.current,
            placement,
            imagePlacementRef.current?.placement ?? placement,
            currentCandidate.revisionKey,
            candidateRef.current?.revisionKey ?? "",
          )
        ) {
          return;
        }
        const bitmap = await createImageBitmap(result.preview);
        const latest = imagePlacementRef.current;
        const latestCandidate = candidateRef.current;
        if (
          !latest ||
          latest.attachment.id !== current.attachment.id ||
          !imagePlacementPreviewResponseIsCurrent(
            sequence,
            imagePreviewSequenceRef.current,
            placement,
            latest.placement,
            currentCandidate.revisionKey,
            latestCandidate?.revisionKey ?? "",
          )
        ) {
          bitmap.close();
          return;
        }
        latest.resultBitmap?.close();
        const accepted: DirectImagePlacement = {
          ...latest,
          resultBitmap: bitmap,
          report: result.header.report,
          previewPlacement: placement,
          previewRevisionKey: currentCandidate.revisionKey,
          acceptedSequence: sequence,
          previewLoading: false,
          error: undefined,
        };
        imagePlacementRef.current = accepted;
        setImagePlacement(accepted);
      } catch (reason) {
        const latest = imagePlacementRef.current;
        if (
          latest &&
          latest.attachment.id === current.attachment.id &&
          imagePreviewSequenceRef.current === sequence
        ) {
          const failed = {
            ...latest,
            previewLoading: false,
            error: String(reason),
          };
          imagePlacementRef.current = failed;
          setImagePlacement(failed);
        }
      }
    },
    [],
  );

  const beginImagePlacement = useCallback(
    async (file: File) => {
      const currentBootstrap = bootstrapRef.current;
      const currentCandidate = candidateRef.current;
      if (
        !currentBootstrap ||
        !currentCandidate ||
        currentCandidate.stale ||
        busy ||
        turnInFlightRef.current ||
        loading
      ) {
        return;
      }
      clearImagePlacement(true);
      let sourceBitmap: ImageBitmap | null = null;
      let attachment: ChatAttachment | null = null;
      try {
        sourceBitmap = await createImageBitmap(file);
        attachment = await stageAttachment(file);
        if (attachment.kind !== "image") {
          throw new Error("PNG, JPEG, WebP 또는 GIF 이미지를 선택하세요.");
        }
        const sourceDimensions = {
          width: sourceBitmap.width,
          height: sourceBitmap.height,
        };
        const placement = initialImagePlacement(
          sourceDimensions,
          currentCandidate.baseline.width,
          currentCandidate.baseline.height,
        );
        const next: DirectImagePlacement = {
          attachment,
          sourceBitmap,
          resultBitmap: null,
          sourceDimensions,
          placement,
          previewMode: "original",
          requestedSequence: 0,
          acceptedSequence: -1,
          previewLoading: false,
          confirming: false,
        };
        imagePlacementRef.current = next;
        setImagePlacement(next);
        setActiveCells(new Set());
        setSelectedObject(null);
        setView("candidate");
        if (!layers.includes("terrain")) {
          setLayers((current) => [...current, "terrain"]);
        }
        await requestImagePlacementPreview(placement);
      } catch (reason) {
        sourceBitmap?.close();
        if (attachment) {
          void discardAttachment(attachment.id).catch(() => undefined);
        }
        imagePlacementRef.current = null;
        setImagePlacement(null);
        setError(`사진 배치를 시작하지 못했습니다: ${String(reason)}`);
      } finally {
        if (imageFileInputRef.current) imageFileInputRef.current.value = "";
      }
    },
    [busy, clearImagePlacement, layers, loading, requestImagePlacementPreview],
  );

  const updateImagePlacement = useCallback(
    (placement: MapImagePlacement, settled: boolean) => {
      const current = imagePlacementRef.current;
      if (!current || current.confirming) return;
      const next = { ...current, placement, error: undefined };
      imagePlacementRef.current = next;
      setImagePlacement(next);
      if (settled) void requestImagePlacementPreview(placement);
    },
    [requestImagePlacementPreview],
  );

  const confirmImagePlacement = useCallback(async () => {
    const current = imagePlacementRef.current;
    const currentBootstrap = bootstrapRef.current;
    const currentCandidate = candidateRef.current;
    if (
      !current ||
      !currentBootstrap ||
      !currentCandidate ||
      current.confirming ||
      !current.report ||
      current.report.protectedConflicts > 0 ||
      !imagePlacementPreviewIsFresh(current, currentCandidate.revisionKey)
    ) {
      return;
    }
    const confirming = { ...current, confirming: true, error: undefined };
    imagePlacementRef.current = confirming;
    setImagePlacement(confirming);
    try {
      const response = await mapImageConfirm({
        sessionId: currentBootstrap.session.id,
        attachmentId: current.attachment.id,
        revisionKey: currentCandidate.revisionKey,
        placement: current.placement,
        previewDigest: current.report.tileGridSha256,
        previewSequence: current.acceptedSequence,
      });
      if (response.previewSequence !== current.acceptedSequence) {
        throw new Error("사진 확인 응답의 preview sequence가 일치하지 않습니다.");
      }
      candidateRef.current = response.candidate;
      setCandidate(response.candidate);
      setMentions((chips) => staleMentions(chips, response.candidate));
      setView("candidate");
      clearImagePlacement(false);
      await refreshObjects(currentBootstrap.session.id);
    } catch (reason) {
      const latest = imagePlacementRef.current;
      if (latest?.attachment.id === current.attachment.id) {
        const failed = {
          ...latest,
          confirming: false,
          error: String(reason),
        };
        imagePlacementRef.current = failed;
        setImagePlacement(failed);
      }
    }
  }, [clearImagePlacement, refreshObjects]);

  const requestStampPlacementPreview = useCallback(
    async (destination: StampDestination) => {
      const current = stampPlacementRef.current;
      const currentBootstrap = bootstrapRef.current;
      const currentCandidate = candidateRef.current;
      if (!current || !currentBootstrap || !currentCandidate) return;
      const sequence = ++stampPreviewSequenceRef.current;
      const pending: DirectStampPlacement = {
        ...current,
        destination,
        requestedSequence: sequence,
        previewLoading: true,
        error: undefined,
      };
      stampPlacementRef.current = pending;
      setStampPlacement(pending);
      try {
        const report = await mapStampPreview({
          sessionId: currentBootstrap.session.id,
          revisionKey: currentCandidate.revisionKey,
          selectionId: current.selection.id,
          destinations: [destination],
        });
        const latest = stampPlacementRef.current;
        if (
          !latest ||
          latest.selection.id !== current.selection.id ||
          stampPreviewSequenceRef.current !== sequence ||
          latest.destination.x !== destination.x ||
          latest.destination.y !== destination.y ||
          candidateRef.current?.revisionKey !== currentCandidate.revisionKey
        ) {
          return;
        }
        const accepted: DirectStampPlacement = {
          ...latest,
          report,
          acceptedSequence: sequence,
          previewLoading: false,
          error: undefined,
        };
        stampPlacementRef.current = accepted;
        setStampPlacement(accepted);
      } catch (reason) {
        const latest = stampPlacementRef.current;
        if (
          latest?.selection.id === current.selection.id &&
          stampPreviewSequenceRef.current === sequence
        ) {
          const failed = {
            ...latest,
            previewLoading: false,
            error: String(reason),
          };
          stampPlacementRef.current = failed;
          setStampPlacement(failed);
        }
      }
    },
    [],
  );

  const beginStampPlacement = useCallback(
    (selection: SavedSelection) => {
      const currentCandidate = candidateRef.current;
      if (
        !currentCandidate ||
        currentCandidate.stale ||
        busy ||
        turnInFlightRef.current ||
        loading
      ) {
        return;
      }
      clearImagePlacement(true);
      clearStampPlacement();
      const width = selection.bounds.right - selection.bounds.left;
      const height = selection.bounds.bottom - selection.bounds.top;
      const destination = {
        x: Math.min(
          selection.bounds.right,
          Math.max(0, currentCandidate.baseline.width - width),
        ),
        y: Math.min(
          selection.bounds.top,
          Math.max(0, currentCandidate.baseline.height - height),
        ),
      };
      const next: DirectStampPlacement = {
        selection,
        destination,
        requestedSequence: 0,
        acceptedSequence: -1,
        previewLoading: false,
        confirming: false,
      };
      stampPlacementRef.current = next;
      setStampPlacement(next);
      setActiveCells(new Set());
      setSelectedObject(null);
      setView("candidate");
      const stampLayers =
        selection.layers.length === 0 ? allLayers : selection.layers;
      setLayers((current) => Array.from(new Set([...current, ...stampLayers])));
      void requestStampPlacementPreview(destination);
    },
    [
      busy,
      clearImagePlacement,
      clearStampPlacement,
      loading,
      requestStampPlacementPreview,
    ],
  );

  const updateStampPlacement = useCallback(
    (destination: StampDestination, settled: boolean) => {
      const current = stampPlacementRef.current;
      if (!current || current.confirming) return;
      const next = { ...current, destination, error: undefined };
      stampPlacementRef.current = next;
      setStampPlacement(next);
      if (settled) void requestStampPlacementPreview(destination);
    },
    [requestStampPlacementPreview],
  );

  const confirmStampPlacement = useCallback(
    async (collisionPolicy: StampCollisionPolicy) => {
      const current = stampPlacementRef.current;
      const currentBootstrap = bootstrapRef.current;
      const currentCandidate = candidateRef.current;
      if (
        !current ||
        !currentBootstrap ||
        !currentCandidate ||
        current.confirming ||
        !current.report ||
        current.previewLoading ||
        current.requestedSequence !== current.acceptedSequence ||
        current.report.outsideAuthorityCells > 0 ||
        current.report.protectedCells > 0
      ) {
        return;
      }
      const confirming = { ...current, confirming: true, error: undefined };
      stampPlacementRef.current = confirming;
      setStampPlacement(confirming);
      try {
        const response = await mapStampConfirm({
          sessionId: currentBootstrap.session.id,
          revisionKey: currentCandidate.revisionKey,
          selectionId: current.selection.id,
          destinations: [current.destination],
          collisionPolicy,
        });
        candidateRef.current = response.candidate;
        setCandidate(response.candidate);
        setMentions((chips) => staleMentions(chips, response.candidate));
        setView("candidate");
        clearStampPlacement();
        await refreshObjects(currentBootstrap.session.id);
      } catch (reason) {
        const latest = stampPlacementRef.current;
        if (latest?.selection.id === current.selection.id) {
          const failed = {
            ...latest,
            confirming: false,
            error: String(reason),
          };
          stampPlacementRef.current = failed;
          setStampPlacement(failed);
        }
      }
    },
    [clearStampPlacement, refreshObjects],
  );

  const send = useCallback(
    async (attachments: ChatAttachment[]) => {
      if (!bootstrap || !candidate || busy || turnInFlight) return;
      const compactRequested =
        prompt.trim() === "/compact" &&
        mentions.length === 0 &&
        attachments.length === 0;
      if (compactRequested) {
        setPrompt("");
        setBusy(true);
        logSequenceRef.current += 1;
        setConversation((entries) => [
          ...entries,
          {
            id: logSequenceRef.current,
            kind: "info",
            text: "대화 컨텍스트 압축 중…",
          },
        ]);
        try {
          await compactSession(bootstrap.session.id);
          logSequenceRef.current += 1;
          setConversation((entries) => [
            ...entries,
            {
              id: logSequenceRef.current,
              kind: "ok",
              text: "대화 컨텍스트를 압축했습니다.",
            },
          ]);
        } catch (reason) {
          logSequenceRef.current += 1;
          setConversation((entries) => [
            ...entries,
            {
              id: logSequenceRef.current,
              kind: "error",
              text: `대화 컨텍스트를 압축하지 못했습니다: ${String(reason)}`,
            },
          ]);
        } finally {
          setBusy(false);
        }
        return;
      }

      if (mentions.some((chip) => chip.stale)) {
        setError(
          "현재 후보와 맞지 않는 맵 멘션을 제거하거나 다시 선택한 뒤 요청하세요.",
        );
        return;
      }
      const activeMentions = mentions;
      if (
        !prompt.trim() &&
        activeMentions.length === 0 &&
        attachments.length === 0
      ) {
        return;
      }
      recoveredTurnRef.current = false;
      eventRevisionRef.current = candidate.revisionKey;
      clearLiveDraftPreview();
      logSequenceRef.current += 1;
      setConversation((entries) => [
        ...entries,
        {
          id: logSequenceRef.current,
          kind: "you",
          text:
            prompt.trim() ||
            (activeMentions.length > 0
              ? "구조화된 맵 멘션을 반영해 주세요."
              : "첨부 파일을 분석해 주세요."),
          mapMentions: activeMentions.map((chip) => chip.mention),
          attachments,
        },
      ]);
      resetTurn();
      turnEndedRef.current = false;
      markTurnInFlight(true);
      setAsk(undefined);
      const text = prompt;
      setPrompt("");
      try {
        const next = await mapChat({
          sessionId: bootstrap.session.id,
          text,
          attachments: attachments.map((attachment) => attachment.id),
          candidateRevision: candidate.currentRevision,
          mentions: activeMentions.map((chip) => chip.mention),
        });
        setCandidate(next);
        candidateRef.current = next;
        setMentions((chips) => staleMentions(chips, next));
        await refreshObjects(bootstrap.session.id);
      } catch (reason) {
        if (!turnEndedRef.current) {
          turnEndedRef.current = true;
          archiveCurrentTurn();
          logSequenceRef.current += 1;
          setConversation((entries) => [
            ...entries,
            {
              id: logSequenceRef.current,
              kind: "error",
              text: String(reason),
            },
          ]);
        }
      } finally {
        if (!turnEndedRef.current) {
          turnEndedRef.current = true;
          archiveCurrentTurn();
        }
        clearLiveDraftPreview();
        eventRevisionRef.current = "";
        markTurnInFlight(false);
      }
    },
    [
      archiveCurrentTurn,
      clearLiveDraftPreview,
      bootstrap,
      busy,
      candidate,
      markTurnInFlight,
      mentions,
      prompt,
      refreshObjects,
      resetTurn,
      turnInFlight,
    ],
  );

  const cancelTurn = useCallback(async () => {
    if (!bootstrap || !turnInFlightRef.current || busy) return;
    turnEndedRef.current = true;
    recoveredTurnRef.current = false;
    eventRevisionRef.current = "";
    clearLiveDraftPreview();
    archiveCurrentTurn();
    setAsk(undefined);
    markTurnInFlight(false);
    setBusy(true);
    try {
      await mapCancel(bootstrap.session.id);
      logSequenceRef.current += 1;
      setConversation((entries) => [
        ...entries,
        {
          id: logSequenceRef.current,
          kind: "info",
          text: "작업을 중단했습니다.",
        },
      ]);
    } catch (reason) {
      setError(`작업 중단에 실패했습니다: ${String(reason)}`);
    } finally {
      setBusy(false);
    }
  }, [
    archiveCurrentTurn,
    bootstrap,
    busy,
    clearLiveDraftPreview,
    markTurnInFlight,
  ]);

  const updateCandidate = useCallback(
    async (operation: () => Promise<CandidateStateView>) => {
      setBusy(true);
      try {
        const next = await operation();
        candidateRef.current = next;
        setCandidate(next);
        setMentions((chips) => staleMentions(chips, next));
        if (bootstrap) await refreshObjects(bootstrap.session.id);
      } catch (reason) {
        setError(String(reason));
      } finally {
        setBusy(false);
      }
    },
    [bootstrap, refreshObjects],
  );

  const mutateSourceAndReload = useCallback(
    async (operation: () => Promise<CandidateStateView>) => {
      setBusy(true);
      try {
        await operation();
        await reload();
      } catch (reason) {
        setError(String(reason));
      } finally {
        setBusy(false);
      }
    },
    [reload],
  );

  if (loading && !bootstrap) {
    return <div className="flex h-dvh items-center justify-center bg-background text-sm text-muted-foreground">Map Agent 연결 및 저장 SCX 로딩…</div>;
  }
  if (!bootstrap || !candidate) {
    return (
      <main className="flex h-dvh items-center justify-center bg-background p-6 text-foreground">
        <section className="max-w-lg rounded-xl border border-border bg-card p-6 text-center shadow-xl">
          <Layers3 className="mx-auto size-10 text-muted-foreground" aria-hidden="true" />
          <h1 className="mt-3 text-lg font-semibold">Map Agent를 열 수 없습니다</h1>
          <p className="mt-2 text-sm text-muted-foreground">{error || "현재 프로젝트의 저장된 OpenMapName을 확인할 수 없습니다."}</p>
          <div className="mt-4 flex justify-center gap-2">
            <Button type="button" onClick={() => void reload()}><RefreshCw className="size-4" />다시 시도</Button>
            <Button
              type="button"
              variant="outline"
              onClick={() => {
                void getAllWindows().then((windows) => windows.find((window) => window.label === "main")?.setFocus());
                void getCurrentWindow().close();
              }}
            >
              main 창으로
            </Button>
          </div>
        </section>
      </main>
    );
  }

  const locations = objects.flatMap((item) => (item.location ? [item.location] : []));
  const selectedMention = mentions.find((chip) => chip.id === selectedMentionId);
  const latestDiff = candidate.revisions.find(
    (revision) => revision.revision === candidate.currentRevision,
  )?.diff;
  const liveDraftVisible =
    liveDraft !== null &&
    liveDraft.candidateRevision === candidate.revisionKey &&
    view === "candidate";
  const renderedView: MapView = liveDraftVisible ? "draft" : view;
  const renderedRevisionKey = liveDraftVisible
    ? `${candidate.revisionKey}|draft:${liveDraft.requestId}:${liveDraft.generation}`
    : candidate.revisionKey;
  const renderedObjects = liveDraftVisible ? draftObjects : objects;
  const imagePreviewFresh =
    imagePlacement !== null &&
    imagePlacementPreviewIsFresh(imagePlacement, candidate.revisionKey);
  const imageCanConfirm =
    imagePreviewFresh &&
    imagePlacement.report !== undefined &&
    imagePlacement.report.protectedConflicts === 0 &&
    !imagePlacement.previewLoading &&
    !imagePlacement.confirming;
  const stampPreviewFresh =
    stampPlacement !== null &&
    stampPlacement.requestedSequence === stampPlacement.acceptedSequence;
  const stampCanConfirm =
    stampPreviewFresh &&
    stampPlacement.report !== undefined &&
    stampPlacement.report.outsideAuthorityCells === 0 &&
    stampPlacement.report.protectedCells === 0 &&
    stampPlacement.report.requiredLocationSlots <=
      stampPlacement.report.availableLocationSlots &&
    !stampPlacement.previewLoading &&
    !stampPlacement.confirming;
  const stampCollisionTotal = stampPlacement?.report
    ? stampLayerCountTotal(stampPlacement.report.collisions)
    : 0;



  return (
    <>
      <input
        ref={imageFileInputRef}
        type="file"
        accept="image/png,image/jpeg,image/webp,image/gif"
        className="sr-only"
        tabIndex={-1}
        aria-label="지형으로 배치할 사진 선택"
        onChange={(event) => {
          const file = event.target.files?.[0];
          if (file) void beginImagePlacement(file);
        }}
      />
    <MapWorkbench
      selectionAnchor={imagePlacement ? null : selectionAnchor}
      toolbar={
        <MapToolbar
          context={bootstrap.context}
          candidate={candidate}
          changedSource={changedSource}
          view={view}
          busy={
            busy ||
            turnInFlight ||
            loading ||
            sessionActionBusy ||
            imagePlacement !== null ||
            stampPlacement !== null
          }
          reloadingSource={candidate.stale && sessionActionBusy}
          imagePlacementActive={imagePlacement !== null}
          liveDraftActive={liveDraft !== null}
          onImagePlace={() => imageFileInputRef.current?.click()}
          onReloadSource={() => void createSession()}
          onView={setView}
          onRevert={(revision) =>
            void updateCandidate(() => candidateRevert(bootstrap.session.id, revision))
          }
          onDiscard={() => {
            if (!window.confirm("현재 후보 전체를 취소하고 기준 맵으로 돌아갈까요? 원본은 변경되지 않습니다.")) return;
            setBusy(true);
            void candidateDiscard(bootstrap.session.id)
              .then(() => reload())
              .finally(() => setBusy(false));
          }}
          onApply={() => {
            if (!window.confirm("검증된 mixed-layer 후보 전체를 원본 SCX에 원자적으로 Apply할까요?")) return;
            void mutateSourceAndReload(() => candidateApply(bootstrap.session.id));
          }}
          onUndo={() => {
            if (!window.confirm("마지막 Apply의 full backup bytes를 원본에 복원할까요?")) return;
            void mutateSourceAndReload(() => applyUndo(bootstrap.session.id));
          }}
        />
      }
      palette={
        <MapPalette
          key={bootstrap.session.id}
          sessionId={bootstrap.session.id}
          tileset={bootstrap.context.revision.tileset}
          locations={locations}
          selections={candidate.selections}
          onMention={handlePaletteMention}
          onStampMention={addStampMention}
          onStampPlace={beginStampPlacement}
          onLocation={handleLocationMention}
          onNewLocation={handleNewLocation}
        />
      }
      minimap={
        <MapMinimap
          sessionId={bootstrap.session.id}
          revisionKey={renderedRevisionKey}
          width={candidate.baseline.width}
          height={candidate.baseline.height}
          view={renderedView}
          requestId={liveDraftVisible ? liveDraft.requestId : undefined}
          layers={layers}
          selections={candidate.selections}
          activeRows={minimapActiveRows}
          objects={renderedObjects}
          diffRows={diffDetails.terrainRows}
          diffMarkers={diffDetails.markers}
          viewport={mapViewport}
          onNavigate={(x, y) =>
            setViewportTarget((current) => ({
              x,
              y,
              sequence: (current?.sequence ?? 0) + 1,
            }))
          }
        />
      }
      canvas={
        <MapCanvas
          sessionId={bootstrap.session.id}
          revisionKey={renderedRevisionKey}
          width={candidate.baseline.width}
          height={candidate.baseline.height}
          view={renderedView}
          requestId={liveDraftVisible ? liveDraft.requestId : undefined}
          layers={layers}
          selections={candidate.selections}
          activeCells={activeCells}
          selectionShape={selectionShape}
          selectionOperation={selectionOperation}
          interactionMode={interactionMode}
          objects={renderedObjects}
          diffRows={diffDetails.terrainRows}
          diffMarkers={diffDetails.markers}
          highlightedObjectId={highlightedObjectId}
          highlightedSelectionId={highlightedSelectionId}
          focusTarget={focusTarget}
          viewportTarget={viewportTarget}
          imagePlacement={
            imagePlacement
              ? {
                  placement: imagePlacement.placement,
                  sourceDimensions: imagePlacement.sourceDimensions,
                  bitmap:
                    imagePlacement.previewMode === "original"
                      ? imagePlacement.sourceBitmap
                      : imagePlacement.resultBitmap,
                  previewMode: imagePlacement.previewMode,
                  canConfirm: imageCanConfirm,
                }
              : undefined
          }
          stampPlacement={
            stampPlacement
              ? {
                  destination: stampPlacement.destination,
                  sourceBounds: stampPlacement.selection.bounds,
                  rows: stampPlacement.selection.rows,
                  canConfirm: stampCanConfirm,
                }
              : undefined
          }
          onActiveCells={setActiveCells}
          onCursor={setCursor}
          onObjectSelect={(object) => {
            setSelectedObject(object);
            setHighlightedObjectId(object?.id);
          }}
          onZoom={setZoom}
          onSelectionAnchor={setSelectionAnchor}
          onViewportChange={setMapViewport}
          onImagePlacement={updateImagePlacement}
          onImageConfirm={() => void confirmImagePlacement()}
          onImageCancel={() => clearImagePlacement(true)}
          onStampPlacement={updateStampPlacement}
          onStampConfirm={() => {
            if (stampCanConfirm && stampCollisionTotal === 0) {
              void confirmStampPlacement("merge");
            }
          }}
          onStampCancel={clearStampPlacement}
        />
      }
      selectionToolbar={
        <div className="space-y-2">
          {error && (
            <div role="alert" className="mx-auto flex max-w-xl items-center gap-2 rounded-lg border border-destructive/50 bg-destructive/90 px-3 py-2 text-xs text-white shadow-xl">
              <span className="min-w-0 flex-1">{error}</span>
              <Button type="button" size="icon" variant="ghost" className="min-h-11 min-w-11" aria-label="오류 닫기" onClick={() => setError("")}>
                <X className="size-4" aria-hidden="true" />
              </Button>
            </div>
          )}
          {imagePlacement ? (
            <ImagePlacementControls
              fileName={imagePlacement.attachment.name}
              sourceDimensions={imagePlacement.sourceDimensions}
              placement={imagePlacement.placement}
              mapWidth={candidate.baseline.width}
              mapHeight={candidate.baseline.height}
              previewMode={imagePlacement.previewMode}
              report={imagePlacement.report}
              previewFresh={imagePreviewFresh}
              previewLoading={imagePlacement.previewLoading}
              confirming={imagePlacement.confirming}
              error={imagePlacement.error}
              onPlacement={updateImagePlacement}
              onPreviewMode={(previewMode) => {
                const current = imagePlacementRef.current;
                if (!current) return;
                const next = { ...current, previewMode };
                imagePlacementRef.current = next;
                setImagePlacement(next);
              }}
              onConfirm={() => void confirmImagePlacement()}
              onCancel={() => clearImagePlacement(true)}
            />
          ) : stampPlacement ? (
            <StampPlacementControls
              selection={stampPlacement.selection}
              destination={stampPlacement.destination}
              mapWidth={candidate.baseline.width}
              mapHeight={candidate.baseline.height}
              report={stampPlacement.report}
              previewFresh={stampPreviewFresh}
              previewLoading={stampPlacement.previewLoading}
              confirming={stampPlacement.confirming}
              error={stampPlacement.error}
              onDestination={(destination) => updateStampPlacement(destination, true)}
              onConfirm={(policy) => void confirmStampPlacement(policy)}
              onCancel={clearStampPlacement}
            />
          ) : (
            <>
          <SelectionToolbar
            activeCells={activeCells}
            shape={selectionShape}
            operation={selectionOperation}
            role={selectionRole}
            allowedLayers={selectionLayers}
            label={selectionLabel}
            interactionMode={interactionMode}
            savedSelections={candidate.selections}
            onShape={setSelectionShape}
            onOperation={(operation) => {
              setSelectionOperation(operation);
              if (operation === "clear") setActiveCells(new Set());
            }}
            onRole={setSelectionRole}
            onLayers={setSelectionLayers}
            onLabel={setSelectionLabel}
            onInteractionMode={setInteractionMode}
            onCells={setActiveCells}
            onSave={() => void saveActiveSelection().then(() => setActiveCells(new Set())).catch((reason) => setError(String(reason)))}
            onMention={() => void saveActiveSelection().then((selection) => {
              addRegionMention(selection);
              setActiveCells(new Set());
            }).catch((reason) => setError(String(reason)))}
            onClear={() => setActiveCells(new Set())}
            onLoadSelection={(selection) => {
              setActiveCells(rowsToCells(selection.rows));
              setSelectionLabel(selection.label);
              setSelectionRole(selection.role);
              setSelectionLayers(selection.layers);
            }}
            onDeleteSelection={(selection) => {
              if (stampPlacementRef.current?.selection.id === selection.id) {
                clearStampPlacement();
              }
              void deleteSelection(bootstrap.session.id, selection.id)
                .then((next) => {
                  setCandidate(next);
                  setMentions((chips) => staleMentions(chips, next));
                })
                .catch((reason) => setError(String(reason)));
            }}
          />
          {selectedObject && interactionMode === "inspect" && (
            <div className="mx-auto flex w-fit items-center gap-2 rounded-lg border border-border bg-card/95 p-2 shadow-xl">
              <Box className="size-4" aria-hidden="true" />
              <span className="text-xs">{selectedObject.kind} · 구조화 좌표 hit</span>
              <Button
                type="button"
                size="sm"
                disabled={liveDraftVisible}
                title={
                  liveDraftVisible
                    ? "수정 중 미리보기 개체는 후보 확정 후 멘션할 수 있습니다."
                    : undefined
                }
                onClick={addSelectedObjectMention}
              >
                프롬프트에 추가
              </Button>
              <Button type="button" size="icon" variant="ghost" className="min-h-11 min-w-11" aria-label="개체 선택 해제" onClick={() => {
                setSelectedObject(null);
                setHighlightedObjectId(undefined);
              }}>
                <X className="size-4" />
              </Button>
            </div>
          )}
            </>
          )}
        </div>
      }
      agent={
        <MapAgentPanel
          sessionName={bootstrap.session.name}
          conversation={conversation}
          turn={turn}
          live={turnInFlight}
          actionBusy={busy || imagePlacement !== null || stampPlacement !== null}
          contextUsage={contextUsage}
          codexSettings={codexSettings}
          codexSettingsBusy={codexSettingsBusy}
          text={prompt}
          mentions={mentions}
          selectedMentionId={selectedMentionId}
          ask={ask}
          selections={candidate.selections}
          mapWidth={candidate.baseline.width}
          mapHeight={candidate.baseline.height}
          onText={setPrompt}
          onSend={(attachments) => void send(attachments)}
          onCancel={() => void cancelTurn()}
          onStageAttachment={stageAttachment}
          onDiscardAttachment={discardAttachment}
          onCodexSettingsChange={(model, reasoningEffort) => {
            void handleCodexSettingsChange(model, reasoningEffort);
          }}
          onCodexSettingsReload={() => void loadCodexModelSettings()}
          onMentionSelect={setSelectedMentionId}
          onMentionRemove={(id) => {
            setMentions((chips) => chips.filter((chip) => chip.id !== id));
            if (selectedMentionId === id) setSelectedMentionId(undefined);
          }}
          onMentionFind={(id) => {
            const chip = mentions.find((item) => item.id === id);
            if (!chip) return;
            if (chip.mention.kind === "region" || chip.mention.kind === "stamp") {
              const selectionId = chip.mention.selectionId;
              const selection = candidate.selections.find(
                (item) => item.id === selectionId,
              );
              if (!selection) return;
              setActiveCells(rowsToCells(selection.rows));
              setHighlightedSelectionId(selection.id);
              setFocusTarget((current) => ({
                bounds: selection.bounds,
                sequence: (current?.sequence ?? 0) + 1,
              }));
            } else if (chip.mention.kind === "object") {
              const objectId =
                `${chip.mention.objectRef.kind}:${chip.mention.objectRef.ordinal}:${chip.mention.objectRef.semanticFingerprint}`;
              setHighlightedObjectId(objectId);
              setFocusTarget((current) => ({
                objectId,
                sequence: (current?.sequence ?? 0) + 1,
              }));
            } else if (chip.mention.kind === "location") {
              const objectId = `location:${chip.mention.locationId}`;
              setHighlightedObjectId(objectId);
              setFocusTarget((current) => ({
                objectId,
                sequence: (current?.sequence ?? 0) + 1,
              }));
            }
          }}
          onMentionHighlight={(id) => {
            const chip = mentions.find((item) => item.id === id);
            setHighlightedSelectionId(
              chip?.mention.kind === "region" || chip?.mention.kind === "stamp"
                ? chip.mention.selectionId
                : undefined,
            );
            if (chip?.mention.kind === "object") {
              setHighlightedObjectId(
                `${chip.mention.objectRef.kind}:${chip.mention.objectRef.ordinal}:${chip.mention.objectRef.semanticFingerprint}`,
              );
            } else if (chip?.mention.kind === "location") {
              setHighlightedObjectId(`location:${chip.mention.locationId}`);
            } else {
              setHighlightedObjectId(undefined);
            }
          }}
          onQualifierChange={(qualifiers: MentionQualifiers) => {
            if (!selectedMention || selectedMention.mention.kind !== "palette") return;
            setMentions((chips) =>
              chips.map((chip) =>
                chip.id === selectedMention.id && chip.mention.kind === "palette"
                  ? { ...chip, mention: { ...chip.mention, qualifiers } }
                  : chip,
              ),
            );
          }}
          onAskSubmit={(requestId, answers: Record<string, AskAnswer>) => {
            setAsk((current) => current ? { ...current, submitting: true } : current);
            void invoke("ask_response", {
              sessionId: bootstrap.session.id,
              requestId,
              answers,
            })
              .then(() => setAsk(undefined))
              .catch((reason) => {
                setError(String(reason));
                setAsk((current) => current ? { ...current, submitting: false } : current);
              });
          }}
          onHistory={() => {
            setSessionHistoryOpen(true);
            void refreshSessionHistory();
          }}
        />
      }
      status={
        <div className="flex min-w-0 flex-1 items-center gap-3 overflow-hidden">
          <span className="shrink-0 font-mono text-[11px] text-cyan-300">
            {cursor ? `tile ${cursor.x},${cursor.y} · px ${cursor.x * 32},${cursor.y * 32}` : "cursor —"}
          </span>
          <span className="shrink-0 text-[11px] text-muted-foreground">zoom {(zoom * 100).toFixed(0)}%</span>
          <span className="shrink-0 text-[11px] text-muted-foreground">선택 {activeCells.size.toLocaleString()} · 개체 {objects.length.toLocaleString()}</span>
          <div className="flex shrink-0 gap-1" aria-label="표시 레이어">
            {allLayers.map((layer) => (
              <button
                key={layer}
                type="button"
                className={`rounded px-1.5 py-0.5 text-[10px] ${layers.includes(layer) ? "bg-primary/15 text-primary" : "text-muted-foreground"}`}
                aria-pressed={layers.includes(layer)}
                onClick={() => setLayers((visible) => visible.includes(layer) ? visible.filter((item) => item !== layer) : [...visible, layer])}
              >
                {layer}
              </button>
            ))}
          </div>
          <div className="min-w-0 flex-1 overflow-hidden">
            <CandidateControls candidate={candidate} details={diffDetails} />
          </div>
          {latestDiff && (latestDiff.outsideTarget > 0 || latestDiff.protected > 0) && (
            <span className="shrink-0 text-[11px] text-destructive">authority 위반</span>
          )}
          <LocateFixed className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
        </div>
      }
      />
      <MapSessionHistoryDialog
        open={sessionHistoryOpen}
        sessions={mapSessions}
        activeId={bootstrap.session.id}
        loading={sessionHistoryLoading}
        busy={busy || turnInFlight || sessionActionBusy}
        onOpenChange={(open) => {
          setSessionHistoryOpen(open);
          if (open) void refreshSessionHistory();
        }}
        onReload={() => void refreshSessionHistory()}
        onCreate={() => void createSession()}
        onLoad={(sessionId) => void loadSession(sessionId)}
        onRename={(sessionId, name) => void renameSession(sessionId, name)}
        onDelete={(sessionId) => void deleteSession(sessionId)}
      />
    </>
  );
}
