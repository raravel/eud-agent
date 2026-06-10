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
import { SparklesIcon, WrenchIcon } from "lucide-react";
import {
  Conversation,
  ConversationContent,
  ConversationScrollButton,
} from "@/components/ai-elements/conversation";
import { Message, MessageContent } from "@/components/ai-elements/message";
import { Response } from "@/components/ai-elements/response";
import { Shimmer } from "@/components/ai-elements/shimmer";
import { Spinner } from "@/components/ui/spinner";
import { cn } from "@/lib/utils";
import { AgentStream, ToolList } from "@/components/AgentStream";
import { AgentAnswer } from "@/components/AgentAnswer";
import type { LogEntry, LogKind, Phase, TurnState } from "@/state/store";

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

export function ConversationLog({
  log,
  phase,
  turn,
  ragLoading,
  onSuggestion,
  suggestionsEnabled = true,
}: ConversationLogProps) {
  const busy = BUSY_PHASES.has(phase);
  // Waiting shimmer: between chat send (phase → thinking, fresh empty turn) and
  // the FIRST streamed agent_event nothing renders, so the user cannot tell the
  // input was received. While thinking with an EMPTY turn buffer, show a
  // Shimmer label inline; the first reasoning/tool/answer content replaces it.
  const waiting =
    turn !== undefined &&
    phase === "thinking" &&
    turn.reasoning.length === 0 &&
    !turn.answerStarted &&
    turn.tools.length === 0;
  // The active spinner target: the LATEST progress entry, but only while busy.
  let activeProgressId: number | null = null;
  if (busy) {
    for (const entry of log) {
      if (entry.kind === "progress" && entry.stage) {
        activeProgressId = entry.id;
      }
    }
  }

  // Empty-conversation hero (UX: empty states carry guidance + an action):
  // only while idle-ready with nothing logged — any log entry, warmup shimmer,
  // or in-flight turn replaces it.
  const empty = log.length === 0 && phase === "ready" && !ragLoading;

  return (
    <Conversation className="flex-1">
      <ConversationContent className="gap-2 p-4">
        {empty && (
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
              <p className="text-base font-semibold">무엇을 만들까요?</p>
              <p className="text-sm text-muted-foreground">
                자연어로 지시하면 epScript 코드를 만들어 에디터에 적용합니다.
              </p>
            </div>
            {onSuggestion && (
              <ul className="flex w-full max-w-sm flex-col gap-2">
                {SUGGESTIONS.map((text) => (
                  <li key={text}>
                    <button
                      type="button"
                      disabled={!suggestionsEnabled}
                      onClick={() => onSuggestion(text)}
                      className="w-full cursor-pointer rounded-lg border border-border bg-card/60 px-3 py-2 text-left text-sm text-muted-foreground transition-colors duration-200 hover:border-emerald-500/40 hover:bg-emerald-500/10 hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-default disabled:opacity-50"
                    >
                      {text}
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </div>
        )}

        {log.map((entry) => {
          // Agent answers — PROMINENT foreground Message/Response (Streamdown).
          if (entry.kind === "agent") {
            return (
              <Message key={entry.id} from="assistant" className="text-foreground">
                <MessageContent>
                  <Response>{entry.text}</Response>
                </MessageContent>
              </Message>
            );
          }
          // User instructions — secondary Message bubble.
          if (entry.kind === "you") {
            return (
              <Message key={entry.id} from="user">
                <MessageContent>{entry.text}</MessageContent>
              </Message>
            );
          }
          // Archived tool entry (EUD-069): summary line + expandable Tool cards.
          if (entry.tools !== undefined) {
            return (
              <div key={entry.id} className="flex w-full flex-col gap-1">
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
          // System / progress / info rows — muted simple rows.
          const isActive = entry.id === activeProgressId;
          const testId = entry.stage ? `log-entry-${entry.stage}` : undefined;
          return (
            <div
              key={entry.id}
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
        })}

        {/* RAG warmup shimmer — while the model loads the send gate is closed;
            this row explains the locked input. */}
        {ragLoading && (
          <div
            data-testid="rag-waiting"
            role="status"
            className="flex w-fit max-w-[95%] items-center text-sm"
          >
            <Shimmer>RAG 모델 준비 중…</Shimmer>
          </div>
        )}

        {/* Waiting shimmer — feedback that the input was received, before the
            first stream event arrives. */}
        {waiting && (
          <div
            data-testid="turn-waiting"
            role="status"
            className="flex w-fit max-w-[95%] items-center text-sm"
          >
            <Shimmer>생각하는 중…</Shimmer>
          </div>
        )}

        {/* Live agent activity for the current turn — INLINE in the scroll area
            (EUD-069): the reasoning block first, then the turn's activity
            blocks IN ARRIVAL ORDER (tool groups and prose bubbles interleaved
            chronologically — not all tools above all prose). Turns from older
            fixtures without blocks fall back to the legacy tools+answer pair. */}
        {turn && (
          <AgentStream
            reasoning={turn.reasoning}
            answerStarted={turn.answerStarted}
            tools={turn.blocks.length > 0 ? [] : turn.tools}
            live={phase === "thinking"}
          />
        )}
        {turn &&
          phase === "thinking" &&
          turn.blocks.map((block) =>
            block.type === "tools" ? (
              <div
                key={`turn-block-${block.id}`}
                className="flex w-full max-w-[95%] flex-col gap-1"
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
        {turn && phase === "thinking" && turn.blocks.length === 0 && (
          <AgentAnswer text={turn.answer} />
        )}
      </ConversationContent>
      <ConversationScrollButton />
    </Conversation>
  );
}
