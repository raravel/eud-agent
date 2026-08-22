import { SquareIcon } from "lucide-react";

import { Shimmer } from "@/components/ai-elements/shimmer";
import type { TurnState } from "@/state/store";

function turnActivityLabel(turn: TurnState): string {
  for (let index = turn.tools.length - 1; index >= 0; index -= 1) {
    const tool = turn.tools[index];
    if (tool.state === "running") return `도구 실행 중 · ${tool.name}`;
  }
  if (turn.answerStarted) return "응답 작성 중";
  if (turn.tools.length > 0) return "도구 결과 확인 중";
  if (turn.reasoning.length > 0) return "추론 중";
  return "작업 준비 중";
}

export interface AgentTurnStatusProps {
  turn: TurnState;
  onCancel?(): void;
  cancelDisabled?: boolean;
}

export function AgentTurnStatus({
  turn,
  onCancel,
  cancelDisabled = false,
}: AgentTurnStatusProps) {
  return (
    <div
      data-testid="active-turn-status"
      role="status"
      aria-live="polite"
      aria-atomic="true"
      className="mb-2 flex min-h-11 items-center justify-between gap-3 rounded-lg border border-border bg-card/95 px-3 shadow-sm"
    >
      <div className="flex min-w-0 items-center gap-2 text-sm">
        <span
          aria-hidden
          className="size-2 shrink-0 animate-pulse rounded-full bg-emerald-400 motion-reduce:animate-none"
        />
        <Shimmer className="truncate">{turnActivityLabel(turn)}</Shimmer>
      </div>
      {onCancel && (
        <button
          type="button"
          aria-label="작업 중단"
          disabled={cancelDisabled}
          onClick={onCancel}
          className="flex min-h-11 shrink-0 cursor-pointer items-center gap-1.5 rounded-md px-3 text-sm text-muted-foreground transition-colors duration-200 hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-default disabled:opacity-50"
        >
          <SquareIcon aria-hidden className="size-3.5 fill-current" />
          중단
        </button>
      )}
    </div>
  );
}
