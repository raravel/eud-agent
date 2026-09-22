/**
 * The request a shutdown or a cancellation left unresolved.
 *
 * The stage strip only shows the controls while the interrupted request is
 * still the session's live one. Once the user sends another message that
 * request is set aside — it keeps its research, its plan, and whether the user
 * approved that plan — and this bar is where it stays visible and resolvable,
 * so a later message can never quietly discard approved work.
 *
 * Korean labels; the same `workflow_resume` / `workflow_restart` commands as
 * the strip's controls.
 */
import { OctagonPause, Play, RotateCcw } from "lucide-react";

import { Button } from "@/components/ui/button";
import type { WorkflowEvent, WorkflowStage } from "@/lib/ipc";

const STAGE_LABELS: Record<WorkflowStage, string> = {
  triage: "파악",
  clarify: "파악",
  research: "조사",
  planning: "계획",
  critique: "계획 검토",
  plan_review: "계획 승인",
  executing: "실행",
  verifying: "검증",
  changeset_review: "변경 검토",
  interrupted: "진행",
  cancelled: "진행",
  done: "진행",
  failed: "진행",
};

export interface InterruptedRequestBarProps {
  request: WorkflowEvent;
  actionBusy: boolean;
  onResume: () => void;
  onRestart: () => void;
}

export function InterruptedRequestBar({
  request,
  actionBusy,
  onResume,
  onRestart,
}: InterruptedRequestBarProps) {
  const stage = request.interruptedStage ?? request.stage;
  const approved =
    request.plan !== undefined &&
    request.plan.approvedSha256 === request.plan.sha256;
  const summary = request.goal?.trim() ?? "";
  return (
    <section
      aria-label="정리되지 않은 이전 요청"
      className="flex flex-wrap items-center gap-2 rounded-md border border-amber-500/40 bg-amber-500/5 px-3 py-2 text-sm"
    >
      <OctagonPause aria-hidden className="size-4 shrink-0 text-amber-400" />
      <span className="text-amber-400">
        {STAGE_LABELS[stage]} 단계에서 중단된 요청이 있습니다.
      </span>
      {summary !== "" && (
        <span className="min-w-0 flex-1 truncate text-muted-foreground" title={summary}>
          {summary}
        </span>
      )}
      {approved && (
        <span className="text-muted-foreground">승인된 계획 있음</span>
      )}
      <div className="ml-auto flex items-center gap-2">
        <Button
          type="button"
          size="sm"
          className="h-8"
          disabled={actionBusy}
          onClick={onResume}
        >
          <Play aria-hidden className="size-3.5" />
          이어서 진행
        </Button>
        <Button
          type="button"
          size="sm"
          variant="outline"
          className="h-8"
          disabled={actionBusy}
          onClick={onRestart}
        >
          <RotateCcw aria-hidden className="size-3.5" />
          처음부터
        </Button>
      </div>
    </section>
  );
}
