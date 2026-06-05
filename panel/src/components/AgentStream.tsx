/**
 * Agent activity stream (features/06 ## Behaviors → Agent stream): a live
 * activity line rendered under the latest user message from the current turn's
 * `agent_event`s — "도구 호출 n건 · 현재: <last tool/detail>". When the turn ends
 * (the store phase leaves `thinking`, i.e. `live` goes false) it collapses into
 * a summary row (no spinner). With zero events it renders nothing.
 *
 * Kept a plain styled row (no markdown pipeline; the Spinner primitive is the
 * only shared dependency — dep-pruning carry-forward from ConversationLog).
 */
import { Spinner } from "@/components/ui/spinner";
import { cn } from "@/lib/utils";

/** One streamed agent activity (mirrors `agent_event {kind, detail}`). */
export interface AgentActivity {
  kind: string;
  detail: string;
}

export interface AgentStreamProps {
  /** The current turn's agent_event stream (chronological). */
  events: AgentActivity[];
  /** True while the turn is in flight (store phase === "thinking"). */
  live: boolean;
}

/** Tool-call events count toward "도구 호출 n건". */
const TOOL_CALL_KIND = "tool_call";

export function AgentStream({ events, live }: AgentStreamProps) {
  if (events.length === 0) return null;

  const toolCalls = events.filter((e) => e.kind === TOOL_CALL_KIND).length;
  const last = events[events.length - 1];
  const current = last.detail || last.kind;

  return (
    <div
      className={cn(
        "flex w-fit max-w-[95%] items-center gap-2 px-4 py-1 text-sm text-muted-foreground",
      )}
      aria-label="에이전트 활동"
    >
      {live && <Spinner className="size-3.5 shrink-0" />}
      <span className="whitespace-pre-wrap break-words">
        {`도구 호출 ${toolCalls}건 · 현재: ${current}`}
      </span>
    </div>
  );
}
