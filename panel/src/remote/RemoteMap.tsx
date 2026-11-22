/**
 * The phone's Map Agent: one Map session's conversation and Map chat, its
 * live run, the candidate picture, the change summary, and Apply / Undo /
 * 폐기 / 되돌리기 — the same commands the PC's Map window sends, over the
 * remote transport. Canvas tools (selections, mentions, image and stamp
 * placement, properties) stay on the PC.
 */
import i18n from "@/lib/i18n";
import { useTranslation } from "react-i18next";
import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import {
  ChevronLeft,
  LoaderCircle,
  Map as MapIcon,
  MessageSquare,
  MoreVertical,
  RefreshCw,
  RotateCcw,
  TriangleAlert,
  ZoomIn,
  ZoomOut,
} from "lucide-react";
import { toast } from "sonner";

import { AskCard } from "@/components/AskCard";
import { ConversationLog } from "@/components/ConversationLog";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
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
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { discardAttachment, stageAttachment } from "@/lib/attachments";
import {
  compactSession,
  sessionModelSettingsGet,
  sessionModelSettingsSave,
  type ChatAttachment,
  type ReasoningSelection,
  type SessionModelSettings,
} from "@/lib/ipc";
import type { AskAnswer } from "@/lib/protocol";
import { RESYNC_EVENT, remoteTransport } from "@/lib/remoteTransport";
import { invoke, listen } from "@/lib/transport";
import { panelLogFromConversation } from "@/map/mapConversation";
import type { MapConversationEntry } from "@/map/MapAgentPanel";
import { MapPromptInput, type MapPromptDraft } from "@/map/MapPromptInput";
import type { PendingSteer } from "@/state/store";
import {
  applyUndo,
  candidateApply,
  candidateDiscard,
  candidateRevert,
  mapCancel,
  mapConversationReset,
  mapConversationRewind,
  mapRender,
  mapRunSnapshot,
  mapSessionCreate,
  mapSessionLoad,
  type CandidateStateView,
  type MapBootstrapResponse,
  type MapRunStartedPayload,
} from "@/map/mapProtocol";
import {
  candidateSummary,
  createRemoteMapStore,
  rewindPrefix,
  wholeMapRender,
  type PendingMapAsk,
  type RemoteMapStore,
} from "./remoteMapModel";
import { recoveryNote, invokeResending, type RecoveryHooks } from "./connectionRecovery";
import { saveLog } from "./logWriter";

type CandidateAction =
  | { kind: "apply" }
  | { kind: "undo" }
  | { kind: "discard" }
  | { kind: "revert"; revision: number };

function actionCopy(kind: CandidateAction["kind"]): {
  title: string;
  confirm: string;
  busy: string;
  destructive: boolean;
} {
  return {
    title: i18n.t(`remote.map.action.${kind}.title`),
    confirm: i18n.t(`remote.map.action.${kind}.confirm`),
    busy: i18n.t(`remote.map.action.${kind}.busy`),
    destructive: kind !== "apply",
  };
}

function actionDescription(action: CandidateAction): string {
  const revision = action.kind === "revert" ? action.revision : undefined;
  return i18n.t(`remote.map.action.${action.kind}.desc`, { revision });
}

function messageOf(error: unknown): string {
  return typeof error === "string" ? error : error instanceof Error ? error.message : String(error);
}

const ZOOMS = [1, 2, 4] as const;

