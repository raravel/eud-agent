/**
 * Staged-workflow stage strip (features/staged-workflow-plan.md ## Phase 2
 * Panel): 파악 → 조사 → 계획 → 승인 → 실행 → 검증 → 검토 rendered above the
 * conversation while a pipeline request is active, under review, or
 * interrupted. The App decides visibility (answer/direct routes hide the
 * strip once triage resolves); this component is a thin renderer of the
 * session's last `WorkflowEvent` plus the phase-derived controls:
 *   - a cancel control while a stage job is busy (reuses the turn cancel);
 *   - 이어서 진행 / 처음부터 for `interrupted` (workflow_resume / workflow_restart);
 *   - 취소됨 + 처음부터 for `cancelled` (terminal; artifacts retained).
 * Korean labels; `aria-current="step"` on the active step; reduced-motion safe.
 */
import {
  Ban,
  Check,
  CircleDashed,
  LoaderCircle,
  OctagonPause,
  RotateCcw,
  Play,
  SquareIcon,
} from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import type { WorkflowEvent, WorkflowStage } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { isBusyPhase, type Phase } from "@/state/store";

/** The strip steps in order; each step covers one or more workflow stages. */
const STEPS: ReadonlyArray<{
  id: string;
  label: string;
  stages: readonly WorkflowStage[];
}> = [
  { id: "triage", label: "파악", stages: ["triage", "clarify"] },
  { id: "research", label: "조사", stages: ["research"] },
  { id: "planning", label: "계획", stages: ["planning", "critique"] },
  { id: "plan_review", label: "승인", stages: ["plan_review"] },
  { id: "executing", label: "실행", stages: ["executing"] },
  { id: "verifying", label: "검증", stages: ["verifying"] },
  { id: "changeset_review", label: "검토", stages: ["changeset_review"] },
];

/** Verify attempts are soft-bounded at 2 (verifying → executing fix loop). */
const MAX_VERIFY_ATTEMPTS = 2;

type StepState = "completed" | "active" | "pending";

/** Index of the step covering `stage`, or -1 for stages outside the strip. */
function stepIndexFor(stage: WorkflowStage | undefined): number {
  if (stage === undefined) return -1;
  return STEPS.findIndex((step) => step.stages.includes(stage));
}

/**
 * The step the strip highlights: the current stage, the interrupted stage
 * while interrupted or cancelled (when known), or past the last step once
 * done (`failed` highlights nothing; the App hides the strip for it).
 */
function activeStepIndex(workflow: WorkflowEvent): number {
  switch (workflow.stage) {
    case "interrupted":
    case "cancelled":
      return stepIndexFor(workflow.interruptedStage);
    case "done":
      return STEPS.length;
    default:
      return stepIndexFor(workflow.stage);
  }
}

function stepStateAt(index: number, active: number): StepState {
  if (index < active) return "completed";
  if (index === active) return "active";
  return "pending";
}

export interface WorkflowStripProps {
  workflow: WorkflowEvent;
  phase: Phase;
  /** A cancel/resume/restart command is waiting for the core. */
  actionBusy: boolean;
  onCancel(): void;
  onResume(): void;
  onRestart(): void;
}

export function WorkflowStrip({
  workflow,
  phase,
  actionBusy,
  onCancel,
  onResume,
  onRestart,
}: WorkflowStripProps) {
  const active = activeStepIndex(workflow);
  const busy = isBusyPhase(phase);
  // The snapshot stays `interrupted` until the backend re-emits the resumed
  // stage; the phase already left `interrupted` once resume/restart was sent,
  // so the controls follow the phase and the step shows the busy spinner.
  const interrupted = workflow.stage === "interrupted" && phase === "interrupted";
  const cancelled = workflow.stage === "cancelled" && !busy;
  // The active step of a halted request renders as interrupted/cancelled.
  const halted: "interrupted" | "cancelled" | null = interrupted
    ? "interrupted"
    : cancelled
      ? "cancelled"
      : null;

  const counterFor = (stepId: string): string | null => {
    if (stepId === "verifying" && workflow.verifyAttempts > 0) {
      return `${workflow.verifyAttempts}/${MAX_VERIFY_ATTEMPTS}`;
    }
    if (stepId === "planning" && workflow.critiqueRounds > 0) {
      return `비평 ${workflow.critiqueRounds}회`;
    }
    return null;
  };

  return (
    <nav
      aria-label="작업 단계"
      data-testid="workflow-strip"
      data-stage={workflow.stage}
      className="flex shrink-0 flex-wrap items-center gap-x-3 gap-y-2 border-b border-border bg-card/30 px-4 py-2 text-xs"
    >
      <ol className="flex min-w-0 flex-1 flex-wrap items-center gap-1">
        {STEPS.map((step, index) => {
          const state = stepStateAt(index, active);
          const counter = counterFor(step.id);
          const stepHalted = halted !== null && state === "active" ? halted : null;
          return (
            <li
              key={step.id}
              data-state={stepHalted ?? state}
              aria-current={state === "active" ? "step" : undefined}
              className={cn(
                "flex items-center gap-1.5 rounded-md px-2 py-1 transition-colors motion-reduce:transition-none",
                state === "completed" && "text-primary",
                state === "active" &&
                  stepHalted === null &&
                  "bg-primary/10 font-medium text-foreground",
                stepHalted === "interrupted" &&
                  "bg-amber-500/10 font-medium text-amber-400",
                stepHalted === "cancelled" && "font-medium text-muted-foreground",
                state === "pending" && "text-muted-foreground",
              )}
            >
              {state === "completed" ? (
                <Check aria-hidden className="size-3.5 shrink-0" />
              ) : stepHalted === "interrupted" ? (
                <OctagonPause aria-hidden className="size-3.5 shrink-0" />
              ) : stepHalted === "cancelled" ? (
                <Ban aria-hidden className="size-3.5 shrink-0" />
              ) : state === "active" && busy ? (
                <LoaderCircle
                  aria-hidden
                  className="size-3.5 shrink-0 animate-spin motion-reduce:animate-none"
                />
              ) : (
                <CircleDashed aria-hidden className="size-3.5 shrink-0" />
              )}
              <span>{step.label}</span>
              {counter !== null && (
                <span className="text-[11px] tabular-nums text-muted-foreground">
                  {counter}
                </span>
              )}
              {stepHalted === "interrupted" && <span className="sr-only">중단됨</span>}
            </li>
          );
        })}
      </ol>

      <div className="flex shrink-0 items-center gap-2">
        {workflow.deepPlanning && (
          <Badge variant="outline" className="text-[11px]">
            심층 계획
          </Badge>
        )}
        {busy && (
          <Button
            type="button"
            size="sm"
            variant="outline"
            className="h-8"
            disabled={actionBusy}
            aria-label="작업 중단"
            onClick={onCancel}
          >
            <SquareIcon aria-hidden className="size-3.5" />
            중단
          </Button>
        )}
        {interrupted && (
          <>
            <span role="status" className="text-amber-400">
              작업이 중단되었습니다.
            </span>
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
          </>
        )}
        {cancelled && (
          <span role="status" className="flex items-center gap-1 text-muted-foreground">
            <Ban aria-hidden className="size-3.5" />
            취소됨
          </span>
        )}
        {(interrupted || cancelled) && (
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
        )}
      </div>
    </nav>
  );
}
