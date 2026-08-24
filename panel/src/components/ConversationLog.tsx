/**
 * Conversation / event log (features/06 ## UI layout + Behaviors), rebuilt on the
 * vendored AI Elements (Conversation + Message) + Streamdown (decision 06):
 *   - user instructions render as Message bubbles (secondary);
 *   - agent answers render as PROMINENT (foreground) Message/Response bubbles via
 *     Streamdown — the most visible text in the log (answer prominence inverts the
 *     original v2 styling);
 *   - system/progress/info/ok/warn/error rows stay muted simple rows; the LATEST
 *     progress entry spins while the panel is busy (incl. waiting_build);
 *   - EUD-069: the LIVE agent stream (reasoning block + tool rows + streamed
 *     answer bubble) renders INLINE at the end of this scroll area — never as a
 *     fixed band between the log and the input (an unbounded band squeezed the
 *     log to 0px and the plan card to 33px in the live E2E). Archived tool
 *     entries (LogEntry.tools) render their Tool cards back, expandable.
 *
 * The Conversation container provides auto-scroll-to-bottom (use-stick-to-bottom)
 * so streamed answers keep the latest content in view. The store caps the log at
 * 500 entries.
 */
import {
  AudioLinesIcon,
  FileTextIcon,
  ImageIcon,
  PencilLineIcon,
  SparklesIcon,
  WrenchIcon,
} from "lucide-react";
import { useMemo, type ReactNode } from "react";
import {
  measureElement as measureVirtualElement,
  observeElementRect as observeVirtualElementRect,
  useVirtualizer,
} from "@tanstack/react-virtual";
import { useStickToBottomContext } from "use-stick-to-bottom";
import {
  Conversation,
  ConversationContent,
  ConversationScrollButton,
} from "@/components/ai-elements/conversation";
import { Message, MessageContent } from "@/components/ai-elements/message";
import { DiagramResponse } from "@/components/ai-elements/response";
import { Shimmer } from "@/components/ai-elements/shimmer";
import { Spinner } from "@/components/ui/spinner";
import { cn } from "@/lib/utils";
import { AgentStream, ToolList } from "@/components/AgentStream";
import { AgentAnswer } from "@/components/AgentAnswer";
import type { LogEntry, LogKind, Phase, TurnState } from "@/state/store";
import { formatAttachmentSize } from "@/lib/attachments";
import { MentionChips } from "@/components/MentionComposer";

export interface ConversationLogProps {
  /** Store log entries (kind / text / optional stage). */
  log: LogEntry[];
  /** Panel phase — decides whether the latest progress entry is "active". */
  phase: Phase;
  /**
   * Per-turn live buffers (EUD-069): when present, the reasoning/tool surfaces
   * and the live answer bubble render INLINE at the end of the conversation.
   */
  turn?: TurnState;
  /**
   * RAG model warmup in progress (store.rag === "loading"): sending is blocked
   * by the store gate, and a Shimmer "RAG 모델 준비 중…" row explains why.
   */
  ragLoading?: boolean;
  /**
   * Empty-conversation suggestion chips: clicking one sends the example text
   * as a chat (App wires this to the same handler as the InstructionBox).
   * Omitted → the empty state renders without chips.
   */
  onSuggestion?: (text: string) => void;
  /** Send gating for the suggestion chips (store.canSend). */
  suggestionsEnabled?: boolean;
  /** Rewind from a user message and restore it into the prompt for editing. */
  onEditMessage?: (entry: LogEntry) => void;
  /** Disable edit actions while cancel/rewind is settling in the core. */
  editDisabled?: boolean;
  /** Product-specific copy while retaining the shared empty-state layout. */
  emptyTitle?: string;
  emptyDescription?: string;
  /** Optional structured metadata rendered inside a user message bubble. */
  renderUserMeta?: (entry: LogEntry) => ReactNode;
  /** Session-scoped interactive card appended after the live turn. */
  tail?: ReactNode;
}

/** Example instructions shown in the empty conversation (click → send). */
const SUGGESTIONS: readonly string[] = [
  "게임 시작 시 모든 플레이어에게 미네랄 1000 지급",
  "마린의 HP를 2배로 올려줘",
  "현재 프로젝트의 트리거 구조를 설명해줘",
];

