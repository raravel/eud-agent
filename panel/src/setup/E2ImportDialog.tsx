import { useEffect, useRef, useState } from "react";
import {
  CircleAlertIcon,
  FileInputIcon,
  FolderOpenIcon,
  LinkIcon,
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
import {
  decodeE2ImportResult,
  isE2ImportResultFor,
  type E2CancelRequest,
  type E2ImportCompleted,
  type E2ImportIdentity,
  type E2ImportProgress,
  type E2ImportRequest,
  type E2ImportResult,
  type E2PreflightRequest,
  type E2PreflightResult,
  type E2RelinkChoice,
  type E2RelinkNeed,
  type E3sDestinationSelection,
} from "@/lib/projectImport";
import type { E2ImportEventMessage } from "@/lib/protocol";
import { formatPathForDisplay } from "@/lib/utils";
import { ImportStep } from "@/setup/ImportStep";

export interface E2ImportDialogProps {
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
  readonly pickSource: () => Promise<string | null>;
  readonly pickDestination: () => Promise<E3sDestinationSelection | null>;
  readonly pickRelink: (need: E2RelinkNeed) => Promise<string | null>;
  readonly preflight: (request: E2PreflightRequest) => Promise<E2PreflightResult>;
  readonly importProject: (request: E2ImportRequest) => Promise<unknown>;
  readonly cancelImport: (request: E2CancelRequest) => Promise<unknown>;
  readonly subscribe: (
    onMessage: (message: E2ImportEventMessage) => void,
  ) => Promise<() => void> | (() => void);
  readonly onImported: (result: E2ImportCompleted) => void;
}

type ActiveTask = "source" | "destination" | "preflight" | "relink" | "import" | "cancel" | null;

function issueRecovery(issue: { code: string }): string {
  switch (issue.code) {
    case "unsupported_version":
      return "지원하는 EUD Editor 2 버전(0.17.9.7)에서 만든 E2S/E2P인지 확인한 뒤 다시 선택해 주세요.";
    case "malformed_input":
      return "파일 구조를 읽을 수 없습니다. 원본 E2S/E2P가 손상되지 않았는지 확인한 뒤 다시 선택해 주세요.";
    case "active_scdb_unsupported":
      return "활성 SCDB 데이터는 현재 안전하게 가져올 수 없습니다. 원본 프로젝트에서 SCDB 사용을 정리한 뒤 다시 시도해 주세요.";
    case "active_grp_unsupported":
      return "실제로 사용 중인 GRP 리소스는 현재 안전하게 가져올 수 없습니다. 원본 프로젝트에서 해당 리소스를 정리한 뒤 다시 시도해 주세요.";
    case "active_chatwrite_unverified":
      return "ChatWrite 동작을 안전하게 검증하지 못했습니다. 원본 프로젝트에서 ChatWrite 사용을 확인한 뒤 다시 시도해 주세요.";
    case "active_definition_unknown":
      return "알 수 없는 활성 정의를 안전하게 변환할 수 없습니다. 원본 프로젝트의 해당 정의를 확인한 뒤 다시 시도해 주세요.";
    case "source_changed":
      return "파일 또는 연결 경로가 확인 중 변경되었습니다. 원본과 연결 파일을 다시 확인한 뒤 처음부터 진행해 주세요.";
    default:
      return "안전하게 가져올 수 없는 항목이 있습니다. 오류 상세의 원인을 확인한 뒤 원본 프로젝트를 정리하고 다시 시도해 주세요.";
  }
}

function issueDetails(issues: readonly { code: string; message: string; path?: string }[]): string {
  return issues
    .map((issue) => `${issue.code}: ${issue.message}${issue.path === undefined ? "" : ` (${issue.path})`}`)
    .join("\n");
}

export function E2ImportDialog({
  open,
  onOpenChange,
  pickSource,
  pickDestination,
  pickRelink,
  preflight,
  importProject,
  cancelImport,
  subscribe,
  onImported,
}: E2ImportDialogProps) {
  const [source, setSource] = useState<string | null>(null);
  const [destination, setDestination] = useState<E3sDestinationSelection | null>(null);
  const [relinks, setRelinks] = useState<readonly E2RelinkChoice[]>([]);
  const [needs, setNeeds] = useState<readonly E2RelinkNeed[]>([]);
  const [fingerprint, setFingerprint] = useState<string | null>(null);
  const [activeTask, setActiveTask] = useState<ActiveTask>(null);
  const [failure, setFailure] = useState<{ message: string; detail?: string } | null>(null);
  const [progress, setProgress] = useState<E2ImportProgress | null>(null);
  const [cancelRequested, setCancelRequested] = useState(false);
  const [detailsExpanded, setDetailsExpanded] = useState(false);
  const contentRef = useRef<HTMLDivElement>(null);
  const identityRef = useRef<E2ImportIdentity | null>(null);
  const terminalHandledRef = useRef(false);
  const cancelRequestedRef = useRef(false);
  const importPendingRef = useRef(false);

  const busy = activeTask !== null;
  const preflightReady = fingerprint !== null && needs.length === 0;

  useEffect(() => {
    if (failure === null) return;
    contentRef.current?.scrollTo({ top: contentRef.current.scrollHeight });
  }, [failure, detailsExpanded]);

  const reset = () => {
    identityRef.current = null;
    terminalHandledRef.current = false;
    cancelRequestedRef.current = false;
    importPendingRef.current = false;
    setSource(null);
    setDestination(null);
    setRelinks([]);
    setNeeds([]);
    setFingerprint(null);
    setActiveTask(null);
    setFailure(null);
    setProgress(null);
    setCancelRequested(false);
    setDetailsExpanded(false);
  };

  const changeOpen = (nextOpen: boolean) => {
    if (!nextOpen && (busy || importPendingRef.current)) return;
    if (!nextOpen) reset();
    onOpenChange(nextOpen);
  };

  const showIssues = (issues: readonly { code: string; message: string; path?: string }[]) => {
    setFailure({ message: issueRecovery(issues[0]), detail: issueDetails(issues) });
  };

  const processResult = (result: E2ImportResult, expectedFingerprint: string) => {
    if (terminalHandledRef.current || result.fingerprint !== expectedFingerprint) return;
    const activeIdentity = identityRef.current;
    if (activeIdentity !== null && !isE2ImportResultFor(result, activeIdentity)) return;

    if (result.state === "progress") {
      if (activeIdentity === null) {
        identityRef.current = { importId: result.importId, fingerprint: result.fingerprint };
      }
      setProgress(result);
      if (cancelRequestedRef.current && importPendingRef.current) {
        void requestCancel({ importId: result.importId }, expectedFingerprint);
      }
      return;
    }

    if (activeIdentity === null) {
      const importId = result.importId;
      const resultFingerprint = result.fingerprint;
      if (importId === undefined || resultFingerprint === undefined) return;
      identityRef.current = { importId, fingerprint: resultFingerprint };
    }
    terminalHandledRef.current = true;
    importPendingRef.current = false;
    setProgress(null);
    if (result.state === "completed") {
      onImported(result);
      reset();
      onOpenChange(false);
      return;
    }
    if (result.state === "fatal") {
      showIssues(result.issues);
    } else {
      setFailure({ message: "가져오기를 취소했습니다. 원본 파일과 대상 폴더 상태를 확인한 뒤 다시 시도할 수 있습니다." });
    }
    setActiveTask(null);
  };

  const requestCancel = async (request: E2CancelRequest, expectedFingerprint: string) => {
    if (!importPendingRef.current || terminalHandledRef.current) return;
    setActiveTask("cancel");
    try {
      const result = decodeE2ImportResult(await cancelImport(request), "setup_cancel_e2_import");
      processResult(result, expectedFingerprint);
    } catch (error) {
      setFailure({
        message: "취소 요청을 보내지 못했습니다. 가져오기 진행 상태를 확인해 주세요.",
        detail: error instanceof Error ? error.message : String(error),
      });
      setActiveTask("import");
    }
  };

  const runPreflight = async (nextRelinks: readonly E2RelinkChoice[]) => {
    if (source === null || destination?.empty !== true) return;
    setActiveTask("preflight");
    setFailure(null);
    setFingerprint(null);
    setNeeds([]);
    try {
      const result = await preflight({ sourcePath: source, destinationPath: destination.path, relinks: nextRelinks });
      if (result.state === "ready") {
        setFingerprint(result.fingerprint);
        return;
      }
      if (result.state === "needsRelink") {
        setFingerprint(result.fingerprint);
        setNeeds(result.needs);
        return;
      }
      showIssues(result.issues);
    } catch (error) {
      setFailure({
        message: "가져오기 전 확인을 실행하지 못했습니다. 오류 상세를 확인한 뒤 다시 시도해 주세요.",
        detail: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setActiveTask(null);
    }
  };

  const selectSource = async () => {
    setActiveTask("source");
    setFailure(null);
    try {
      const selected = await pickSource();
      if (selected !== null) {
        setSource(selected);
        setDestination(null);
        setRelinks([]);
        setNeeds([]);
        setFingerprint(null);
      }
    } catch (error) {
      setFailure({
        message: "E2S/E2P 파일 선택 창을 열지 못했습니다. 잠시 후 다시 시도해 주세요.",
        detail: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setActiveTask(null);
    }
  };

  const selectDestination = async () => {
    setActiveTask("destination");
    setFailure(null);
    try {
      const selected = await pickDestination();
      if (selected === null) return;
      setDestination(selected);
      setRelinks([]);
      setNeeds([]);
      setFingerprint(null);
      if (!selected.empty) {
        setFailure({ message: "선택한 작업 폴더가 비어 있지 않습니다. 새 빈 폴더를 만든 뒤 다시 선택해 주세요." });
        return;
      }
      setActiveTask("preflight");
      const result = await preflight({ sourcePath: source ?? "", destinationPath: selected.path, relinks: [] });
      if (result.state === "ready") setFingerprint(result.fingerprint);
      else if (result.state === "needsRelink") {
        setFingerprint(result.fingerprint);
        setNeeds(result.needs);
      } else showIssues(result.issues);
    } catch (error) {
      setFailure({
        message: "작업 폴더 선택 또는 가져오기 전 확인을 실행하지 못했습니다. 오류 상세를 확인한 뒤 다시 시도해 주세요.",
        detail: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setActiveTask(null);
    }
  };

  const selectRelink = async (need: E2RelinkNeed) => {
    setActiveTask("relink");
    setFailure(null);
    try {
      const selected = await pickRelink(need);
      if (selected === null) return;
      const nextRelinks = [...relinks.filter((choice) => choice.needId !== need.id), { needId: need.id, path: selected }];
      setRelinks(nextRelinks);
      await runPreflight(nextRelinks);
    } catch (error) {
      setFailure({
        message: "연결할 파일을 선택하지 못했습니다. 원본 파일 위치를 확인한 뒤 다시 시도해 주세요.",
        detail: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setActiveTask(null);
    }
  };

  const startImport = async () => {
    if (source === null || destination?.empty !== true || fingerprint === null || !preflightReady) return;
    setActiveTask("import");
    setFailure(null);
    setProgress(null);
    setCancelRequested(false);
    cancelRequestedRef.current = false;
    identityRef.current = null;
    terminalHandledRef.current = false;
    importPendingRef.current = true;
    let unlisten: (() => void) | undefined;
    try {
      unlisten = await subscribe((message) => processResult(message.result, fingerprint));
      const result = decodeE2ImportResult(
        await importProject({ sourcePath: source, destinationPath: destination.path, fingerprint, relinks }),
      );
      processResult(result, fingerprint);
    } catch (error) {
      if (!terminalHandledRef.current) {
        importPendingRef.current = false;
        setFailure({
          message: "가져오기 명령을 실행하지 못했습니다. 오류 상세를 확인한 뒤 다시 시도해 주세요.",
          detail: error instanceof Error ? error.message : String(error),
        });
      }
    } finally {
      unlisten?.();
      if (!terminalHandledRef.current) setActiveTask(null);
    }
  };

  const cancel = () => {
    if (!importPendingRef.current || identityRef.current === null) return;
    cancelRequestedRef.current = true;
    setCancelRequested(true);
    void requestCancel({ importId: identityRef.current.importId }, identityRef.current.fingerprint);
  };

  const sourceState = source === null ? "current" : "complete";
  const destinationState = source === null ? "pending" : destination === null ? "current" : destination.empty ? "complete" : "invalid";
  const relinkState = needs.length > 0 ? "current" : preflightReady ? "complete" : "pending";

  return (
    <Dialog open={open} onOpenChange={changeOpen}>
      <DialogContent
        aria-busy={busy || importPendingRef.current}
        closeLabel="닫기"
        className="max-h-[calc(100dvh-2rem)] flex flex-col gap-0 overflow-hidden p-0 sm:max-w-xl [&>button]:z-20"
        onEscapeKeyDown={(event) => {
          if (busy || importPendingRef.current) event.preventDefault();
        }}
        onInteractOutside={(event) => {
          if (busy || importPendingRef.current) event.preventDefault();
        }}
      >
        <DialogHeader className="relative z-10 shrink-0 border-b border-border bg-background px-6 py-5 text-left">
          <DialogTitle className="flex items-center gap-2">
            <FileInputIcon aria-hidden className="size-4 text-primary" />
            E2 프로젝트 가져오기
          </DialogTitle>
          <DialogDescription className="break-keep leading-6">
            EUD Editor 2의 E2S 또는 E2P 프로젝트를 새 작업 폴더로 안전하게 가져옵니다.
          </DialogDescription>
        </DialogHeader>

        <div ref={contentRef} className="min-h-0 flex-1 overflow-y-auto px-4 py-4 sm:px-6">
          <div className="break-keep rounded-lg border border-border bg-muted/40 p-3 text-sm leading-6 text-muted-foreground">
            원본 E2S/E2P와 연결된 파일은 변경하지 않습니다. 지원하지 않는 활성 기능은 건너뛰지 않고 원인을 알려 드립니다.
          </div>

          <ol className="mt-4 grid gap-3">
            <ImportStep number={1} title="E2S/E2P 선택" description="가져올 E2S 파일 또는 E2P 폴더를 선택합니다." state={sourceState}>
              <Button type="button" variant={source === null ? "default" : "outline"} className="min-h-11" disabled={busy || importPendingRef.current} onClick={() => void selectSource()}>
                {activeTask === "source" ? <Loader2Icon aria-hidden className="size-4 animate-spin motion-reduce:animate-none" /> : <FileInputIcon aria-hidden className="size-4" />}
                {source === null ? "E2S/E2P 선택" : "다른 E2S/E2P 선택"}
              </Button>
              {source !== null && <p className="mt-2 break-all font-mono text-xs text-muted-foreground">{formatPathForDisplay(source)}</p>}
            </ImportStep>

            <ImportStep number={2} title="새 작업 폴더 선택" description="비어 있는 새 폴더를 선택합니다. 가져온 프로젝트는 이 폴더에 만들어집니다." state={destinationState}>
              <Button type="button" variant={destination === null ? "default" : "outline"} className="min-h-11" disabled={busy || importPendingRef.current || source === null} onClick={() => void selectDestination()}>
                {activeTask === "destination" ? <Loader2Icon aria-hidden className="size-4 animate-spin motion-reduce:animate-none" /> : <FolderOpenIcon aria-hidden className="size-4" />}
                {destination === null ? "새 작업 폴더 선택" : "다른 작업 폴더 선택"}
              </Button>
              {destination !== null && <p className="mt-2 break-all font-mono text-xs text-muted-foreground">{formatPathForDisplay(destination.path)}</p>}
            </ImportStep>

            <ImportStep number={3} title="연결 파일 확인" description="원본 맵이나 리소스를 찾지 못하면 직접 다시 연결합니다." state={relinkState}>
              {needs.length > 0 ? (
                <ul className="grid gap-2" aria-label="다시 연결할 파일 목록">
                  {needs.map((need) => {
                    const choice = relinks.find((item) => item.needId === need.id);
                    const label = need.kind === "inputMap" ? "원본 맵 다시 연결" : "리소스 다시 연결";
                    return (
                      <li key={need.id} className="rounded-md border border-border bg-muted/30 p-3">
                        <p className="font-medium text-sm text-foreground">{need.label}</p>
                        {need.originalPath !== undefined && <p className="mt-1 break-all font-mono text-xs text-muted-foreground">{need.originalPath}</p>}
                        <Button type="button" variant="outline" className="mt-2 min-h-11" disabled={busy || importPendingRef.current} onClick={() => void selectRelink(need)}>
                          {activeTask === "relink" ? <Loader2Icon aria-hidden className="size-4 animate-spin motion-reduce:animate-none" /> : <LinkIcon aria-hidden className="size-4" />}
                          {choice === undefined ? label : `${label} 변경`}
                        </Button>
                        {choice !== undefined && <p className="mt-2 break-all font-mono text-xs text-muted-foreground">{formatPathForDisplay(choice.path)}</p>}
                      </li>
                    );
                  })}
                </ul>
              ) : preflightReady ? <p className="text-sm text-muted-foreground">연결 파일을 확인했습니다. 가져오기를 시작할 수 있습니다.</p> : <p className="text-sm text-muted-foreground">파일과 작업 폴더를 선택하면 연결 상태를 확인합니다.</p>}
            </ImportStep>
          </ol>

          {progress !== null && (
            <div role="status" aria-live="polite" className="mt-4 rounded-md border border-border bg-muted/40 p-3 text-sm leading-6">
              <p className="font-medium text-foreground">가져오는 중: {progress.message}</p>
              <p className="text-muted-foreground">{progress.completed} / {progress.total} · {progress.phase}</p>
            </div>
          )}

          {failure !== null && (
            <div role="alert" className="mt-4 flex gap-2 rounded-md border border-destructive/40 bg-destructive/10 p-3 text-sm leading-6 text-foreground">
              <CircleAlertIcon aria-hidden className="mt-1 size-4 shrink-0 text-destructive" />
              <div className="min-w-0">
                <p>{failure.message}</p>
                {failure.detail !== undefined && (
                  <details className="mt-2" open={detailsExpanded} onToggle={(event) => setDetailsExpanded(event.currentTarget.open)}>
                    <summary className="cursor-pointer rounded-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">오류 상세</summary>
                    <pre className="mt-2 whitespace-pre-wrap break-all font-mono text-xs">{failure.detail}</pre>
                  </details>
                )}
              </div>
            </div>
          )}
        </div>

        <DialogFooter className="relative z-10 shrink-0 flex-wrap gap-2 border-t border-border bg-background px-4 py-4 sm:px-6">
          <Button type="button" variant="ghost" className="min-h-11" disabled={busy || importPendingRef.current} onClick={() => changeOpen(false)}>취소</Button>
          {importPendingRef.current && identityRef.current !== null && (
            <Button type="button" variant="outline" className="min-h-11" disabled={activeTask === "cancel" || cancelRequested} onClick={cancel}>
              {activeTask === "cancel" ? <Loader2Icon aria-hidden className="size-4 animate-spin motion-reduce:animate-none" /> : null}
              {cancelRequested ? "취소 요청 중…" : "가져오기 취소"}
            </Button>
          )}
          <Button type="button" className="min-h-11" disabled={!preflightReady || busy || importPendingRef.current} onClick={() => void startImport()}>
            {activeTask === "import" ? <Loader2Icon aria-hidden className="size-4 animate-spin motion-reduce:animate-none" /> : null}
            {activeTask === "import" ? "가져오는 중…" : "선택한 위치로 가져오기"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
