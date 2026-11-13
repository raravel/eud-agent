import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  CircleAlertIcon,
  FileInputIcon,
  FolderOpenIcon,
  LinkIcon,
  Loader2Icon,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import i18n from "@/lib/i18n";
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
      return i18n.t("setup.e2.recoverVersion");
    case "malformed_input":
      return i18n.t("setup.e2.recoverMalformed");
    case "active_scdb_unsupported":
      return i18n.t("setup.e2.recoverScdb");
    case "active_grp_unsupported":
      return i18n.t("setup.e2.recoverGrp");
    case "active_chatwrite_unverified":
      return i18n.t("setup.e2.recoverChatWrite");
    case "active_definition_unknown":
      return i18n.t("setup.e2.recoverDefinition");
    case "source_changed":
      return i18n.t("setup.e2.recoverChanged");
    default:
      return i18n.t("setup.e2.recoverDefault");
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
  const { t } = useTranslation();
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
  const importAttemptRef = useRef(0);
  const activeAttemptRef = useRef<number | null>(null);
  const terminalAttemptRef = useRef<number | null>(null);
  const cancelRequestedRef = useRef(false);
  const importPendingRef = useRef(false);

  const busy = activeTask !== null;
  const preflightReady = fingerprint !== null && needs.length === 0;

  useEffect(() => {
    if (failure === null) return;
    contentRef.current?.scrollTo({ top: contentRef.current.scrollHeight });
  }, [failure, detailsExpanded]);

  const invalidateImportAttempt = () => {
    importAttemptRef.current += 1;
    activeAttemptRef.current = null;
    terminalAttemptRef.current = null;
    identityRef.current = null;
    cancelRequestedRef.current = false;
    importPendingRef.current = false;
  };

  const reset = () => {
    invalidateImportAttempt();
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

  const processResult = (result: E2ImportResult, expectedFingerprint: string, attempt: number, directInvoke = false) => {
    if (activeAttemptRef.current !== attempt || terminalAttemptRef.current === attempt) return;
    if (result.state === "fatal" && result.fingerprint === undefined && directInvoke) {
      terminalAttemptRef.current = attempt;
      importPendingRef.current = false;
      setProgress(null);
      showIssues(result.issues);
      setActiveTask(null);
      return;
    }
    if (result.fingerprint !== expectedFingerprint) return;
    const activeIdentity = identityRef.current;
    if (activeIdentity !== null && !isE2ImportResultFor(result, activeIdentity)) return;

    if (result.state === "progress") {
      if (activeIdentity === null) {
        identityRef.current = { importId: result.importId, fingerprint: result.fingerprint };
      }
      setProgress(result);
      if (cancelRequestedRef.current && importPendingRef.current) {
        void requestCancel({ importId: result.importId }, expectedFingerprint, attempt);
      }
      return;
    }

    if (activeIdentity === null) {
      const importId = result.importId;
      const resultFingerprint = result.fingerprint;
      if (importId === undefined || resultFingerprint === undefined) return;
      identityRef.current = { importId, fingerprint: resultFingerprint };
    }
    terminalAttemptRef.current = attempt;
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
      setFailure({ message: t("setup.e2.cancelled") });
    }
    setActiveTask(null);
  };

  const requestCancel = async (request: E2CancelRequest, expectedFingerprint: string, attempt: number) => {
    if (!importPendingRef.current || activeAttemptRef.current !== attempt || terminalAttemptRef.current === attempt) return;
    setActiveTask("cancel");
    try {
      const result = decodeE2ImportResult(await cancelImport(request), "setup_cancel_e2_import");
      processResult(result, expectedFingerprint, attempt);
    } catch (error) {
      if (activeAttemptRef.current !== attempt || terminalAttemptRef.current === attempt) return;
      setFailure({
        message: t("setup.e2.cancelFailed"),
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
        message: t("setup.e2.preflightFailed"),
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
        invalidateImportAttempt();
        setSource(selected);
        setDestination(null);
        setRelinks([]);
        setNeeds([]);
        setFingerprint(null);
      }
    } catch (error) {
      setFailure({
        message: t("setup.e2.pickSourceFailed"),
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
        setFailure({ message: t("setup.e2.notEmpty") });
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
        message: t("setup.e2.destinationFailed"),
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
        message: t("setup.e2.relinkFailed"),
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
    const attempt = importAttemptRef.current + 1;
    importAttemptRef.current = attempt;
    activeAttemptRef.current = attempt;
    terminalAttemptRef.current = null;
    cancelRequestedRef.current = false;
    identityRef.current = null;
    importPendingRef.current = true;
    let unlisten: (() => void) | undefined;
    try {
      unlisten = await subscribe((message) => processResult(message.result, fingerprint, attempt));
      const result = decodeE2ImportResult(
        await importProject({ sourcePath: source, destinationPath: destination.path, fingerprint, relinks }),
      );
      processResult(result, fingerprint, attempt, true);
    } catch (error) {
      if (activeAttemptRef.current === attempt && terminalAttemptRef.current !== attempt) {
        importPendingRef.current = false;
        setFailure({
          message: t("setup.e2.importFailed"),
          detail: error instanceof Error ? error.message : String(error),
        });
      }
    } finally {
      unlisten?.();
      if (activeAttemptRef.current === attempt && terminalAttemptRef.current !== attempt) setActiveTask(null);
    }
  };

  const cancel = () => {
    const attempt = activeAttemptRef.current;
    if (!importPendingRef.current || identityRef.current === null || attempt === null) return;
    cancelRequestedRef.current = true;
    setCancelRequested(true);
    void requestCancel({ importId: identityRef.current.importId }, identityRef.current.fingerprint, attempt);
  };

  const sourceState = source === null ? "current" : "complete";
  const destinationState = source === null ? "pending" : destination === null ? "current" : destination.empty ? "complete" : "invalid";
  const relinkState = needs.length > 0 ? "current" : preflightReady ? "complete" : "pending";

  return (
    <Dialog open={open} onOpenChange={changeOpen}>
      <DialogContent
        aria-busy={busy || importPendingRef.current}
        closeLabel={t("setup.e2.close")}
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
            {t("setup.e2.title")}
          </DialogTitle>
          <DialogDescription className="break-keep leading-6">
            {t("setup.e2.description")}
          </DialogDescription>
        </DialogHeader>

        <div ref={contentRef} className="min-h-0 flex-1 overflow-y-auto px-4 py-4 sm:px-6">
          <div className="break-keep rounded-lg border border-border bg-muted/40 p-3 text-sm leading-6 text-muted-foreground">
            {t("setup.e2.note")}
          </div>

          <ol className="mt-4 grid gap-3">
            <ImportStep number={1} title={t("setup.e2.step1")} description={t("setup.e2.step1Desc")} state={sourceState}>
              <Button type="button" variant={source === null ? "default" : "outline"} className="min-h-11" disabled={busy || importPendingRef.current} onClick={() => void selectSource()}>
                {activeTask === "source" ? <Loader2Icon aria-hidden className="size-4 animate-spin motion-reduce:animate-none" /> : <FileInputIcon aria-hidden className="size-4" />}
                {source === null ? t("setup.e2.chooseSource") : t("setup.e2.chooseOtherSource")}
              </Button>
              {source !== null && <p className="mt-2 break-all font-mono text-xs text-muted-foreground">{formatPathForDisplay(source)}</p>}
            </ImportStep>

            <ImportStep number={2} title={t("setup.e2.step2")} description={t("setup.e2.step2Desc")} state={destinationState}>
              <Button type="button" variant={destination === null ? "default" : "outline"} className="min-h-11" disabled={busy || importPendingRef.current || source === null} onClick={() => void selectDestination()}>
                {activeTask === "destination" ? <Loader2Icon aria-hidden className="size-4 animate-spin motion-reduce:animate-none" /> : <FolderOpenIcon aria-hidden className="size-4" />}
                {destination === null ? t("setup.e2.chooseDest") : t("setup.e2.chooseOtherDest")}
              </Button>
              {destination !== null && <p className="mt-2 break-all font-mono text-xs text-muted-foreground">{formatPathForDisplay(destination.path)}</p>}
            </ImportStep>

            <ImportStep number={3} title={t("setup.e2.step3")} description={t("setup.e2.step3Desc")} state={relinkState}>
              {needs.length > 0 ? (
                <ul className="grid gap-2" aria-label={t("setup.e2.relinkList")}>
                  {needs.map((need) => {
                    const choice = relinks.find((item) => item.needId === need.id);
                    const label = need.kind === "inputMap" ? t("setup.e2.relinkMap") : t("setup.e2.relinkResource");
                    return (
                      <li key={need.id} className="rounded-md border border-border bg-muted/30 p-3">
                        <p className="font-medium text-sm text-foreground">{need.label}</p>
                        {need.originalPath !== undefined && <p className="mt-1 break-all font-mono text-xs text-muted-foreground">{need.originalPath}</p>}
                        <Button type="button" variant="outline" className="mt-2 min-h-11" disabled={busy || importPendingRef.current} onClick={() => void selectRelink(need)}>
                          {activeTask === "relink" ? <Loader2Icon aria-hidden className="size-4 animate-spin motion-reduce:animate-none" /> : <LinkIcon aria-hidden className="size-4" />}
                          {choice === undefined ? label : t("setup.e2.relinkChange", { label })}
                        </Button>
                        {choice !== undefined && <p className="mt-2 break-all font-mono text-xs text-muted-foreground">{formatPathForDisplay(choice.path)}</p>}
                      </li>
                    );
                  })}
                </ul>
              ) : preflightReady ? <p className="text-sm text-muted-foreground">{t("setup.e2.linksOk")}</p> : <p className="text-sm text-muted-foreground">{t("setup.e2.linksPending")}</p>}
            </ImportStep>
          </ol>

          {progress !== null && (
            <div role="status" aria-live="polite" className="mt-4 rounded-md border border-border bg-muted/40 p-3 text-sm leading-6">
              <p className="font-medium text-foreground">{t("setup.e2.progress", { message: progress.message })}</p>
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
                    <summary className="cursor-pointer rounded-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">{t("setup.e2.details")}</summary>
                    <pre className="mt-2 whitespace-pre-wrap break-all font-mono text-xs">{failure.detail}</pre>
                  </details>
                )}
              </div>
            </div>
          )}
        </div>

        <DialogFooter className="relative z-10 shrink-0 flex-wrap gap-2 border-t border-border bg-background px-4 py-4 sm:px-6">
          <Button type="button" variant="ghost" className="min-h-11" disabled={busy || importPendingRef.current} onClick={() => changeOpen(false)}>{t("setup.e2.cancel")}</Button>
          {importPendingRef.current && identityRef.current !== null && (
            <Button type="button" variant="outline" className="min-h-11" disabled={activeTask === "cancel" || cancelRequested} onClick={cancel}>
              {activeTask === "cancel" ? <Loader2Icon aria-hidden className="size-4 animate-spin motion-reduce:animate-none" /> : null}
              {cancelRequested ? t("setup.e2.cancelling") : t("setup.e2.cancelImport")}
            </Button>
          )}
          <Button type="button" className="min-h-11" disabled={!preflightReady || busy || importPendingRef.current} onClick={() => void startImport()}>
            {activeTask === "import" ? <Loader2Icon aria-hidden className="size-4 animate-spin motion-reduce:animate-none" /> : null}
            {activeTask === "import" ? t("setup.e2.importing") : t("setup.e2.start")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
