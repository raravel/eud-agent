/**
 * Conversation / event log (features/06 ## UI layout + Behaviors), rebuilt on the
 * vendored AI Elements (Conversation + Message) + Streamdown (decision 06):
 *   - user instructions render as Message bubbles (secondary);
 *   - agent answers render as PROMINENT (foreground) Message/Response bubbles via
 *     Streamdown — the most visible text in the log (answer prominence inverts the
 *     original v2 styling);
 *   - system/progress/info/ok/warn/error rows stay muted simple rows; the LATEST
 *     progress entry spins while the panel is busy (incl. waiting_build).
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
import type { LogEntry, LogKind, Phase } from "@/state/store";

export interface ConversationLogProps {
  /** Store log entries (kind / text / optional stage). */
  log: LogEntry[];
  /** Panel phase — decides whether the latest progress entry is "active". */
  phase: Phase;
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

export function ConversationLog({ log, phase }: ConversationLogProps) {
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
      </ConversationContent>
      <ConversationScrollButton />
    </Conversation>
  );
}
