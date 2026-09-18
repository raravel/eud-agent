/**
 * Verifier verdict card (features/staged-workflow-plan.md ## Phase 6): the
 * pass/fail verdict, its summary, and the unmet acceptance criteria, rendered
 * above the changeset while `WorkflowEvent.verdict` is present. The store
 * archives the same verdict into the log as an ok/warn row.
 */
import { CheckCircle2, XCircle } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import type { WorkflowVerifyResult } from "@/lib/ipc";

export interface VerdictCardProps {
  verdict: WorkflowVerifyResult;
  /** Verify attempts consumed so far (shown as `검증 n/2`). */
  attempts?: number;
}

/** Verify attempts are soft-bounded at 2 (verifying → executing fix loop). */
const MAX_VERIFY_ATTEMPTS = 2;

export function VerdictCard({ verdict, attempts }: VerdictCardProps) {
  const passed = verdict.verdict === "pass";
  const Icon = passed ? CheckCircle2 : XCircle;

  return (
    <section
      aria-label="검증 결과"
      data-verdict={verdict.verdict}
      className="shrink-0 border-t border-border px-4 py-2"
    >
      <Card className="gap-0 overflow-hidden border-border bg-card/60 py-0 shadow-none">
        <CardHeader className="gap-2 px-4 py-3">
          <div className="flex items-start gap-3">
            <Icon
              className={`mt-0.5 size-4 shrink-0 ${passed ? "text-emerald-400" : "text-amber-400"}`}
              aria-hidden="true"
            />
            <div className="min-w-0 flex-1">
              <CardTitle className="text-sm">
                {passed ? "검증 통과" : "검증 실패"}
              </CardTitle>
              <p className="mt-1 whitespace-pre-wrap text-xs leading-5 text-muted-foreground">
                {verdict.summary}
              </p>
            </div>
            {attempts !== undefined && attempts > 0 && (
              <Badge variant="outline" className="shrink-0 text-[11px]">
                검증 {attempts}/{MAX_VERIFY_ATTEMPTS}
              </Badge>
            )}
          </div>
        </CardHeader>
        {verdict.unmet.length > 0 && (
          <CardContent className="border-t border-border px-4 py-3">
            <p className="text-xs font-medium text-foreground">미충족 항목</p>
            <ul aria-label="미충족 항목" className="mt-1.5 list-disc space-y-1 pl-5 text-xs leading-5 text-muted-foreground">
              {verdict.unmet.map((item, index) => (
                <li key={`${index}-${item}`}>{item}</li>
              ))}
            </ul>
          </CardContent>
        )}
      </Card>
    </section>
  );
}