/** Phases in which a live progress entry should still spin (v2: a turn in flight). */
const BUSY_PHASES: ReadonlySet<Phase> = new Set<Phase>([
  "thinking",
  "plan_review",
]);

/** Per-kind text styling for a muted (non-bubble) log row. */
const MUTED_KIND_CLASS: Record<Exclude<LogKind, "you" | "agent">, string> = {
  info: "text-muted-foreground",
  progress: "text-muted-foreground",
  ok: "text-emerald-400",
  warn: "text-amber-400",
  error: "text-destructive",
};

type ConversationRow =
  | { key: string; type: "log"; entry: LogEntry }
  | { key: "empty"; type: "empty" }
  | { key: "rag"; type: "rag" }
  | { key: "live"; type: "live" }
  | { key: "tail"; type: "tail"; node: ReactNode };

interface RowRenderContext {
  phase: Phase;
  turn?: TurnState;
  activeProgressId: number | null;
  onSuggestion?: (text: string) => void;
  suggestionsEnabled: boolean;
  onEditMessage?: (entry: LogEntry) => void;
  editDisabled: boolean;
  emptyTitle: string;
  emptyDescription: string;
  renderUserMeta?: (entry: LogEntry) => ReactNode;
}

export function ConversationLog({
  log,
  phase,
  turn,
  ragLoading,
  onSuggestion,
  suggestionsEnabled = true,
  onEditMessage,
  editDisabled = false,
  emptyTitle = "무엇을 만들까요?",
  emptyDescription = "자연어로 지시하면 epScript 코드를 만들어 에디터에 적용합니다.",
  renderUserMeta,
  tail,
}: ConversationLogProps) {
  const busy = BUSY_PHASES.has(phase);
  const activeProgressId = useMemo(() => {
    if (!busy) return null;
    let latest: number | null = null;
    for (const entry of log) {
      if (entry.kind === "progress" && entry.stage) latest = entry.id;
    }
    return latest;
  }, [busy, log]);

  const empty = log.length === 0 && phase === "ready" && !ragLoading;
  const hasLiveTurn =
    turn !== undefined &&
    (turn.reasoning.length > 0 ||
      turn.answerStarted ||
      turn.tools.length > 0 ||
      turn.blocks.length > 0);
  const rows = useMemo<ConversationRow[]>(() => {
    const next: ConversationRow[] = log.map((entry) => ({
      key: `log-${entry.id}`,
      type: "log",
      entry,
    }));
    if (empty) next.push({ key: "empty", type: "empty" });
    if (ragLoading) next.push({ key: "rag", type: "rag" });
    if (hasLiveTurn) next.push({ key: "live", type: "live" });
    if (tail !== undefined && tail !== null) {
      next.push({ key: "tail", type: "tail", node: tail });
    }
    return next;
  }, [empty, hasLiveTurn, log, ragLoading, tail]);

  return (
    <Conversation className="flex-1">
      <ConversationContent className="block p-4">
        <VirtualizedConversationRows
          rows={rows}
          context={{
            phase,
            turn,
            activeProgressId,
            onSuggestion,
            suggestionsEnabled,
            onEditMessage,
            editDisabled,
            emptyTitle,
            emptyDescription,
            renderUserMeta,
          }}
        />
      </ConversationContent>
      <ConversationScrollButton />
    </Conversation>
  );
}

