import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  CircleAlertIcon,
  FileInputIcon,
  FolderOpenIcon,
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
    return i18n.t("setup.e3s.failMapMissing");
  }
  if (error.startsWith("e3s_import_destination_not_empty:")) {
    return i18n.t("setup.e3s.notEmpty");
  }
  if (error.startsWith("e3s_import_unsupported:")) {
    return i18n.t("setup.e3s.failUnsupported");
  }
  if (error.startsWith("e3s_import_harness_failed:")) {
    return i18n.t("setup.e3s.failHarness");
  }
  return i18n.t("setup.e3s.failGeneric");
}

export function E3sImportDialog({
  open,
  onOpenChange,
  pickSource,
  pickDestination,
  importProject,
  onImported,
}: E3sImportDialogProps) {
  const { t } = useTranslation();
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
      setFailure({ message: t("setup.e3s.pickSourceFailed") });
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
          setFailure({ message: t("setup.e3s.notEmpty") });
        }
      }
    } catch {
      setFailure({ message: t("setup.e3s.pickDestFailed") });
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
          message: t("setup.e3s.incomplete"),
          detail: "setup_import_e3s returned projectOpened=false without importIssues",
        });
        return;
      }
      onImported(setup);
      reset();
      onOpenChange(false);
    } catch (error) {
      setFailure({
        message: t("setup.e3s.commandFailed"),
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
        closeLabel={t("setup.e3s.close")}
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
            {t("setup.e3s.title")}
          </DialogTitle>
          <DialogDescription className="break-keep leading-6">
            {t("setup.e3s.description")}
          </DialogDescription>
        </DialogHeader>

        <div ref={contentRef} className="relative z-0 min-h-0 flex-1 overflow-y-auto px-4 py-4 sm:px-6">
          <div className="break-keep rounded-lg border border-border bg-muted/40 p-3 text-sm leading-6 text-muted-foreground">
            <p className="font-medium text-foreground">{t("setup.e3s.noteStrong")}</p>
            <p className="mt-1">{t("setup.e3s.note1")}</p>
            <p className="mt-2">{t("setup.e3s.note2Before")}<code>.eud-agent</code>{t("setup.e3s.note2After")}</p>
            <p className="mt-2 text-xs leading-5">{t("setup.e3s.note3")}</p>
          </div>

          <ol className="mt-4 grid gap-3">
            <ImportStep
              number={1}
              title={t("setup.e3s.step1")}
              description={t("setup.e3s.step1Desc")}
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
                {source === null ? t("setup.e3s.chooseSource") : t("setup.e3s.chooseOtherSource")}
              </Button>
              {source !== null && (
                <p className="mt-2 break-all font-mono text-xs text-muted-foreground">{formatPathForDisplay(source.path)}</p>
              )}
            </ImportStep>

            <ImportStep
              number={2}
              title={t("setup.e3s.step2")}
              description={t("setup.e3s.step2Desc")}
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
                {destination === null ? t("setup.e3s.chooseDest") : t("setup.e3s.chooseOtherDest")}
              </Button>
              {destination !== null && (
                <p className="mt-2 break-all font-mono text-xs text-muted-foreground">{formatPathForDisplay(destination.path)}</p>
              )}
            </ImportStep>

            <ImportStep
              number={3}
              title={t("setup.e3s.step3")}
              description={
                reviewActive
                  ? t("setup.e3s.step3Review")
                  : t("setup.e3s.step3Idle")
              }
              state={reviewState}
            >
              {reviewActive ? (
                <div
                  ref={reviewRegionRef}
                  tabIndex={-1}
                  aria-label={t("setup.e3s.reviewLabel")}
                  aria-live="polite"
                  className="rounded-md border border-border bg-muted/40 p-3 text-sm leading-6 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                >
                  <p className="font-medium text-foreground">{t("setup.e3s.reviewCount", { count: reviewItems.length })}</p>
                  <ul
                    className="mt-2 max-h-56 space-y-2 overflow-y-auto pr-1"
                    aria-label={t("setup.e3s.reviewList")}
                  >
                    {reviewItems.map((issue) => (
                      <li key={issue.id} className="rounded border border-border/70 bg-background/70 p-2">
                        <p className="break-all font-mono text-xs text-foreground">
                          <span className="font-sans text-muted-foreground">
                            {issue.scope === "workspace"
                              ? t("setup.e3s.scopeWorkspace")
                              : issue.scope === "memory"
                                ? t("setup.e3s.scopeMemory")
                                : t("setup.e3s.scopeSession")}
                            {" · "}
                          </span>
                          {issue.path}
                        </p>
                        <p className="mt-1 break-keep text-sm text-foreground">
                          {formatImportIssueReason(issue.reason)}
                        </p>
                        <details className="mt-1 text-xs text-muted-foreground">
                          <summary className="cursor-pointer rounded-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
                            {t("setup.e3s.showReason")}
                          </summary>
                          <p className="mt-1 break-all font-mono">{issue.reason}</p>
                        </details>
                      </li>
                    ))}
                  </ul>
                  <p className="mt-3 text-muted-foreground">
                    {t("setup.e3s.reviewNote")}
                  </p>
                  <p className="mt-2 font-medium text-foreground">{t("setup.e3s.reviewAsk")}</p>
                </div>
              ) : (
                <p className="text-sm text-muted-foreground">{t("setup.e3s.reviewIdle")}</p>
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
                    <summary className="cursor-pointer rounded-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">{t("setup.e3s.details")}</summary>
                    <pre className="mt-2 whitespace-pre-wrap break-all font-mono text-xs">{failure.detail}</pre>
                  </details>
                )}
              </div>
            </div>
          )}
        </div>

        <DialogFooter className="relative z-10 shrink-0 flex-wrap gap-2 border-t border-border bg-background px-4 py-4 sm:px-6">
          <Button type="button" variant="ghost" className="min-h-11" disabled={busy} onClick={() => changeOpen(false)}>
            {t("setup.e3s.cancel")}
          </Button>
          {reviewActive ? (
            <>
              <Button type="button" variant="outline" className="min-h-11" disabled={busy} onClick={() => void runImport()}>
                {activeTask === "import" && <Loader2Icon aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />}
                {activeTask === "import" ? t("setup.e3s.rechecking") : t("setup.e3s.recheck")}
              </Button>
              <Button
                type="button"
                className="min-h-11"
                disabled={busy}
                onClick={() => void runImport(reviewItems.map((issue) => issue.id))}
              >
                {activeTask === "import" && <Loader2Icon aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />}
                {activeTask === "import" ? t("setup.e3s.importing") : t("setup.e3s.importExcluding", { count: reviewItems.length })}
              </Button>
            </>
          ) : (
            <Button type="button" className="min-h-11" disabled={!ready || busy} onClick={() => void runImport()}>
              {activeTask === "import" && <Loader2Icon aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />}
              {activeTask === "import" ? t("setup.e3s.importing") : t("setup.e3s.start")}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
