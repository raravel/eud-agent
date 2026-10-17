/**
 * The Map conversation as data: the persisted panel log of a Map session,
 * one live turn folded from its `agent_event`s, and the archived entries a
 * finished turn leaves. The desktop Map window and the phone's Map screen
 * share these, so a run reads the same on both.
 */
import type {
  AgentEventData,
  AgentTool,
  LogKind,
  TurnState,
} from "@/state/store";
import type { MapConversationEntry } from "./MapAgentPanel";
import type { MapBootstrapResponse, MapMentionSnapshot } from "./mapProtocol";

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

export function conversationFromSession(
  bootstrap: MapBootstrapResponse,
): MapConversationEntry[] {
  const log = bootstrap.session.panelLog?.log ?? [];
  return log.flatMap((entry) => {
    if (!isMapLogKind(entry.kind)) return [];
    const persisted = entry as typeof entry & {
      mapMentions?: MapMentionSnapshot[];
      requestId?: string;
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
        ...(typeof persisted.requestId === "string"
          ? { requestId: persisted.requestId }
          : {}),
        attachments: entry.attachments,
        ...(tools && tools.length > 0 ? { tools } : {}),
      },
    ];
  });
}

export function panelLogFromConversation(
  conversation: MapConversationEntry[],
  logSeq: number,
) {
  return {
    schemaVersion: 2,
    logSeq,
    log: conversation.map(({ mapMentions, requestId, ...entry }) => ({
      ...entry,
      mapMentions,
      ...(requestId === undefined ? {} : { requestId }),
    })),
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
): {
  turn: TurnState;
  cursor: MapTurnCursor;
  unpairedToolStart?: { name: string; args?: string };
} {
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
    const callId =
      data.callId !== undefined && data.callId.trim().length > 0
        ? data.callId
        : undefined;
    if (callId === undefined) {
      return {
        turn,
        cursor: nextCursor,
        unpairedToolStart: {
          name: detail || "tool",
          ...(data.args ? { args: data.args } : {}),
        },
      };
    }
    if (turn.tools.some((tool) => tool.callId === callId)) {
      return { turn, cursor: nextCursor };
    }
    nextCursor.toolSequence += 1;
    const tool: AgentTool = {
      id: `map-tool-${nextCursor.toolSequence}`,
      callId,
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
    const callId =
      data.callId !== undefined && data.callId.trim().length > 0
        ? data.callId
        : undefined;
    const complete = (tool: AgentTool): AgentTool => ({
      ...tool,
      state: failed ? "failed" : "done",
      ...(data.result ? { detail: data.result } : {}),
    });
    const matchingIndex =
      callId === undefined
        ? -1
        : turn.tools.findIndex(
            (tool) =>
              tool.callId === callId && tool.state === "running",
          );
    if (
      matchingIndex < 0 &&
      callId !== undefined &&
      turn.tools.some((tool) => tool.callId === callId)
    ) {
      return { turn, cursor: nextCursor };
    }
    if (matchingIndex < 0) {
      nextCursor.toolSequence += 1;
      const terminal: AgentTool = {
        id: `map-tool-${nextCursor.toolSequence}`,
        ...(callId !== undefined ? { callId } : {}),
        name: detail || "tool",
        state: failed ? "failed" : "done",
        ...(data.result ? { detail: data.result } : {}),
      };
      const blocks = turn.blocks.slice();
      const last = blocks[blocks.length - 1];
      if (last !== undefined && last.type === "tools") {
        blocks[blocks.length - 1] = {
          ...last,
          tools: [...last.tools, terminal],
        };
      } else {
        nextCursor.blockSequence += 1;
        blocks.push({
          id: nextCursor.blockSequence,
          type: "tools",
          tools: [terminal],
        });
      }
      return {
        turn: { ...turn, tools: [...turn.tools, terminal], blocks },
        cursor: nextCursor,
      };
    }
    const tools = turn.tools.slice();
    const matchingTool = tools[matchingIndex];
    tools[matchingIndex] = complete(matchingTool);
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
        if (block.tools[toolIndex].id !== matchingTool.id) continue;
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

/**
 * Whether steer rows parked while a tool ran may be placed now: always when
 * the request is ending (`force`: answer, error, failure, cancel), otherwise
 * once no tool of the turn still runs. Shared by the Map window and the
 * phone's Map screen so both place them on every ending path.
 */
export function steerRowsPlaceable(turn: TurnState, parked: number, force: boolean): boolean {
  if (parked === 0) return false;
  return force || !turn.tools.some((tool) => tool.state === "running");
}
