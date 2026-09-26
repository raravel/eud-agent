/**
 * 빌드 결과 — what the header's "프로젝트 빌드" button reports back.
 *
 * rules.md (## Build generation): euddraft diagnostics and a fresh output map
 * are the only build authority, warnings never fail a build, and the raw streams
 * never travel whole (euddraft prints every null tile on one line). So this
 * dialog shows the verdict, each diagnostic at its innermost project frame, and
 * the bounded head/tail excerpt — with `build/euddraft/build.log` named for the
 * rest. A build that never started carries its recovery action instead.
 */
import { AlertTriangleIcon, CheckCircle2Icon, ChevronRightIcon, XCircleIcon } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import type { BuildDiagnostic, ProjectBuildReport } from "@/lib/ipc";

export interface BuildResultDialogProps {
  open: boolean;
  /** The finished build, or null when it never produced a verdict. */
  report: ProjectBuildReport | null;
  /** Why the build never ran, in Korean, with its recovery action. */
  error?: string | null;
  onOpenChange(open: boolean): void;
}

/** `src/main.eps:12` when the frame is located, else the file alone. */
function location(diagnostic: BuildDiagnostic): string {
  return diagnostic.line > 0
    ? `${diagnostic.file}:${diagnostic.line}`
    : diagnostic.file;
}

function DiagnosticList({
  diagnostics,
  tone,
}: {
  diagnostics: BuildDiagnostic[];
  tone: "error" | "warning";
}) {
  const border = tone === "error" ? "border-destructive/40" : "border-amber-500/40";
  const text = tone === "error" ? "text-destructive" : "text-amber-400";
  return (
    <ul className="space-y-2">
      {diagnostics.map((diagnostic, index) => (
        <li
          key={`${diagnostic.file}:${diagnostic.line}:${index}`}
          className={`rounded-md border ${border} bg-muted/40 px-3 py-2`}
        >
          <div className="flex items-baseline gap-2 text-xs">
            <span className={`shrink-0 font-medium ${text}`}>
              {location(diagnostic)}
            </span>
            {diagnostic.count > 1 && (
              <span className="shrink-0 text-muted-foreground">
                ×{diagnostic.count}
              </span>
            )}
            <span className="ml-auto shrink-0 text-muted-foreground">
              {diagnostic.source}
            </span>
          </div>
          <p className="mt-1 whitespace-pre-wrap break-words text-sm">
            {diagnostic.message}
          </p>
          {diagnostic.raw && diagnostic.raw !== diagnostic.message && (
            <Collapsible>
              <CollapsibleTrigger asChild>
                <Button
                  type="button"
                  size="sm"
                  variant="ghost"
                  className="mt-1 h-auto gap-1 px-1.5 py-1 text-xs text-muted-foreground [&[data-state=open]>svg]:rotate-90"
                >
                  <ChevronRightIcon
                    aria-hidden
                    className="size-3 transition-transform motion-reduce:transition-none"
                  />
                  원본 출력
                </Button>
              </CollapsibleTrigger>
              <CollapsibleContent>
                <pre className="mt-1 max-h-48 overflow-auto rounded bg-background/60 p-2 text-[11px] leading-relaxed">
                  {diagnostic.raw}
                </pre>
              </CollapsibleContent>
            </Collapsible>
          )}
        </li>
      ))}
    </ul>
  );
}

export function BuildResultDialog({
  open,
  report,
  error,
  onOpenChange,
}: BuildResultDialogProps) {
  const ok = report?.ok === true;
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex max-h-[85vh] flex-col sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            {error ? (
              <AlertTriangleIcon aria-hidden className="size-4 text-amber-400" />
            ) : ok ? (
              <CheckCircle2Icon aria-hidden className="size-4 text-emerald-400" />
            ) : (
              <XCircleIcon aria-hidden className="size-4 text-destructive" />
            )}
            빌드 결과
            {report && (
              <Badge variant={ok ? "secondary" : "destructive"}>
                {ok ? "성공" : "실패"}
              </Badge>
            )}
          </DialogTitle>
          <DialogDescription>
            {error
              ? "빌드를 시작하지 못했습니다."
              : ok
                ? "euddraft가 출력 맵을 새로 만들었습니다."
                : "euddraft가 보고한 오류를 고친 뒤 다시 빌드해 주세요."}
          </DialogDescription>
        </DialogHeader>

        <div className="min-h-0 flex-1 space-y-4 overflow-y-auto">
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {error}
            </p>
          )}

          {report && (
            <>
              <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 text-xs">
                <dt className="text-muted-foreground">출력 맵</dt>
                <dd className="break-all">{report.outputMap}</dd>
                {report.deployedMap && (
                  <>
                    <dt className="text-muted-foreground">스타크래프트 복사본</dt>
                    <dd className="break-all">{report.deployedMap}</dd>
                  </>
                )}
                <dt className="text-muted-foreground">euddraft 종료 코드</dt>
                <dd>{report.rawStatus}</dd>
                {report.logPath && (
                  <>
                    <dt className="text-muted-foreground">전체 로그</dt>
                    <dd className="break-all">{report.logPath}</dd>
                  </>
                )}
              </dl>

              {report.errors.length > 0 && (
                <section className="space-y-2">
                  <h3 className="text-sm font-semibold text-destructive">
                    오류 {report.errors.length}개
                  </h3>
                  <DiagnosticList diagnostics={report.errors} tone="error" />
                </section>
              )}

              {report.warnings.length > 0 && (
                <section className="space-y-2">
                  <h3 className="text-sm font-semibold text-amber-400">
                    경고 {report.warnings.length}개
                  </h3>
                  <DiagnosticList diagnostics={report.warnings} tone="warning" />
                </section>
              )}

              {report.errors.length === 0 && report.warnings.length === 0 && (
                <p className="text-sm text-muted-foreground">
                  보고된 오류와 경고가 없습니다.
                </p>
              )}

              <Collapsible>
                <CollapsibleTrigger asChild>
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    className="gap-1.5 [&[data-state=open]>svg]:rotate-90"
                  >
                    <ChevronRightIcon
                      aria-hidden
                      className="size-3.5 transition-transform motion-reduce:transition-none"
                    />
                    euddraft 출력 보기
                  </Button>
                </CollapsibleTrigger>
                <CollapsibleContent>
                  <pre className="mt-2 max-h-64 overflow-auto rounded-md border border-border bg-muted/40 p-2 text-[11px] leading-relaxed">
                    {report.outputExcerpt || "(출력이 없습니다)"}
                  </pre>
                </CollapsibleContent>
              </Collapsible>
            </>
          )}
        </div>

        <DialogFooter>
          <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
            닫기
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
