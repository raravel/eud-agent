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
  state: "running" | "done";
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

  // Auto behavior: open while reasoning streams (the answer has not started yet),
  // collapse once the answer begins. A user toggle (`userOpen`) overrides it so
  // the collapsed block can be MANUALLY re-expanded afterwards. `open` is fully
  // controlled so the collapsed content leaves the DOM (no leaked text).
  const autoOpen = hasReasoning && !answerStarted;
  const reasoningOpen = userOpen !== null ? userOpen : autoOpen;
  const reasoningStreaming = live && !answerStarted;

  return (
    <div
      className="flex w-full max-w-[95%] flex-col gap-2 px-4"
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
        <div className="flex flex-col gap-1">
          <span className="text-xs text-muted-foreground">
            도구 호출 {tools.length}건
          </span>
          {tools.map((tool) => (
            <Tool key={tool.id} data-testid={`tool-${tool.id}`}>
              <ToolHeader title={tool.name} state={tool.state} />
              {tool.detail && (
                <ToolContent>
                  <div className="p-3 text-xs whitespace-pre-wrap break-words text-muted-foreground">
                    {tool.detail}
                  </div>
                </ToolContent>
              )}
            </Tool>
          ))}
        </div>
      )}
    </div>
  );
}
