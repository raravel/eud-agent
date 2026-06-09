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
import {
  Plan,
  PlanAction,
  PlanContent,
  PlanHeader,
  PlanTitle,
  PlanTrigger,
} from "@/components/ai-elements/plan";
import { Response } from "@/components/ai-elements/response";
import { Button } from "@/components/ui/button";
import type { PlanState } from "@/state/store";

export interface PlanViewProps {
  /** The active plan card (markdown + revision). */
  plan: PlanState;
  /** A turn is in flight (approve already sent / feedback running) — disable. */
  pending: boolean;
  /** Send plan_approve{}; the App invokes the command + store action. */
  onApprove(): void;
}

export function PlanView({ plan, pending, onApprove }: PlanViewProps) {
  return (
    <section
      aria-label="계획 검토"
      className="flex max-h-[40vh] flex-col gap-3 overflow-y-auto border-t border-border p-4"
    >
      <Plan defaultOpen className="gap-3 py-3">
        <PlanHeader className="px-3">
          <PlanTitle className="text-sm">{`계획안 (rev ${plan.revision})`}</PlanTitle>
          <PlanAction>
            <PlanTrigger />
          </PlanAction>
        </PlanHeader>
        <PlanContent className="px-3 text-sm">
          {/* Key on the revision: a new plan is a FULL replacement (not a
              streaming append), so remount Streamdown to avoid stale cached
              blocks from the previous revision. */}
          <Response key={plan.revision}>{plan.markdown}</Response>
        </PlanContent>
      </Plan>

      <div className="flex items-center justify-between gap-2">
        <span className="text-xs text-muted-foreground">
          수정하려면 아래 입력창에 피드백을 입력하세요.
        </span>
        <Button type="button" disabled={pending} onClick={() => onApprove()}>
          승인
        </Button>
      </div>
    </section>
  );
}
