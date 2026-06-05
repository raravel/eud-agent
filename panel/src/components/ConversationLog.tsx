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
  Conversation,
  ConversationContent,
  ConversationScrollButton,
} from "@/components/ai-elements/conversation";
import { Message, MessageContent } from "@/components/ai-elements/message";
import { Response } from "@/components/ai-elements/response";
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
}

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

export function ConversationLog({ log, phase, turn }: ConversationLogProps) {
  const busy = BUSY_PHASES.has(phase);
  // The active spinner target: the LATEST progress entry, but only while busy.
  let activeProgressId: number | null = null;
  if (busy) {
    for (const entry of log) {
      if (entry.kind === "progress" && entry.stage) {
        activeProgressId = entry.id;
      }
    }
  }

  return (
    <Conversation className="flex-1">
      <ConversationContent className="gap-2 p-4">
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
                  className={cn("text-sm", MUTED_KIND_CLASS[entry.kind])}
                >
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

        {/* Live agent activity for the current turn — INLINE in the scroll area
            (EUD-069): the reasoning block + running tool rows, then the live
            streamed answer bubble while the turn is in flight. */}
        {turn && (
          <AgentStream
            reasoning={turn.reasoning}
            answerStarted={turn.answerStarted}
            tools={turn.tools}
            live={phase === "thinking"}
          />
        )}
        {turn && phase === "thinking" && <AgentAnswer text={turn.answer} />}
      </ConversationContent>
      <ConversationScrollButton />
    </Conversation>
  );
}