function VirtualizedConversationRows({
  rows,
  context,
}: {
  rows: ConversationRow[];
  context: RowRenderContext;
}) {
  const { scrollRef } = useStickToBottomContext();
  const virtualizer = useVirtualizer<HTMLElement, HTMLDivElement>({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    getItemKey: (index) => rows[index]?.key ?? index,
    estimateSize: (index) => estimateRowSize(rows[index]),
    overscan: 8,
    initialRect: { width: 800, height: 600 },
    observeElementRect: (instance, callback) =>
      observeVirtualElementRect(instance, (rect) =>
        callback({
          width: rect.width || 800,
          height: rect.height || 600,
        }),
      ),
    measureElement: (element, entry, instance) => {
      const measured = measureVirtualElement(element, entry, instance);
      const index = Number(element.dataset.index);
      return measured > 0 ? measured : estimateRowSize(rows[index]);
    },
  });

  return (
    <div
      data-testid="virtualized-conversation"
      className="relative w-full"
      style={{ height: virtualizer.getTotalSize() }}
    >
      {virtualizer.getVirtualItems().map((item) => {
        const row = rows[item.index];
        if (row === undefined) return null;
        return (
          <div
            key={item.key}
            ref={virtualizer.measureElement}
            data-index={item.index}
            data-virtual-row
            className="absolute left-0 top-0 w-full pb-2"
            style={{ transform: `translateY(${item.start}px)` }}
          >
            {renderRow(row, context)}
          </div>
        );
      })}
    </div>
  );
}

function estimateRowSize(row: ConversationRow | undefined): number {
  if (row === undefined) return 72;
  if (row.type === "empty") return 300;
  if (row.type === "rag") return 32;
  if (row.type === "live") return 160;
  if (row.type === "tail") return 280;
  if (row.entry.kind === "agent") return 140;
  if (row.entry.kind === "you") return 120;
  if (row.entry.tools !== undefined) return 96;
  return 40;
}

