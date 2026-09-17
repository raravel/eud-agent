import { useEffect, useRef, useState } from "react";
import {
  CircleAlertIcon,
  FileInputIcon,
  FolderOpenIcon,
  Loader2Icon,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import type {
  E3sDestinationSelection,
  E3sImportRequest,
  E3sSourceSelection,
} from "@/lib/projectImport";
import { formatImportIssueReason } from "@/lib/projectImport";
import type { HarnessImportIssue, SetupMessage } from "@/lib/protocol";
import { formatPathForDisplay } from "@/lib/utils";
import { ImportStep } from "@/setup/ImportStep";

export interface E3sImportDialogProps {
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
  readonly pickSource: () => Promise<E3sSourceSelection | null>;
  readonly pickDestination: () => Promise<E3sDestinationSelection | null>;
  readonly importProject: (request: E3sImportRequest) => Promise<SetupMessage>;
  readonly onImported: (setup: SetupMessage) => void;
}

function importFailureText(error: string): string {
  if (error.startsWith("e3s_import_source_map_missing:")) {
    return "E3S가 참조하는 원본 맵을 찾지 못했습니다. E3S와 원본 SCX/SCM 파일을 같은 폴더에 둔 뒤 E3S를 다시 선택해 주세요.";
  }
  if (error.startsWith("e3s_import_destination_not_empty:")) {
    return "선택한 작업 폴더가 비어 있지 않습니다. 새 빈 폴더를 만든 뒤 다시 선택해 주세요.";
  }
  if (error.startsWith("e3s_import_unsupported:")) {
    return "이 E3S에는 아직 지원하지 않는 편집 형식이 있습니다. CUI epScript가 MainFile인 프로젝트인지 확인해 주세요.";
  }
  if (error.startsWith("e3s_import_harness_failed:")) {
    return "가져오기 중 생성된 자료를 안전하게 정리하거나 복원하지 못했습니다. 오류 상세에 표시된 파일을 사용하는 프로그램을 닫고 대상 폴더 상태를 확인한 뒤 다시 시도해 주세요. 원본 E3S는 변경하지 않았습니다.";
  }
  return "E3S를 가져오지 못했습니다. 오류 상세에서 실패 원인을 확인한 뒤 다시 시도해 주세요.";
}

export function E3sImportDialog({
  open,
  onOpenChange,
  pickSource,
  pickDestination,
  importProject,
  onImported,
}: E3sImportDialogProps) {
  const [source, setSource] = useState<E3sSourceSelection | null>(null);
  const [destination, setDestination] = useState<E3sDestinationSelection | null>(null);
  const [activeTask, setActiveTask] = useState<"source" | "destination" | "import" | null>(null);
  const [failure, setFailure] = useState<{
    message: string;
    detail?: string;
  } | null>(null);
  const [importIssues, setImportIssues] = useState<HarnessImportIssue[] | null>(null);
  const [detailsExpanded, setDetailsExpanded] = useState(false);
  const contentRef = useRef<HTMLDivElement>(null);
  const reviewRegionRef = useRef<HTMLDivElement>(null);
  const busy = activeTask !== null;
  const ready = source !== null && destination?.empty === true;
  const reviewActive = importIssues !== null;
  const reviewItems = importIssues ?? [];

  useEffect(() => {
    if (failure === null && importIssues === null) return;
    if (contentRef.current === null) return;
    const content = contentRef.current;
    content.scrollTop = content.scrollHeight;
  }, [failure, importIssues, detailsExpanded]);
  useEffect(() => {
    if (importIssues !== null && !busy) {
      reviewRegionRef.current?.focus({ preventScroll: true });
    }
  }, [busy, importIssues]);

  const reset = () => {
    setSource(null);
    setDestination(null);
    setActiveTask(null);
    setFailure(null);
    setImportIssues(null);
    setDetailsExpanded(false);
  };

  const changeOpen = (nextOpen: boolean) => {
    if (!nextOpen && busy) return;
    if (!nextOpen) reset();
    onOpenChange(nextOpen);
  };

  const selectSource = async () => {
    setActiveTask("source");
    setFailure(null);
    setImportIssues(null);
    setDetailsExpanded(false);
    try {
      const selected = await pickSource();
      if (selected !== null) setSource(selected);
    } catch {
      setFailure({ message: "E3S 파일 선택 창을 열지 못했습니다. 잠시 후 다시 시도해 주세요." });
    } finally {
      setActiveTask(null);
    }
  };

  const selectDestination = async () => {
    setActiveTask("destination");
    setFailure(null);
    setImportIssues(null);
    setDetailsExpanded(false);
    try {
      const selected = await pickDestination();
      if (selected !== null) {
        setDestination(selected);
        if (!selected.empty) {
          setFailure({ message: "선택한 작업 폴더가 비어 있지 않습니다. 새 빈 폴더를 만든 뒤 다시 선택해 주세요." });
        }
      }
    } catch {
      setFailure({ message: "작업 폴더 선택 창을 열지 못했습니다. 잠시 후 다시 시도해 주세요." });
    } finally {
      setActiveTask(null);
    }
  };

  const runImport = async (excludedImportItems?: readonly string[]) => {
    if (source === null || destination?.empty !== true) return;
    setActiveTask("import");
    setFailure(null);
    setImportIssues(null);
    setDetailsExpanded(false);
    try {
      const request: E3sImportRequest =
        excludedImportItems === undefined
          ? { sourceE3s: source.path, destination: destination.path }
          : {
              sourceE3s: source.path,
              destination: destination.path,
              excludedImportItems,
            };
      const setup = await importProject(request);
      if (setup.error) {
        setFailure({
          message: importFailureText(setup.error),
          detail: setup.error,
        });
        return;
      }
      if (setup.importIssues !== undefined && setup.importIssues.length > 0) {
        setImportIssues(setup.importIssues);
        return;
      }
      if (!setup.projectOpened) {
        setFailure({
          message: "가져오기가 완료되지 않았습니다. 프로젝트를 활성화하지 않았습니다.",
          detail: "setup_import_e3s returned projectOpened=false without importIssues",
        });
        return;
      }
      onImported(setup);
      reset();
      onOpenChange(false);
    } catch (error) {
      setFailure({
        message: "가져오기 명령을 실행하지 못했습니다. 오류 상세를 확인하고 앱을 다시 연 뒤 시도해 주세요.",
        detail: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setActiveTask(null);
    }
  };

  const sourceState = source === null ? "current" : "complete";
  const destinationState =
    source === null
      ? "pending"
      : destination === null
        ? "current"
        : destination.empty
          ? "complete"
          : "invalid";
  const reviewState = importIssues === null ? "pending" : "current";
  return (
    <Dialog open={open} onOpenChange={changeOpen}>
      <DialogContent
        aria-busy={busy}
        closeLabel="닫기"
        className="max-h-[calc(100dvh-2rem)] flex flex-col gap-0 overflow-hidden p-0 sm:max-w-xl [&>button]:z-20"
        onEscapeKeyDown={(event) => {
          if (busy) event.preventDefault();
        }}
        onInteractOutside={(event) => {
          if (busy) event.preventDefault();
        }}
      >
        <DialogHeader className="relative z-10 shrink-0 border-b border-border bg-background px-6 py-5 text-left">
          <DialogTitle className="flex items-center gap-2">
            <FileInputIcon aria-hidden className="size-4 text-primary" />
            E3S 프로젝트 가져오기
          </DialogTitle>
          <DialogDescription className="break-keep leading-6">
            EUD Editor 3에서 작업하던 .e3s 프로젝트를 가져옵니다. 가져온 뒤에는 EUD Editor 3 실행 없이 여기서 편집하고 빌드할 수 있습니다.
          </DialogDescription>
        </DialogHeader>

        <div ref={contentRef} className="relative z-0 min-h-0 flex-1 overflow-y-auto px-4 py-4 sm:px-6">
          <div className="break-keep rounded-lg border border-border bg-muted/40 p-3 text-sm leading-6 text-muted-foreground">
            <p className="font-medium text-foreground">원본 E3S와 맵 파일은 변경하지 않습니다.</p>
            <p className="mt-1">새로 만든 빈 폴더에 별도 프로젝트로 가져옵니다. 이후 작업은 이곳에 저장되며, 원본 E3S에 자동으로 반영되지 않습니다.</p>
            <p className="mt-2">승인 문서·프로젝트 메모리·위키 같은 내구성 있는 자료는 새 프로젝트의 <code>.eud-agent</code>에 저장합니다. 대화와 실행 중인 작업은 이 PC의 AppData에 남아 이동하지 않습니다. 원본 하네스는 보존합니다.</p>
            <p className="mt-2 text-xs leading-5">CUI epScript가 MainFile인 프로젝트를 지원합니다. GUI·RawText 등 지원하지 않는 형식이 포함되면 가져올 수 없습니다.</p>
          </div>

          <ol className="mt-4 grid gap-3">
            <ImportStep
              number={1}
              title="E3S 파일 선택"
              description="기존 프로젝트의 .e3s 파일을 선택합니다. E3S가 참조하는 원본 SCX/SCM도 같은 폴더에 두세요."
              state={sourceState}
            >
              <Button
                type="button"
                variant={source === null ? "default" : "outline"}
                className="min-h-11"
                disabled={busy}
                onClick={() => void selectSource()}
              >
                {activeTask === "source" ? (
                  <Loader2Icon aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
                ) : (
                  <FileInputIcon aria-hidden className="size-4" />
                )}
                {source === null ? "E3S 파일 선택" : "다른 E3S 파일 선택"}
              </Button>
              {source !== null && (
                <p className="mt-2 break-all font-mono text-xs text-muted-foreground">{formatPathForDisplay(source.path)}</p>
              )}
            </ImportStep>

            <ImportStep
              number={2}
              title="새 작업 폴더 선택"
              description="선택 창에서 새 폴더를 만든 뒤 그 빈 폴더를 선택합니다. 가져온 프로젝트와 이후 작업이 이곳에 저장됩니다."
              state={destinationState}
            >
              <Button
                type="button"
                variant={destination === null ? "default" : "outline"}
                className="min-h-11"
                disabled={busy || source === null}
                onClick={() => void selectDestination()}
              >
                {activeTask === "destination" ? (
                  <Loader2Icon aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
                ) : (
                  <FolderOpenIcon aria-hidden className="size-4" />
                )}
                {destination === null ? "새 작업 폴더 선택" : "다른 작업 폴더 선택"}
              </Button>
              {destination !== null && (
                <p className="mt-2 break-all font-mono text-xs text-muted-foreground">{formatPathForDisplay(destination.path)}</p>
              )}
            </ImportStep>

            <ImportStep
              number={3}
              title="가져올 부가 문서 확인"
              description={
                reviewActive
                  ? "가져오지 못한 선택 항목의 경로와 사유를 확인한 뒤, 건강한 나머지 자료를 가져올지 결정해 주세요."
                  : "가져오기를 시작하면 선택적으로 연결된 하네스 자료를 안전하게 확인합니다."
              }
              state={reviewState}
            >
              {reviewActive ? (
                <div
                  ref={reviewRegionRef}
                  tabIndex={-1}
                  aria-label="가져오지 못한 부가 항목 검토"
                  aria-live="polite"
                  className="rounded-md border border-border bg-muted/40 p-3 text-sm leading-6 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                >
                  <p className="font-medium text-foreground">가져오지 못한 부가 항목 {reviewItems.length}개</p>
                  <ul
                    className="mt-2 max-h-56 space-y-2 overflow-y-auto pr-1"
                    aria-label="가져오지 못한 부가 항목 목록"
                  >
                    {reviewItems.map((issue) => (
                      <li key={issue.id} className="rounded border border-border/70 bg-background/70 p-2">
                        <p className="break-all font-mono text-xs text-foreground">
                          <span className="font-sans text-muted-foreground">
                            {issue.scope === "workspace"
                              ? "작업 문서"
                              : issue.scope === "memory"
                                ? "프로젝트 메모리·위키"
                                : "대화 세션"}
                            {" · "}
                          </span>
                          {issue.path}
                        </p>
                        <p className="mt-1 break-keep text-sm text-foreground">
                          {formatImportIssueReason(issue.reason)}
                        </p>
                        <details className="mt-1 text-xs text-muted-foreground">
                          <summary className="cursor-pointer rounded-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
                            원본 사유 보기
                          </summary>
                          <p className="mt-1 break-all font-mono">{issue.reason}</p>
                        </details>
                      </li>
                    ))}
                  </ul>
                  <p className="mt-3 text-muted-foreground">
                    검토한 항목만 새 프로젝트에 포함하지 않습니다. 건강하게 읽을 수 있는 나머지 자료는 가져오며, 원본은 변경하지 않습니다.
                  </p>
                  <p className="mt-2 font-medium text-foreground">이 항목들을 제외하고 가져올까요?</p>
                </div>
              ) : (
                <p className="text-sm text-muted-foreground">가져오지 못한 선택 항목이 발견되면 이 단계에서 경로와 사유를 확인하고 동의할 수 있습니다.</p>
              )}
            </ImportStep>
          </ol>

          {failure !== null && (
            <div role="alert" className="mt-4 flex break-keep gap-2 rounded-lg border border-destructive/40 bg-destructive/10 p-3 text-sm leading-6 text-destructive">
              <CircleAlertIcon aria-hidden className="mt-1 size-4 shrink-0" />
              <div className="min-w-0 flex-1">
                <p>{failure.message}</p>
                {failure.detail && (
                  <details
                    className="mt-2"
                    open={detailsExpanded}
                    onToggle={(event) => setDetailsExpanded(event.currentTarget.open)}
                  >
                    <summary className="cursor-pointer rounded-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">오류 상세</summary>
                    <pre className="mt-2 whitespace-pre-wrap break-all font-mono text-xs">{failure.detail}</pre>
                  </details>
                )}
              </div>
            </div>
          )}
        </div>

        <DialogFooter className="relative z-10 shrink-0 flex-wrap gap-2 border-t border-border bg-background px-4 py-4 sm:px-6">
          <Button type="button" variant="ghost" className="min-h-11" disabled={busy} onClick={() => changeOpen(false)}>
            취소
          </Button>
          {reviewActive ? (
            <>
              <Button type="button" variant="outline" className="min-h-11" disabled={busy} onClick={() => void runImport()}>
                {activeTask === "import" && <Loader2Icon aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />}
                {activeTask === "import" ? "다시 확인하는 중…" : "다시 확인"}
              </Button>
              <Button
                type="button"
                className="min-h-11"
                disabled={busy}
                onClick={() => void runImport(reviewItems.map((issue) => issue.id))}
              >
                {activeTask === "import" && <Loader2Icon aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />}
                {activeTask === "import" ? "가져오는 중…" : `검토한 ${reviewItems.length}개를 제외하고 가져오기`}
              </Button>
            </>
          ) : (
            <Button type="button" className="min-h-11" disabled={!ready || busy} onClick={() => void runImport()}>
              {activeTask === "import" && <Loader2Icon aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />}
              {activeTask === "import" ? "가져오는 중…" : "선택한 위치로 가져오기"}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
