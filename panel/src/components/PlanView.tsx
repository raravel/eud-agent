/**
 * Plan review card (features/06 ## UI layout + Behaviors → Plan review), built
 * on the vendored AI-Elements Plan component + Streamdown (decision 06):
 * the proposed plan renders inside the Plan card with a [승인] button
 * (`plan_approve{}`).
 *
 * EUD-074 (user decision 2026-06-05): the embedded feedback textarea and the
 * [수정요청] button are REMOVED — plan feedback flows through the MAIN prompt
 * input (typing there during plan_review sends `plan_feedback{text}`; App owns
 * the routing). Revision replacement is owned by the STORE; this component is
 * a thin renderer of whatever `plan` it is given. Korean labels.
 */
import { useEffect, useRef, useState } from "react";

import {
  Plan,
  PlanAction,
  PlanContent,
  PlanHeader,
  PlanTitle,
  PlanTrigger,
} from "@/components/ai-elements/plan";
import { DiagramResponse } from "@/components/ai-elements/response";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import type { WorkflowPlanArtifact } from "@/lib/ipc";
import type { PlanState } from "@/state/store";

const HEIGHT_KEY = "eud.plan-view.height";
const DEFAULT_HEIGHT = 320;
const MIN_HEIGHT = 180;
const MAX_HEIGHT = 720;
const RESERVED_VIEWPORT_HEIGHT = 320;
const KEYBOARD_STEP = 16;

function maximumHeight(): number {
  if (typeof window === "undefined") return MAX_HEIGHT;
  return Math.max(
    MIN_HEIGHT,
    Math.min(MAX_HEIGHT, window.innerHeight - RESERVED_VIEWPORT_HEIGHT),
  );
}

function clampHeight(height: number): number {
  return Math.round(Math.min(maximumHeight(), Math.max(MIN_HEIGHT, height)));
}

function readStoredHeight(): number {
  if (typeof localStorage === "undefined") return DEFAULT_HEIGHT;
  try {
    const stored = localStorage.getItem(HEIGHT_KEY);
    if (stored === null) return clampHeight(DEFAULT_HEIGHT);
    const saved = Number(stored);
    return Number.isFinite(saved)
      ? clampHeight(saved)
      : clampHeight(DEFAULT_HEIGHT);
  } catch {
    return clampHeight(DEFAULT_HEIGHT);
  }
}

export interface PlanViewProps {
  /** The active plan card (markdown + revision). */
  plan: PlanState;
  /**
   * Staged-workflow plan artifact (acceptance criteria, critic verdict, deep
   * planning); absent on the event-only path (features/staged-workflow-plan.md
   * ## Phase 5 Plan review).
   */
  artifact?: WorkflowPlanArtifact;
  /** Whether the plan body is expanded for the selected session. */
  open: boolean;
  /** Persist expansion changes in the selected session slot. */
  onOpenChange(open: boolean): void;
  /** A turn is in flight (approve already sent / feedback running) — disable. */
  pending: boolean;
  /** Send plan_approve{}; the App invokes the command + store action. */
  onApprove(): void;
}