function renderRow(row: ConversationRow, context: RowRenderContext) {
  switch (row.type) {
    case "empty":
      return (
        <div
          data-testid="conversation-empty"
          className="flex flex-col items-center gap-5 px-4 py-14 text-center animate-in fade-in duration-300 motion-reduce:animate-none"
        >
          <span
            aria-hidden
            className="flex size-12 items-center justify-center rounded-2xl border border-emerald-500/30 bg-emerald-500/15 text-emerald-400"
          >
            <SparklesIcon className="size-6" />
          </span>
          <div className="grid gap-1">
            <p className="text-base font-semibold">{context.emptyTitle}</p>
            <p className="text-sm text-muted-foreground">
              {context.emptyDescription}
            </p>
          </div>
          {context.onSuggestion && (
            <ul className="flex w-full max-w-sm flex-col gap-2">
              {SUGGESTIONS.map((text) => (
                <li key={text}>
                  <button
                    type="button"
                    disabled={!context.suggestionsEnabled}
                    onClick={() => context.onSuggestion?.(text)}
                    className="w-full cursor-pointer rounded-lg border border-border bg-card/60 px-3 py-2 text-left text-sm text-muted-foreground transition-colors duration-200 hover:border-emerald-500/40 hover:bg-emerald-500/10 hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-default disabled:opacity-50"
                  >
                    {text}
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      );
    case "rag":
      return (
        <div
          data-testid="rag-waiting"
          role="status"
          className="flex w-fit max-w-[95%] items-center text-sm"
        >
          <Shimmer>RAG 모델 준비 중…</Shimmer>
        </div>
      );
    case "live":
      return renderLiveTurn(context.turn, context.phase);
    case "tail":
      return row.node;
    case "log":
      return renderLogEntry(row.entry, context);
  }
}

function renderLogEntry(entry: LogEntry, context: RowRenderContext) {
  if (entry.kind === "agent") {
    return (
      <Message from="assistant" className="text-foreground">
        <MessageContent>
          <DiagramResponse mode="static">{entry.text}</DiagramResponse>
        </MessageContent>
      </Message>
    );
  }

  if (entry.kind === "you") {
    return (
      <Message from="user">
        <MessageContent>
          {entry.mentions && entry.mentions.length > 0 && (
            <div className="mb-2 flex max-w-md justify-end">
              <MentionChips mentions={entry.mentions} align="end" />
            </div>
          )}
          {entry.attachments && entry.attachments.length > 0 && (
            <div className="mb-2 flex max-w-md flex-wrap justify-end gap-2">
              {entry.attachments.map((attachment) => {
                const preview =
                  attachment.previewUrl?.startsWith("data:image/") === true
                    ? attachment.previewUrl
                    : null;
                return preview !== null ? (
                  <figure
                    key={attachment.id}
                    className="overflow-hidden rounded-lg border border-border/70 bg-background/40"
                  >
                    <img
                      src={preview}
                      alt={`첨부 이미지: ${attachment.name}`}
                      className="max-h-40 max-w-56 object-contain"
                    />
                    <figcaption className="flex items-center gap-1.5 px-2 py-1 text-[11px] text-muted-foreground">
                      <ImageIcon className="size-3" />
                      <span className="max-w-40 truncate">{attachment.name}</span>
                      <span>{formatAttachmentSize(attachment.size)}</span>
                    </figcaption>
                  </figure>
                ) : (
                  <div
                    key={attachment.id}
                    className="flex max-w-56 items-center gap-2 rounded-lg border border-border/70 bg-background/40 px-2.5 py-2"
                    title={attachment.name}
                  >
                    {attachment.kind === "image" ? (
                      <ImageIcon className="size-4 shrink-0 text-muted-foreground" />
                    ) : attachment.kind === "audio" ? (
                      <AudioLinesIcon className="size-4 shrink-0 text-muted-foreground" />
                    ) : (
                      <FileTextIcon className="size-4 shrink-0 text-muted-foreground" />
                    )}
                    <span className="min-w-0">
                      <span className="block truncate text-xs">{attachment.name}</span>
                      <span className="block text-[11px] text-muted-foreground">
                        {formatAttachmentSize(attachment.size)}
                      </span>
                    </span>
                  </div>
                );
              })}
            </div>
          )}
          {entry.text.length > 0 && (
            <span className="whitespace-pre-wrap">{entry.text}</span>
          )}
          {context.renderUserMeta?.(entry)}
        </MessageContent>
        {context.onEditMessage && (
          <button
            type="button"
            aria-label="메시지 수정"
            disabled={context.editDisabled}
            onClick={() => context.onEditMessage?.(entry)}
            className="ml-auto flex min-h-11 cursor-pointer items-center gap-1.5 rounded-md px-3 text-xs text-muted-foreground transition-colors duration-200 hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-default disabled:opacity-50"
          >
            <PencilLineIcon aria-hidden className="size-3.5" />
            수정
          </button>
        )}
      </Message>
    );
  }

  if (entry.tools !== undefined) {
    return (
      <div className="my-3 flex w-full flex-col gap-1">
        <span
          className={cn(
            "flex items-center gap-1.5 text-sm",
            MUTED_KIND_CLASS[entry.kind],
          )}
        >
          <WrenchIcon aria-hidden className="size-3.5 shrink-0" />
          {entry.text}
        </span>
        <ToolList tools={entry.tools} />
      </div>
    );
  }

  const isActive = entry.id === context.activeProgressId;
  const testId = entry.stage ? `log-entry-${entry.stage}` : undefined;
  return (
    <div
      data-testid={testId}
      className={cn(
        "flex w-fit max-w-[95%] items-center gap-2 text-sm",
        MUTED_KIND_CLASS[entry.kind],
      )}
    >
      {isActive && <Spinner className="size-3.5 shrink-0" />}
      <span className="whitespace-pre-wrap break-words">{entry.text}</span>
    </div>
  );
}

function renderLiveTurn(turn: TurnState | undefined, phase: Phase) {
  if (turn === undefined) return null;
  return (
    <>
      <AgentStream
        reasoning={turn.reasoning}
        answerStarted={turn.answerStarted}
        tools={turn.blocks.length > 0 ? [] : turn.tools}
        live={phase === "thinking"}
      />
      {phase === "thinking" &&
        turn.blocks.map((block) =>
          block.type === "tools" ? (
            <div
              key={`turn-block-${block.id}`}
              className="my-3 flex w-full max-w-[95%] flex-col gap-1"
            >
              <span className="flex items-center gap-1.5 text-xs text-muted-foreground">
                <WrenchIcon aria-hidden className="size-3.5 shrink-0" />
                도구 호출 {block.tools.length}건
              </span>
              <ToolList tools={block.tools} />
            </div>
          ) : block.text.trim().length > 0 ? (
            <AgentAnswer key={`turn-block-${block.id}`} text={block.text} />
          ) : null,
        )}
      {phase === "thinking" && turn.blocks.length === 0 && (
        <AgentAnswer text={turn.answer} />
      )}
    </>
  );
}
