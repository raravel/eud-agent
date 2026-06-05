/**
 * Plan review card (features/06 ## UI layout + Behaviors → Plan review), rebuilt
 * on the vendored AI-Elements Plan component + Streamdown (decision 06 — replaces
 * the EUD-060 hand-rolled line renderer):
 *   the proposed plan renders inside the Plan card; the markdown body renders via
 *   Streamdown (Response); a 피드백 textarea sends `plan_feedback{text}` (the panel
 *   STAYS in plan_review — the store keeps the card until the next
 *   `plan{revision+1}` replaces it, EUD-058); a [수정요청] button (feedback) and a
 *   [승인] button (`plan_approve{}`). Korean labels.
 *
 * Plan-feedback / plan-approve wiring + the `pending` gating are UNCHANGED from
 * EUD-058/060 (only the markdown renderer + card chrome changed). Revision
 * replacement is owned by the STORE; this component is a thin renderer of whatever
 * `plan` it is given.
 */
import { useState } from "react";
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
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";
import type { PlanState } from "@/state/store";

export interface PlanViewProps {
  /** The active plan card (markdown + revision). */
  plan: PlanState;
  /** A turn is in flight (feedback/approve already sent) — disable controls. */
  pending: boolean;
  /** Send plan_feedback{text}; the App fires the WS message + store action. */
  onFeedback(text: string): void;
  /** Send plan_approve{}; the App fires the WS message + store action. */
  onApprove(): void;
}

export function PlanView({ plan, pending, onFeedback, onApprove }: PlanViewProps) {
  const [feedback, setFeedback] = useState("");

  function handleFeedback() {
    const text = feedback.trim();
    if (pending || text.length === 0) return;
    onFeedback(text);
    setFeedback("");
  }

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

      <Textarea
        aria-label="피드백 입력"
        value={feedback}
        onChange={(e) => setFeedback(e.target.value)}
        placeholder="계획을 수정하려면 피드백을 입력하세요 (예: HP는 100으로)"
        className={cn("max-h-40 min-h-16 resize-none")}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
            e.preventDefault();
            handleFeedback();
          }
        }}
      />

      <div className="flex items-center justify-end gap-2">
        <Button
          type="button"
          variant="outline"
          disabled={pending}
          onClick={handleFeedback}
        >
          수정요청
        </Button>
        <Button type="button" disabled={pending} onClick={() => onApprove()}>
          승인
        </Button>
      </div>
    </section>
  );
}
