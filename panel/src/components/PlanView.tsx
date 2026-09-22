/**
 * Plan tab (features/staged-workflow-plan.md ## Phase 2 Panel): the body of the
 * center-column "계획 (rev N)" tab, built on the vendored AI-Elements Plan
 * component + Streamdown (decision 06). The plan renders full-height like an
 * opened markdown document with a [승인] button (`plan_approve{}`) in its
 * header while the plan awaits review; once approved the tab stays open as a
 * read-only reference.
 *
 * EUD-074 (user decision 2026-06-05): no embedded feedback textarea and no
 * [수정요청] button — plan feedback flows through the MAIN prompt input (typing
 * there during plan_review sends `plan_feedback{text}`; App owns the routing).
 * Revision replacement is owned by the STORE; this component is a thin
 * renderer of whatever `plan` it is given. Korean labels.
 */
import { DiagramResponse } from "@/components/ai-elements/response";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import type { WorkflowPlanArtifact } from "@/lib/ipc";
import type { PlanState } from "@/state/store";

export interface PlanViewProps {
  /** The active plan (markdown + revision). */
  plan: PlanState;
  /**
   * Staged-workflow plan artifact (acceptance criteria, critic verdict, deep
   * planning); absent on the event-only path (features/staged-workflow-plan.md
   * ## Phase 5 Plan review).
   */
  artifact?: WorkflowPlanArtifact;
  /** The plan awaits the user's decision (phase `plan_review`). */
  reviewable: boolean;
  /** A turn is in flight (approve already sent / feedback running) — disable. */
  pending: boolean;
  /** Send plan_approve{}; the App invokes the command + store action. */
  onApprove(): void;
}

export function PlanView({
  plan,
  artifact,
  reviewable,
  pending,
  onApprove,
}: PlanViewProps) {
  return (
    <section
      aria-label="계획 검토"
      data-slot="plan"
      className="flex h-full min-h-0 min-w-0 flex-col overflow-hidden"
    >
      <header className="flex shrink-0 flex-wrap items-center gap-2 border-b border-border bg-muted/30 px-4 py-2 text-xs">
        <h2 className="min-w-0 flex-1 truncate text-sm font-medium text-foreground">
          {`계획안 (rev ${plan.revision})`}
        </h2>
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
        {reviewable ? (
          <div
            data-testid="plan-actions"
            className="flex shrink-0 items-center gap-3"
          >
            <span className="text-muted-foreground">
              수정하려면 아래 입력창에 피드백을 입력하세요.
            </span>
            <Button type="button" size="sm" disabled={pending} onClick={onApprove}>
              승인
            </Button>
          </div>
        ) : (
          <Badge variant="outline" className="shrink-0 text-[11px]">
            {artifact !== undefined && artifact.approvedSha256 === artifact.sha256
              ? "승인됨 · 읽기 전용"
              : "읽기 전용"}
          </Badge>
        )}
      </header>

      <div className="min-h-0 flex-1 overflow-y-auto p-4">
        <div className="mx-auto max-w-4xl text-sm leading-7">
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
        </div>
      </div>
    </section>
  );
}