export function PlanView({
  plan,
  artifact,
  open,
  onOpenChange,
  pending,
  onApprove,
}: PlanViewProps) {
  const [height, setHeight] = useState(readStoredHeight);
  const dragRef = useRef<{ startY: number; startHeight: number } | null>(null);

  useEffect(() => {
    try {
      localStorage.setItem(HEIGHT_KEY, String(height));
    } catch {
      // Height persistence is optional; current-window resizing still works.
    }
  }, [height]);

  useEffect(() => {
    const handleResize = () => setHeight((current) => clampHeight(current));
    window.addEventListener("resize", handleResize);
    return () => window.removeEventListener("resize", handleResize);
  }, []);

  const handleApprove = () => {
    onOpenChange(false);
    onApprove();
  };

  return (
    <section
      aria-label="계획 검토"
      style={open ? { height } : undefined}
      className="relative flex min-h-0 shrink-0 flex-col gap-3 overflow-hidden border-t border-border p-4"
    >
      {open && (
        <div
          role="separator"
          aria-orientation="horizontal"
          aria-label="계획 패널 높이 조절"
          aria-valuemin={MIN_HEIGHT}
          aria-valuemax={maximumHeight()}
          aria-valuenow={height}
          tabIndex={0}
          onDoubleClick={() => setHeight(clampHeight(DEFAULT_HEIGHT))}
          onKeyDown={(event) => {
            if (event.key === "ArrowUp") {
              event.preventDefault();
              setHeight((current) => clampHeight(current + KEYBOARD_STEP));
            } else if (event.key === "ArrowDown") {
              event.preventDefault();
              setHeight((current) => clampHeight(current - KEYBOARD_STEP));
            } else if (event.key === "Home") {
              event.preventDefault();
              setHeight(MIN_HEIGHT);
            } else if (event.key === "End") {
              event.preventDefault();
              setHeight(maximumHeight());
            }
          }}
          onPointerDown={(event) => {
            dragRef.current = {
              startY: event.clientY,
              startHeight: height,
            };
            event.currentTarget.setPointerCapture(event.pointerId);
            event.preventDefault();
          }}
          onPointerMove={(event) => {
            const drag = dragRef.current;
            if (!drag) return;
            setHeight(
              clampHeight(drag.startHeight + drag.startY - event.clientY),
            );
          }}
          onPointerUp={(event) => {
            if (!dragRef.current) return;
            dragRef.current = null;
            if (event.currentTarget.hasPointerCapture(event.pointerId)) {
              event.currentTarget.releasePointerCapture(event.pointerId);
            }
          }}
          onPointerCancel={() => {
            dragRef.current = null;
          }}
          className="group/splitter absolute inset-x-0 top-0 z-10 h-3 -translate-y-1/2 cursor-row-resize touch-none outline-none"
        >
          <span className="absolute inset-x-0 top-1/2 h-px -translate-y-1/2 bg-border transition-colors group-hover/splitter:h-0.5 group-hover/splitter:bg-primary/70 group-focus-visible/splitter:h-0.5 group-focus-visible/splitter:bg-primary group-active/splitter:h-0.5 group-active/splitter:bg-primary" />
        </div>
      )}

      <Plan
        open={open}
        onOpenChange={onOpenChange}
        className="min-h-0 flex-1 gap-3 overflow-hidden py-3"
      >
        <PlanHeader className="shrink-0 px-3">
          <PlanTitle className="text-sm">{`계획안 (rev ${plan.revision})`}</PlanTitle>
          <PlanAction className="flex items-center gap-1.5">
            {artifact?.deep && (
              <Badge variant="outline" className="text-[11px]">
                심층 계획
                {artifact.iterations > 0 ? ` · ${artifact.iterations}회` : ""}
              </Badge>
            )}
            {artifact?.criticVerdict !== undefined && (
              <Badge
                variant={artifact.criticVerdict === "approve" ? "secondary" : "outline"}
                className="text-[11px]"
              >
                {artifact.criticVerdict === "approve" ? "비평 승인" : "비평 수정 요청"}
              </Badge>
            )}
            <PlanTrigger aria-label={open ? "계획안 접기" : "계획안 펼치기"} />
          </PlanAction>
        </PlanHeader>
        <PlanContent className="min-h-0 flex-1 overflow-y-auto px-3 text-sm">
          {artifact && (
            <div className="mb-3 flex flex-col gap-2 rounded-lg border border-border bg-card/40 px-3 py-2 text-xs">
              {artifact.title.trim().length > 0 && (
                <p className="font-medium text-foreground">{artifact.title}</p>
              )}
              {artifact.acceptanceCriteria.length > 0 && (
                <div>
                  <p className="font-medium text-foreground">수용 기준</p>
                  <ul
                    aria-label="수용 기준"
                    className="mt-1 list-disc space-y-0.5 pl-5 leading-5 text-muted-foreground"
                  >
                    {artifact.acceptanceCriteria.map((criterion, index) => (
                      <li key={`${index}-${criterion}`}>{criterion}</li>
                    ))}
                  </ul>
                </div>
              )}
              {artifact.criticSummary !== undefined &&
                artifact.criticSummary.trim().length > 0 && (
                  <div>
                    <p className="font-medium text-foreground">비평 요약</p>
                    <p className="mt-1 whitespace-pre-wrap leading-5 text-muted-foreground">
                      {artifact.criticSummary}
                    </p>
                  </div>
                )}
            </div>
          )}
          {/* Key on the revision: a new plan is a FULL replacement (not a
              streaming append), so remount Streamdown to avoid stale cached
              blocks from the previous revision. */}
          <DiagramResponse key={plan.revision} mode="static">
            {plan.markdown}
          </DiagramResponse>
        </PlanContent>
      </Plan>

      <div
        data-testid="plan-actions"
        className="flex shrink-0 items-center justify-between gap-2 border-t border-border bg-background pt-3"
      >
        <span className="text-xs text-muted-foreground">
          수정하려면 아래 입력창에 피드백을 입력하세요.
        </span>
        <Button type="button" disabled={pending} onClick={handleApprove}>
          승인
        </Button>
      </div>
    </section>
  );
}