function CandidatePicture({ candidate }: { candidate: CandidateStateView }) {
  const { t } = useTranslation();
  const [view, setView] = useState<"candidate" | "original">("candidate");
  const [zoom, setZoom] = useState<(typeof ZOOMS)[number]>(1);
  const [url, setUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [reload, setReload] = useState(0);
  const command = useMemo(() => wholeMapRender(candidate, view), [candidate, view]);

  useEffect(() => {
    let active = true;
    let created: string | null = null;
    setLoading(true);
    setError(null);
    void mapRender(command)
      .then((blob) => {
        if (!active) return;
        created = URL.createObjectURL(blob);
        setUrl(created);
      })
      .catch((reason) => {
        if (active) setError(i18n.t("remote.map.pictureFailed", { error: messageOf(reason) }));
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
      if (created) URL.revokeObjectURL(created);
    };
    // The picture follows the candidate's revision, not every state object.
  }, [candidate.revisionKey, candidate.sessionId, view, reload]);

  const zoomIndex = ZOOMS.indexOf(zoom);
  return (
    <section aria-label={t("remote.map.candidateMap")} className="grid gap-2">
      <div className="flex items-center gap-1">
        <Tabs value={view} onValueChange={(value) => setView(value as typeof view)}>
          <TabsList>
            <TabsTrigger value="candidate" className="min-h-9 px-3">
              {t("remote.map.candidateTab", { revision: candidate.currentRevision })}
            </TabsTrigger>
            <TabsTrigger value="original" className="min-h-9 px-3">
              {t("remote.map.original")}
            </TabsTrigger>
          </TabsList>
        </Tabs>
        <span className="flex-1" />
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="size-11"
          aria-label={t("remote.map.zoomOut")}
          disabled={zoomIndex <= 0}
          onClick={() => setZoom(ZOOMS[Math.max(0, zoomIndex - 1)])}
        >
          <ZoomOut aria-hidden className="size-4" />
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="size-11"
          aria-label={t("remote.map.zoomIn")}
          disabled={zoomIndex >= ZOOMS.length - 1}
          onClick={() => setZoom(ZOOMS[Math.min(ZOOMS.length - 1, zoomIndex + 1)])}
        >
          <ZoomIn aria-hidden className="size-4" />
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="size-11"
          aria-label={t("remote.map.refreshPicture")}
          disabled={loading}
          onClick={() => setReload((value) => value + 1)}
        >
          <RefreshCw aria-hidden className="size-4" />
        </Button>
      </div>
      <div className="relative max-h-[60dvh] overflow-auto rounded-lg border border-border bg-muted">
        {url ? (
          <img
            src={url}
            alt={view === "candidate" ? t("remote.map.altCandidate", { revision: candidate.currentRevision }) : t("remote.map.altOriginal")}
            className="block max-w-none [image-rendering:pixelated]"
            style={{ width: `${zoom * 100}%` }}
          />
        ) : (
          <div className="aspect-square w-full" />
        )}
        {loading && (
          <p
            role="status"
            className="absolute inset-x-0 top-0 flex items-center justify-center gap-2 bg-background/80 py-2 text-xs text-muted-foreground"
          >
            <LoaderCircle aria-hidden className="size-3.5 animate-spin motion-reduce:animate-none" />
            {t("remote.map.drawing")}
          </p>
        )}
      </div>
      {error && (
        <p role="alert" className="text-sm text-destructive">
          {error}
        </p>
      )}
    </section>
  );
}

function CandidatePanel({
  candidate,
  busy,
  locked,
  onAction,
}: {
  candidate: CandidateStateView;
  busy: CandidateAction["kind"] | null;
  locked: boolean;
  onAction(action: CandidateAction): void;
}) {
  const { t } = useTranslation();
  const summary = candidateSummary(candidate);
  const disabled = busy !== null || locked;
  return (
    <div className="grid content-start gap-4 px-4 py-4">
      <CandidatePicture candidate={candidate} />
      <section aria-label={t("remote.map.summaryAria")} className="grid gap-2">
        <h2 className="text-sm font-semibold">
          {summary.revision > 0 ? t("remote.map.summaryTitle", { revision: summary.revision }) : t("remote.map.noCandidate")}
        </h2>
        {summary.revision > 0 && summary.changes.length === 0 && (
          <p className="text-sm text-muted-foreground">{t("remote.map.noDiff")}</p>
        )}
        {summary.changes.length > 0 && (
          <ul className="grid gap-1 text-sm">
            {summary.changes.map((line) => (
              <li key={line}>{line}</li>
            ))}
          </ul>
        )}
        {summary.warnings.map((warning) => (
          <Alert key={warning}>
            <TriangleAlert aria-hidden />
            <AlertDescription>{warning}</AlertDescription>
          </Alert>
        ))}
        {summary.errors.map((error) => (
          <Alert key={error} variant="destructive">
            <TriangleAlert aria-hidden />
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        ))}
      </section>
      {locked && (
        <p className="text-xs text-muted-foreground">{t("remote.map.locked")}</p>
      )}
      <div className="grid grid-cols-2 gap-2">
        <Button
          type="button"
          className="min-h-11"
          disabled={disabled || !summary.canApply}
          onClick={() => onAction({ kind: "apply" })}
        >
          {busy === "apply" && (
            <LoaderCircle aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
          )}
          {t("remote.map.apply")}
        </Button>
        <Button
          type="button"
          variant="outline"
          className="min-h-11"
          disabled={disabled || !summary.canUndo}
          onClick={() => onAction({ kind: "undo" })}
        >
          {busy === "undo" && (
            <LoaderCircle aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
          )}
          {t("remote.map.undoApply")}
        </Button>
        <Button
          type="button"
          variant="outline"
          className="min-h-11"
          disabled={disabled || !summary.canDiscard}
          onClick={() => onAction({ kind: "discard" })}
        >
          {busy === "discard" && (
            <LoaderCircle aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
          )}
          {t("remote.map.discard")}
        </Button>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              type="button"
              variant="outline"
              className="min-h-11"
              disabled={disabled || summary.revertTargets.length === 0}
            >
              {busy === "revert" ? (
                <LoaderCircle aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
              ) : (
                <RotateCcw aria-hidden className="size-4" />
              )}
              {t("remote.map.revert")}
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            {summary.revertTargets.map((revision) => (
              <DropdownMenuItem
                key={revision}
                onSelect={() => onAction({ kind: "revert", revision })}
              >
                {revision === 0 ? t("remote.map.revisionOriginal") : `r${revision}`}
              </DropdownMenuItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
    </div>
  );
}

export interface RemoteMapProps {
  /** The Map session to open; `null` creates a new one. */
  sessionId: string | null;
  projectName: string;
  onBack(): void;
}

/** Waiting messages back in the composer, in the order they were sent. */
function steerMapDraft(steers: readonly PendingSteer[]): MapPromptDraft {
  return {
    text: steers
      .map((steer) => steer.text)
      .filter((text) => text.length > 0)
      .join("\n\n"),
    attachments: steers.flatMap((steer) => steer.attachments),
  };
}

export function RemoteMap({ sessionId: requested, projectName, onBack }: RemoteMapProps) {
  const { t } = useTranslation();
  const [bootstrap, setBootstrap] = useState<MapBootstrapResponse | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [generation, setGeneration] = useState(0);
  const requestedRef = useRef(requested);

  // Open (or, after a reconnect, reopen) the session.
  useEffect(() => {
    let active = true;
    setLoadError(null);
    const id = requestedRef.current;
    void (id === null ? mapSessionCreate() : mapSessionLoad(id))
      .then((next) => {
        if (!active) return;
        requestedRef.current = next.session.id;
        setBootstrap(next);
      })
      .catch((reason) => {
        if (active) setLoadError(messageOf(reason));
      });
    return () => {
      active = false;
    };
  }, [generation]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void listen(RESYNC_EVENT, () => setGeneration((value) => value + 1)).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  if (!bootstrap) {
    return (
      <div className="flex h-full min-w-0 flex-col bg-background text-foreground">
        <MapHeader title={t("remote.map.title")} subtitle={projectName} onBack={onBack} />
        <main className="grid content-start gap-3 px-4 py-4">
          {loadError ? (
            <>
              <p
                role="alert"
                className="rounded-lg border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm text-destructive"
              >
                {t("remote.map.openFailed", { error: loadError })}
              </p>
              <Button
                type="button"
                variant="outline"
                className="min-h-11"
                onClick={() => setGeneration((value) => value + 1)}
              >
                {t("remote.common.retry")}
              </Button>
            </>
          ) : (
            <p role="status" className="flex items-center gap-2 text-sm text-muted-foreground">
              <LoaderCircle aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
              {t("remote.map.opening")}
            </p>
          )}
        </main>
      </div>
    );
  }
  return (
    <RemoteMapSession
      key={`${bootstrap.session.id}:${generation}`}
      bootstrap={bootstrap}
      projectName={projectName}
      onBack={onBack}
      onReload={() => setGeneration((value) => value + 1)}
    />
  );
}

function MapHeader({
  title,
  subtitle,
  onBack,
  children,
}: {
  title: string;
  subtitle: string;
  onBack(): void;
  children?: React.ReactNode;
}) {
  const { t } = useTranslation();
  return (
    <header className="flex min-h-14 shrink-0 items-center gap-1 border-b border-border pl-2 pr-2">
      <Button
        type="button"
        variant="ghost"
        size="icon"
        className="size-11 shrink-0"
        aria-label={t("remote.map.backToList")}
        onClick={onBack}
      >
        <ChevronLeft aria-hidden className="size-5" />
      </Button>
      <div className="min-w-0 flex-1">
        <h1 className="truncate text-sm font-semibold" title={title}>
          {title}
        </h1>
        <p className="truncate text-xs text-muted-foreground">{subtitle}</p>
      </div>
      {children}
    </header>
  );
}

function RemoteMapSession({
  bootstrap,
  projectName,
  onBack,
  onReload,
}: {
  bootstrap: MapBootstrapResponse;
  projectName: string;
  onBack(): void;
  onReload(): void;
}) {
  const { t } = useTranslation();
  const sessionId = bootstrap.session.id;
  const projectId = bootstrap.context.revision.projectId;
  const store: RemoteMapStore = useMemo(() => createRemoteMapStore(bootstrap), [bootstrap]);
  const state = useSyncExternalStore(store.subscribe, store.getState, store.getState);
  const [candidate, setCandidate] = useState<CandidateStateView>(bootstrap.candidate);
  const candidateRef = useRef(candidate);
  candidateRef.current = candidate;
  const [tab, setTab] = useState<"chat" | "map">("chat");
  const [resumeError, setResumeError] = useState(bootstrap.conversationResumeError ?? null);
  const [resetBusy, setResetBusy] = useState(false);
  const [resetOpen, setResetOpen] = useState(false);
  const [actionBusy, setActionBusy] = useState(false);
  const [pending, setPending] = useState<CandidateAction | null>(null);
  const [running, setRunning] = useState<CandidateAction["kind"] | null>(null);
  const [modelSettings, setModelSettings] = useState<SessionModelSettings | null>(null);
  const [modelBusy, setModelBusy] = useState(false);
  const [editDraft, setEditDraft] = useState<MapPromptDraft | null>(null);
  const [rewindBusy, setRewindBusy] = useState(false);
  const [recovering, setRecovering] = useState(false);
  /** This phone wrote to the conversation, so it saves the log (`logWriter.ts`). */
  const participatedRef = useRef(false);

  // A turn whose answer was lost is sent again with the same request id
  // once the socket is back; when the page gives up, the session is
  // reopened and its run snapshot brings the live or finished turn.
  const recovery = useMemo<RecoveryHooks>(
    () => ({
      onLost: () => setRecovering(true),
      waitForOpen: () => remoteTransport().waitForOpen(),
      onRecovered: (_cmd, answered) => {
        if (answered) setRecovering(false);
        else onReload();
      },
    }),
    [onReload],
  );

  const acceptCandidate = useCallback((next: unknown) => {
    const view = next as CandidateStateView & { projectId?: string };
    if (view.sessionId !== sessionId) return;
    if (view.projectId !== undefined && view.projectId !== projectId) return;
    setCandidate(view);
  }, [projectId, sessionId]);

  // Live events and runs started on the PC or by an EPS team task.
  useEffect(() => {
    const unlisteners: Array<() => void> = [];
    let disposed = false;
    const on = <T,>(event: string, handler: (payload: T) => void) => {
      void listen<T>(event, ({ payload }) => {
        if (!disposed) handler(payload);
      }).then((fn) => {
        if (disposed) fn();
        else unlisteners.push(fn);
      });
    };
    for (const name of ["agent_event", "context_usage", "answer", "error", "ask"]) {
      on<Record<string, unknown>>(name, (payload) => store.event(name, payload));
    }
    on<Record<string, unknown>>("steer", (payload) => {
      const returned = store.steer(payload);
      if (returned.length > 0) setEditDraft(steerMapDraft(returned));
    });
    const restoreAsk = async () => {
      const ask = await invoke<PendingMapAsk | null>("ask_pending", { sessionId });
      if (!disposed && ask) store.restoreAsk(ask, candidateRef.current.revisionKey);
    };
    on<MapRunStartedPayload>("map_run_started", (payload) => {
      if (!store.runStarted(payload)) return;
      void mapRunSnapshot(sessionId)
        .then(async (run) => {
          if (disposed || !run || run.requestId !== payload.requestId) return;
          store.adoptRun(run);
          await restoreAsk();
        })
        .catch((reason) => toast.error(i18n.t("remote.map.pendingLoadFailed", { error: messageOf(reason) })));
    });
    on<unknown>("map_candidate_state", acceptCandidate);
    on<unknown>("map_apply_result", acceptCandidate);
    if (bootstrap.pendingRun) store.adoptRun(bootstrap.pendingRun);
    void restoreAsk().catch(() => {});
    return () => {
      disposed = true;
      for (const fn of unlisteners) fn();
    };
  }, [acceptCandidate, bootstrap.pendingRun, sessionId, store]);

  useEffect(() => {
    let active = true;
    void sessionModelSettingsGet(sessionId)
      .then((settings) => active && setModelSettings(settings))
      .catch(() => active && setModelSettings(null));
    return () => {
      active = false;
    };
  }, [sessionId]);

  // The conversation is saved with the session once a turn settles, by the
  // same rule as an EPS conversation (`logWriter.ts`): never just for being
  // opened, and only when this phone took part or no PC window shows the
  // project. A run the phone only watched stays in the run transcript, which
  // every Map client adopts when it opens the session.
  // A save after a reconnect since the session was opened is checked
  // against the PC's log first (`saveLog`).
  const openedConversation = useRef(state.conversation);
  const loadedAt = useRef(remoteTransport().reconnectCount());
  useEffect(() => {
    if (state.inFlight || state.conversation === openedConversation.current) return;
    const timer = window.setTimeout(() => {
      void saveLog({
        invoke,
        id: sessionId,
        participated: participatedRef.current,
        projectId,
        log: () => panelLogFromConversation(store.getState().conversation, store.logSequence()),
        transport: remoteTransport(),
        loadedAt: loadedAt.current,
      });
    }, 500);
    return () => window.clearTimeout(timer);
  }, [projectId, sessionId, state.conversation, state.inFlight, store]);

  // Steers that found no running request, sent once the run has ended here.
  const [unsentSteers, setUnsentSteers] = useState<string[]>([]);

  /** Send a message composed while a request runs to that request. */
  const steer = useCallback(
    async (text: string, attachments: ChatAttachment[]) => {
      participatedRef.current = true;
      const steerId = crypto.randomUUID();
      store.steerSent({ steerId, text: text.trim(), attachments, mentions: [] });
      try {
        // Resent under the same request id when its answer is lost; `null`:
        // given up, and the session is being reopened from the PC.
        const result = (await invokeResending(
          invoke,
          "map_agent_chat_steer",
          {
            command: {
              sessionId,
              steerId,
              text,
              attachments: attachments.map((attachment) => attachment.id),
              mentions: [],
            },
          },
          recovery,
        )) as "accepted" | "no_turn" | null;
        if (result === "no_turn") setUnsentSteers((current) => [...current, steerId]);
      } catch (reason) {
        const returned = store.steerTaken([steerId]);
        if (returned.length > 0) setEditDraft(steerMapDraft(returned));
        toast.error(i18n.t("remote.chat.steerFailed", { error: messageOf(reason) }));
      }
    },
    [recovery, sessionId, store],
  );

  const send = useCallback(
    async (text: string, attachments: ChatAttachment[], imagePlacement: boolean) => {
      if (actionBusy) return;
      if (state.inFlight) {
        await steer(text, attachments);
        return;
      }
      if (text.trim() === "/compact" && attachments.length === 0) {
        setActionBusy(true);
        store.note("info", i18n.t("remote.chat.compacting"));
        try {
          await compactSession(sessionId);
          store.note("ok", i18n.t("remote.chat.compacted"));
        } catch (reason) {
          store.note("error", i18n.t("remote.chat.compactFailed", { error: messageOf(reason) }));
        } finally {
          setActionBusy(false);
        }
        return;
      }
      if (!text.trim() && attachments.length === 0) return;
      participatedRef.current = true;
      const current = candidateRef.current;
      store.sendStarted(text, attachments, current.revisionKey);
      try {
        const command = {
          sessionId,
          text,
          attachments: attachments.map((attachment) => attachment.id),
          candidateRevision: current.currentRevision,
          mentions: [],
          ...(imagePlacement ? { imagePlacement: true } : {}),
        };
        const next = (await invokeResending(
          invoke,
          "map_agent_chat",
          { command },
          recovery,
        )) as CandidateStateView | null;
        // `null`: given up, and the session is being reopened.
        if (next === null) return;
        setCandidate(next);
        store.sendSettled();
      } catch (reason) {
        store.sendSettled(messageOf(reason));
      }
    },
    [actionBusy, recovery, sessionId, state.inFlight, steer, store],
  );

  useEffect(() => {
    if (state.inFlight || actionBusy || unsentSteers.length === 0) return;
    const [steerId, ...rest] = unsentSteers;
    setUnsentSteers(rest);
    const [unsent] = store.steerTaken([steerId]);
    if (unsent === undefined) return;
    if (store.getState().lastRunCancelled) setEditDraft(steerMapDraft([unsent]));
    else void send(unsent.text, unsent.attachments, false);
  }, [actionBusy, send, state.inFlight, store, unsentSteers]);

  const cancel = useCallback(async () => {
    if (!store.getState().inFlight || actionBusy) return;
    setActionBusy(true);
    participatedRef.current = true;
    store.cancelled();
    try {
      await mapCancel(sessionId);
    } catch (reason) {
      toast.error(i18n.t("remote.map.stopFailed", { error: messageOf(reason) }));
    } finally {
      setActionBusy(false);
    }
  }, [actionBusy, sessionId, store]);

  const answer = useCallback(
    async (requestId: string, answers: Record<string, AskAnswer>) => {
      participatedRef.current = true;
      store.askSubmitting(true);
      try {
        // Answered, or given up and the session is being reopened.
        await invokeResending(invoke, "ask_response", { sessionId, requestId, answers }, recovery);
        store.askAnswered();
      } catch (reason) {
        store.askSubmitting(false);
        toast.error(i18n.t("remote.map.answerFailed", { error: messageOf(reason) }));
      }
    },
    [recovery, sessionId, store],
  );

  const resetConversation = useCallback(async () => {
    participatedRef.current = true;
    setResetBusy(true);
    try {
      await mapConversationReset(sessionId);
      setResumeError(null);
      setResetOpen(false);
      store.note("info", i18n.t("remote.map.resetDone"));
    } catch (reason) {
      toast.error(i18n.t("remote.map.resetFailed", { error: messageOf(reason) }));
    } finally {
      setResetBusy(false);
    }
  }, [sessionId, store]);

  const editMessage = useCallback(
    async (entry: MapConversationEntry) => {
      if (entry.kind !== "you" || state.inFlight || rewindBusy) return;
      participatedRef.current = true;
      setRewindBusy(true);
      try {
        const prefix = rewindPrefix(store.getState().conversation, entry.id);
        await mapConversationRewind(sessionId, panelLogFromConversation(prefix, store.logSequence()));
        store.rewound(entry.id);
        setResumeError(null);
        setEditDraft({ text: entry.text, attachments: entry.attachments ?? [] });
      } catch (reason) {
        toast.error(i18n.t("remote.map.rewindFailed", { error: messageOf(reason) }));
      } finally {
        setRewindBusy(false);
      }
    },
    [rewindBusy, sessionId, state.inFlight, store],
  );

  const runAction = useCallback(
    async (action: CandidateAction) => {
      setRunning(action.kind);
      try {
        switch (action.kind) {
          case "apply":
            setCandidate(await candidateApply(sessionId));
            toast.success(i18n.t("remote.map.applied"));
            break;
          case "undo":
            setCandidate(await applyUndo(sessionId));
            toast.success(i18n.t("remote.map.undone"));
            break;
          case "discard":
            await candidateDiscard(sessionId);
            // Discarding ends the candidate; reopen the session on the saved map.
            onReload();
            break;
          case "revert":
            setCandidate(await candidateRevert(sessionId, action.revision));
            break;
        }
        setPending(null);
      } catch (reason) {
        setPending(null);
        toast.error(messageOf(reason), { duration: 10_000 });
      } finally {
        setRunning(null);
      }
    },
    [onReload, sessionId],
  );

  const team = bootstrap.session.teamParent !== undefined;
  const copy = pending ? actionCopy(pending.kind) : null;

  return (
    <div className="flex h-full min-w-0 flex-col overflow-hidden bg-background text-foreground">
      <MapHeader
        title={bootstrap.session.name}
        subtitle={state.inFlight ? t("remote.map.subtitleBusy", { project: projectName }) : t("remote.map.subtitle", { project: projectName })}
        onBack={onBack}
      >
        {team && <Badge variant="secondary">{t("remote.common.team")}</Badge>}
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="size-11 shrink-0"
              aria-label={t("remote.map.menuAria")}
            >
              <MoreVertical aria-hidden className="size-4" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuItem onSelect={() => onReload()}>{t("remote.map.reload")}</DropdownMenuItem>
            <DropdownMenuItem
              disabled={state.inFlight}
              onSelect={() => setResetOpen(true)}
            >
              {t("remote.map.resetConversation")}
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </MapHeader>

      <Tabs
        value={tab}
        onValueChange={(value) => setTab(value as typeof tab)}
        className="flex min-h-0 flex-1 flex-col gap-0"
      >
        <TabsList className="mx-4 mt-2 grid w-auto grid-cols-2">
          <TabsTrigger value="chat" className="min-h-9 gap-1.5">
            <MessageSquare aria-hidden className="size-4" />
            {t("remote.common.conversation")}
          </TabsTrigger>
          <TabsTrigger value="map" className="min-h-9 gap-1.5">
            <MapIcon aria-hidden className="size-4" />
            {t("remote.map.candidateTab", { revision: candidate.currentRevision })}
          </TabsTrigger>
        </TabsList>

        <TabsContent
          value="chat"
          forceMount
          className="flex min-h-0 flex-1 flex-col overflow-hidden data-[state=inactive]:hidden"
        >
          {recovering && (
            <p
              role="status"
              className="flex shrink-0 items-center gap-2 border-b border-border px-4 py-2 text-xs text-muted-foreground"
            >
              <LoaderCircle aria-hidden className="size-3.5 animate-spin motion-reduce:animate-none" />
              {recoveryNote()}
            </p>
          )}
          <ConversationLog
            log={state.conversation}
            phase={state.inFlight ? "thinking" : "ready"}
            turn={state.turn}
            pendingSteers={state.pendingSteers}
            emptyTitle={t("remote.map.emptyTitle")}
            emptyDescription={t("remote.map.emptyDesc")}
            onEditMessage={(entry) => void editMessage(entry as MapConversationEntry)}
            editDisabled={state.inFlight || rewindBusy}
            tail={
              state.ask ? (
                <AskCard
                  requestId={state.ask.requestId}
                  questions={state.ask.questions}
                  submitting={state.ask.submitting}
                  waitSeconds={state.ask.waitSeconds}
                  receivedAt={state.ask.receivedAt}
                  onSubmit={(answers) => void answer(state.ask!.requestId, answers)}
                  onInteract={() =>
                    invoke<number>("ask_touch", { sessionId, requestId: state.ask!.requestId })
                  }
                />
              ) : undefined
            }
          />
          {resumeError && (
            <Alert variant="destructive" className="mx-4 mb-2 w-auto">
              <TriangleAlert aria-hidden />
              <AlertTitle>{t("remote.map.resumeTitle")}</AlertTitle>
              <AlertDescription>
                <p>{resumeError}</p>
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  className="min-h-11"
                  disabled={resetBusy}
                  onClick={() => void resetConversation()}
                >
                  <RotateCcw aria-hidden className="size-4" />
                  {resetBusy ? t("remote.map.resetting") : t("remote.map.resetConversation")}
                </Button>
              </AlertDescription>
            </Alert>
          )}
          <div className="shrink-0 pb-[env(safe-area-inset-bottom)]">
            <MapPromptInput
              turn={state.turn}
              live={state.inFlight}
              askPending={state.ask !== undefined}
              actionBusy={actionBusy || resumeError !== null}
              mentionCount={0}
              hasStaleMentions={false}
              draftScope={`remote-map:${sessionId}`}
              placeholder={t("remote.map.placeholder")}
              draft={editDraft}
              contextUsage={state.contextUsage}
              modelSettings={modelSettings}
              modelSettingsBusy={modelBusy}
              onSend={(text, attachments, imagePlacement) =>
                void send(text, attachments, imagePlacement)
              }
              onCancel={() => void cancel()}
              onStageAttachment={stageAttachment}
              onDiscardAttachment={discardAttachment}
              onModelSettingsChange={(model: string, reasoning: ReasoningSelection | undefined) => {
                // An empty level comes from a select whose options changed, not the user.
                if (reasoning !== undefined && !reasoning.level) return;
                setModelBusy(true);
                void sessionModelSettingsSave(sessionId, model, reasoning)
                  .then(setModelSettings)
                  .catch(() => toast.error(i18n.t("remote.chat.modelSaveFailed")))
                  .finally(() => setModelBusy(false));
              }}
              onModelSettingsReload={() => {
                void sessionModelSettingsGet(sessionId).then(setModelSettings);
              }}
            />
          </div>
        </TabsContent>

        <TabsContent value="map" className="min-h-0 flex-1 overflow-y-auto">
          <CandidatePanel
            candidate={candidate}
            busy={running}
            locked={state.inFlight}
            onAction={setPending}
          />
        </TabsContent>
      </Tabs>

      <Dialog open={pending !== null} onOpenChange={(open) => !open && running === null && setPending(null)}>
        <DialogContent className="w-[calc(100vw-2rem)] max-w-sm">
          <DialogHeader>
            <DialogTitle>{copy?.title}</DialogTitle>
            <DialogDescription>{pending ? actionDescription(pending) : ""}</DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              className="min-h-11"
              disabled={running !== null}
              onClick={() => setPending(null)}
            >
              {t("remote.common.cancel")}
            </Button>
            <Button
              type="button"
              variant={copy?.destructive ? "destructive" : "default"}
              className="min-h-11"
              disabled={running !== null}
              aria-busy={running !== null || undefined}
              onClick={() => pending && void runAction(pending)}
            >
              {running !== null && (
                <LoaderCircle aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
              )}
              {running !== null ? copy?.busy : copy?.confirm}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={resetOpen} onOpenChange={(open) => !open && !resetBusy && setResetOpen(false)}>
        <DialogContent className="w-[calc(100vw-2rem)] max-w-sm">
          <DialogHeader>
            <DialogTitle>{t("remote.map.resetDialogTitle")}</DialogTitle>
            <DialogDescription>
              {t("remote.map.resetDialogDesc")}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              className="min-h-11"
              disabled={resetBusy}
              onClick={() => setResetOpen(false)}
            >
              {t("remote.common.cancel")}
            </Button>
            <Button
              type="button"
              variant="destructive"
              className="min-h-11"
              disabled={resetBusy}
              onClick={() => void resetConversation()}
            >
              {resetBusy ? t("remote.map.resetting") : t("remote.map.resetConversation")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
