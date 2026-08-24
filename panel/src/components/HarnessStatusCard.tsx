import { useMemo, useState } from "react";
import {
  AlertTriangle,
  BookCheck,
  CheckCircle2,
  Clock3,
  LoaderCircle,
  RotateCcw,
  ShieldQuestion,
  XCircle,
  X,
} from "lucide-react";

import { ChangesetView } from "@/components/ChangesetView";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import type { HarnessJobView } from "@/lib/protocol";
import type { ChangesetState } from "@/state/store";

export interface HarnessStatusCardProps {
  jobs: HarnessJobView[];
  pendingJobId: string | null;
  onRuntimeConfirm(jobId: string): void;
  onSkip(jobId: string): void;
  onRetry(jobId: string): void;
  onDismiss(jobId: string): void;
  onDecide(jobId: string, decision: "accept" | "reject"): void;
}

const ACTIVE: Record<HarnessJobView["status"], boolean> = {
  waiting_runtime: true,
  pending: true,
  running: true,
  review: true,
  failed: true,
  completed: false,
  rejected: false,
  skipped: false,
};

const DISMISSIBLE: Record<HarnessJobView["status"], boolean> = {
  waiting_runtime: false,
  pending: false,
  running: false,
  review: false,
  failed: true,
  completed: true,
  rejected: true,
  skipped: true,
};

function statusPresentation(job: HarnessJobView) {
  switch (job.status) {
    case "waiting_runtime":
      return {
        icon: ShieldQuestion,
        label: "인게임 검증 대기",
        tone: "text-amber-400",
        detail: "렌더링·타이밍·충돌 동작을 게임에서 확인한 뒤 문서를 확정합니다.",
      };
    case "pending":
      return {
        icon: Clock3,
        label: "하네스 대기",
        tone: "text-muted-foreground",
        detail: "승인된 변경을 기준으로 문서 동기화를 준비하고 있습니다.",
      };
    case "running":
      return {
        icon: LoaderCircle,
        label: "하네스 동기화 중",
        tone: "text-primary",
        detail: "한 번의 구조화된 모델 호출로 명세 델타를 생성하고 있습니다.",
      };
    case "review":
      return {
        icon: BookCheck,
        label: "문서 변경 검토",
        tone: "text-amber-400",
        detail: "코드와 분리된 명세·작업 기록 변경사항입니다.",
      };
    case "failed":
      return {
        icon: AlertTriangle,
        label: "하네스 실패",
        tone: "text-destructive",
        detail: job.error ?? "문서 동기화에 실패했습니다.",
      };
    case "completed":
      return {
        icon: CheckCircle2,
        label: "하네스 완료",
        tone: "text-emerald-400",
        detail: job.summary ?? "문서 동기화가 승인되었습니다.",
      };
    case "skipped":
      return {
        icon: XCircle,
        label: "하네스 건너뜀",
        tone: "text-muted-foreground",
        detail: "인게임 검증과 문서·메모리 동기화를 건너뛰었습니다.",
      };
    default:
      return {
        icon: XCircle,
        label: "하네스 되돌림",
        tone: "text-muted-foreground",
        detail: "문서 동기화 변경사항을 적용하지 않았습니다.",
      };
  }
}

function HarnessJobCard({
  job,
  pending,
  onRuntimeConfirm,
  onSkip,
  onRetry,
  onDecide,
  onDismiss,
}: {
  job: HarnessJobView;
  pending: boolean;
  onRuntimeConfirm(jobId: string): void;
  onSkip(jobId: string): void;
  onRetry(jobId: string): void;
  onDecide(jobId: string, decision: "accept" | "reject"): void;
  onDismiss(jobId: string): void;
}) {
  const [open, setOpen] = useState(true);
  const presentation = statusPresentation(job);
  const Icon = presentation.icon;
  const changeset: ChangesetState | null = job.changeset
    ? { request_id: job.changeset.request_id, items: job.changeset.items, decisions: {} }
    : null;

  return (
    <Card className="gap-0 overflow-hidden border-border bg-card/60 py-0 shadow-none">
      <CardHeader className="gap-2 px-4 py-3">
        <div className="flex items-start gap-3">
          <Icon
            className={`mt-0.5 size-4 shrink-0 ${presentation.tone} ${job.status === "running" ? "animate-spin motion-reduce:animate-none" : ""}`}
            aria-hidden="true"
          />
          <div className="min-w-0 flex-1">
            <CardTitle className="text-sm">{presentation.label}</CardTitle>
            <p className="mt-1 text-xs leading-5 text-muted-foreground">
              {presentation.detail}
            </p>
          </div>
          <div className="flex shrink-0 items-center gap-1">
            <Badge variant="outline" className="text-[11px]">
              시도 {job.attempts}
            </Badge>
            {DISMISSIBLE[job.status] && (
              <Button
                type="button"
                size="icon"
                variant="ghost"
                className="size-8"
                disabled={pending}
                aria-label="하네스 상태 닫기"
                onClick={() => onDismiss(job.id)}
              >
                <X className="size-4" aria-hidden="true" />
              </Button>
            )}
          </div>
        </div>
      </CardHeader>

      {(job.status === "waiting_runtime" || job.status === "failed") && (
        <CardContent className="flex justify-end gap-2 border-t border-border px-4 py-3">
          {job.status === "waiting_runtime" ? (
            <>
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={pending}
                onClick={() => onSkip(job.id)}
              >
                건너뛰기
              </Button>
              <Button
                type="button"
                size="sm"
                disabled={pending}
                onClick={() => onRuntimeConfirm(job.id)}
              >
                <CheckCircle2 className="size-4" aria-hidden="true" />
                인게임 검증 완료
              </Button>
            </>
          ) : (
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={pending}
              onClick={() => onRetry(job.id)}
            >
              <RotateCcw className="size-4" aria-hidden="true" />
              다시 시도
            </Button>
          )}
        </CardContent>
      )}

      {job.status === "review" && changeset && (
        <ChangesetView
          changeset={changeset}
          open={open}
          onOpenChange={setOpen}
          pending={pending}
          title="하네스 문서 적용"
          bulkOnly
          onDecide={(decision) => onDecide(job.id, decision)}
        />
      )}
    </Card>
  );
}

export function HarnessStatusCard({
  jobs,
  pendingJobId,
  onRuntimeConfirm,
  onSkip,
  onRetry,
  onDecide,
  onDismiss,
}: HarnessStatusCardProps) {
  const visible = useMemo(() => {
    const ordered = [...jobs].sort(
      (left, right) => right.updatedAt - left.updatedAt || right.id.localeCompare(left.id),
    );
    const active = ordered.filter((job) => !job.dismissed && ACTIVE[job.status]);
    const latestTerminal = ordered.find((job) => !ACTIVE[job.status]);
    return latestTerminal &&
      !latestTerminal.dismissed &&
      latestTerminal.status !== "completed" &&
      latestTerminal.status !== "skipped"
      ? [...active, latestTerminal]
      : active;
  }, [jobs]);

  if (visible.length === 0) return null;

  return (
    <section
      aria-label="하네스 동기화"
      aria-live="polite"
      className="flex max-h-[46vh] min-h-0 flex-col gap-2 overflow-y-auto border-t border-border bg-background px-4 py-3"
    >
      {visible.map((job) => (
        <HarnessJobCard
          key={job.id}
          job={job}
          pending={pendingJobId === job.id}
          onSkip={onSkip}
          onRuntimeConfirm={onRuntimeConfirm}
          onRetry={onRetry}
          onDismiss={onDismiss}
          onDecide={onDecide}
        />
      ))}
    </section>
  );
}
