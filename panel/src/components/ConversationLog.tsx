/**
 * Conversation / event log (features/03 ## Behaviors → Conversation):
 *   instructions, progress entries (spinner on the ACTIVE stage incl.
 *   waiting_build), errors, applied confirmations — driven by the store's
 *   capped log.
 *
 * DEP PRUNING (carry-forward): log entries are composed as plain styled rows —
 * NO streamdown / shiki markdown pipeline (which the vendored ai-elements
 * message.tsx would drag in via @streamdown/*). The Spinner primitive is the
 * only shared dependency.
 */
import { Spinner } from "@/components/ui/spinner";
import { cn } from "@/lib/utils";
import type { LogEntry, LogKind, Phase } from "@/state/store";

export interface ConversationLogProps {
  /** Store log entries (kind / text / optional stage). */
  log: LogEntry[];
  /** Panel phase — decides whether the latest progress entry is "active". */
  phase: Phase;
}

/** Phases in which a live progress entry should still spin. */
const BUSY_PHASES: ReadonlySet<Phase> = new Set<Phase>([
  "working",
  "applying",
  "waiting",
]);

/** Per-kind text styling for a log row. */
const KIND_CLASS: Record<LogKind, string> = {
  info: "text-muted-foreground",
  you: "ml-auto rounded-lg bg-secondary px-3 py-2 text-foreground",
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
    <div className="flex flex-1 flex-col gap-2 overflow-y-auto p-4" role="log">
      {log.map((entry) => {
        const isActive = entry.id === activeProgressId;
        const testId = entry.stage ? `log-entry-${entry.stage}` : undefined;
        return (
          <div
            key={entry.id}
            data-testid={testId}
            className={cn(
              "flex w-fit max-w-[95%] items-center gap-2 text-sm",
              entry.kind === "you" && "self-end",
              KIND_CLASS[entry.kind],
            )}
          >
            {isActive && <Spinner className="size-3.5 shrink-0" />}
            <span className="whitespace-pre-wrap break-words">{entry.text}</span>
          </div>
        );
      })}
    </div>
  );
}
