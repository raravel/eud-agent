/**
 * Agent activity stream (features/06 ## Behaviors → Agent stream), rebuilt on the
 * vendored AI Elements (Reasoning + Tool) + Streamdown (decision 06):
 *   - `reasoning` text renders into the Reasoning block: dim/secondary, GPT-style,
 *     collapsible. It auto-opens while reasoning streams (no answer yet) and
 *     auto-collapses once the answer starts (`answerStarted`); the user can then
 *     MANUALLY re-expand it by clicking the trigger (GPT-style — F1 review fix).
 *     A user toggle wins over the auto behavior until the next turn resets it
 *     (the override clears when the per-turn reasoning buffer empties);
 *   - `tool_call`/`tool_result` render as Tool rows by tool name, with a
 *     "도구 호출 n건" summary row;
 *   - raw internal kind identifiers NEVER appear as literal text — this component
 *     only ever renders the reasoning text, the tool NAMES, and Korean labels.
 *
 * With no reasoning and no tools it renders nothing (null).
 */
import { useEffect, useState } from "react";
import { WrenchIcon } from "lucide-react";
import {
  Reasoning,
  ReasoningContent,
  ReasoningTrigger,
} from "@/components/ai-elements/reasoning";
import {
  Tool,
  ToolContent,
  ToolHeader,
} from "@/components/ai-elements/tool";

/** One tool-call row (mirrors the store's per-turn AgentTool). */
export interface AgentTool {
  id: string;
  name: string;
  state: "running" | "done" | "failed";
  /** Tool-call argument text (EUD-068). */
  args?: string;
  /** Tool-result text (EUD-068). */
  detail?: string;
}

export interface AgentStreamProps {
  /** Accumulated reasoning delta text. */
  reasoning: string;
  /** True once answer delta text has begun (collapses the Reasoning block). */
  answerStarted: boolean;
  /** Tool rows from tool_call/tool_result. */
  tools: AgentTool[];
  /** True while the turn is in flight (store phase === "thinking"). */
  live: boolean;
}

export function AgentStream({
  reasoning,
  answerStarted,
  tools,
  live,
}: AgentStreamProps) {
  // User override of the auto open/collapse behavior (F1): null = follow the
  // auto behavior; true/false = the user explicitly opened/closed the block.
  const [userOpen, setUserOpen] = useState<boolean | null>(null);

  const hasReasoning = reasoning.length > 0;

  // Reset the override at the start of each turn — a fresh turn empties the
  // reasoning buffer, so a stale override does not carry across turns.
  useEffect(() => {
    if (!hasReasoning) setUserOpen(null);
  }, [hasReasoning]);

  if (!hasReasoning && tools.length === 0) return null;

  // Auto behavior: open ONLY while reasoning actively streams (live turn, answer
  // not started yet); collapse once the answer begins AND once the turn completes
  // (`live` false). A completed reasoning block ("추론 완료") therefore defaults
  // CLOSED — the store's turn-end archive resets `answerStarted` to false while
  // keeping the reasoning text, so without the `live` gate the finished block
  // would spuriously re-open. A user toggle (`userOpen`) overrides it so the
  // collapsed block can still be MANUALLY re-expanded afterwards. `open` is fully
  // controlled so the collapsed content leaves the DOM (no leaked text).
  const autoOpen = live && hasReasoning && !answerStarted;
  const reasoningOpen = userOpen !== null ? userOpen : autoOpen;
  const reasoningStreaming = live && !answerStarted;

  return (
    <div
      // No horizontal padding of its own — rendered INLINE inside the
      // conversation scroll content (which already pads, EUD-069).
      className="flex w-full max-w-[95%] flex-col gap-2"
      aria-label="에이전트 활동"
    >
      {hasReasoning && (
        <Reasoning
          isStreaming={reasoningStreaming}
          open={reasoningOpen}
          onOpenChange={setUserOpen}
        >
          <ReasoningTrigger />
          <ReasoningContent>{reasoning}</ReasoningContent>
        </Reasoning>
      )}

      {tools.length > 0 && (
        <div className="flex flex-col gap-1 my-3">
          <span className="flex items-center gap-1.5 text-xs text-muted-foreground">
            <WrenchIcon aria-hidden className="size-3.5 shrink-0" />
            도구 호출 {tools.length}건
          </span>
          <ToolList tools={tools} />
        </div>
      )}
    </div>
  );
}

/**
 * The expandable Tool card rows (shared, EUD-069): rendered live by
 * {@link AgentStream} and again by ConversationLog for ARCHIVED tool entries
 * (`LogEntry.tools`), so past turns' tool activity stays inspectable in the
 * conversation history.
 */
export function ToolList({ tools }: { tools: AgentTool[] }) {
  return (
    <>
      {tools.map((tool) => (
        <Tool key={tool.id} data-testid={`tool-${tool.id}`}>
          <ToolHeader title={tool.name} state={tool.state} />
          {(tool.args || tool.detail) && (
            <ToolContent>
              <div className="flex flex-col gap-2 p-3 text-xs text-muted-foreground">
                {tool.args && (
                  <div>
                    <div className="mb-1 font-medium">요청</div>
                    <pre className="overflow-x-auto whitespace-pre-wrap break-words rounded bg-muted/40 p-2">
                      {tool.args}
                    </pre>
                  </div>
                )}
                {tool.detail && (
                  <div>
                    <div className="mb-1 font-medium">결과</div>
                    <pre className="overflow-x-auto whitespace-pre-wrap break-words rounded bg-muted/40 p-2">
                      {tool.detail}
                    </pre>
                  </div>
                )}
              </div>
            </ToolContent>
          )}
        </Tool>
      ))}
    </>
  );
}
