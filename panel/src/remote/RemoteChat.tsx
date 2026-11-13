/**
 * One EPS conversation on the phone: the desktop's conversation log, composer
 * (attachments, skills, interview, model), question card, plan review and
 * build result, over the remote transport. It follows one session; the
 * desktop's split panes and message editing stay on the PC. A message sent
 * while a turn runs steers that turn (`chat_steer`), as on the PC.
 */
import i18n from "@/lib/i18n";
import { useTranslation } from "react-i18next";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { ChevronLeft, ClipboardList, Hammer, LoaderCircle, Square } from "lucide-react";
import { toast } from "sonner";

import { AskCard } from "@/components/AskCard";
import { BuildResultDialog } from "@/components/BuildResultDialog";
import { ConversationLog } from "@/components/ConversationLog";
import {
  InstructionBox,
  type ChatPayload,
  type SteerPayload,
} from "@/components/InstructionBox";
import { PlanView } from "@/components/PlanView";
import type { SessionActivity } from "@/components/SessionSidebar";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { discardAttachment, stageAttachment } from "@/lib/attachments";
import {
  IpcClient,
  compactSession,
  mentionSearch,
  providerSettingsGet,
  providerStatusList,
  runProjectBuild,
  sessionModelSettingsGet,
  sessionModelSettingsSave,
  skillList,
  type AskAnswer,
  type AskKind,
  type AskQuestion,
  type MentionSearchRequest,
  type ProjectBuildReport,
  type ReasoningSelection,
  type ServerMessage,
  type SessionModelSettings,
  type SessionRecord,
  type SkillSummary,
} from "@/lib/ipc";
import { progressLabel } from "@/lib/progress";
import { RESYNC_EVENT, remoteTransport } from "@/lib/remoteTransport";
import { invoke, listen } from "@/lib/transport";
import { providerChatErrorText } from "@/providers/providerCopy";
import { EMPTY_ASK_DRAFT } from "@/state/askDraft";
import { emptyPanelLog, serializePanelLog } from "@/state/panelLog";
import {
  createPanelStore,
  isBusyPhase,
  type PanelStore,
  type PendingSteer,
} from "@/state/store";
import {
  steerAsChat,
  steerDraft,
  steerReturnsToComposer,
  whenTurnEnds,
} from "@/lib/steer";
import type { SteerPushResult } from "@/lib/protocol";

import { recoveryNote, invokeResending, recoveringInvoke } from "./connectionRecovery";
import { createLogSaver, saveLog } from "./logWriter";

interface PendingAsk {
  sessionId: string;
  requestId: string;
  questions: AskQuestion[];
  waitSeconds?: number;
  kind?: AskKind;
}

const ACTIVITY_KEYS: Partial<Record<SessionActivity, string>> = {
  running_read: "remote.chat.activity.running_read",
  running_write: "remote.chat.activity.running_write",
  waiting_input: "remote.chat.activity.waiting_input",
  review: "remote.chat.activity.review",
};

export interface RemoteChatProps {
  /** The session to show; `null` starts a new conversation on first send. */
  sessionId: string | null;
  projectName: string;
  onBack(): void;
}

/**
 * The model a new conversation starts with: the PC's default provider and its
 * saved model, as the desktop's new-chat composer shows them.
 */
async function draftModelSettings(): Promise<SessionModelSettings | null> {
  const provider = (await providerStatusList(invoke)).find(
    (status) => status.selectedAsDefault,
  )?.provider;
  if (!provider) return null;
  const view = await providerSettingsGet(provider, invoke);
  const selectedModel =
    view.selectedModel ?? view.models.find((model) => model.isDefault)?.model;
  if (!selectedModel) return null;
  return {
    provider,
    models: view.models,
    selectedModel,
    ...(view.selectedReasoning ? { selectedReasoning: view.selectedReasoning } : {}),
  };
}

/** Route one app message to the conversation's store. */
function applyMessage(
  store: PanelStore,
  sessionId: string | null,
  msg: ServerMessage,
  setActivity: (activity: SessionActivity) => void,
  onSteersReturned: (steers: PendingSteer[]) => void = () => undefined,
): void {
  switch (msg.type) {
    case "status":
      store.applyStatus({ compiling: msg.compiling, project: msg.project });
      return;
    case "list":
      store.applyList({ files: msg.files, error: msg.error });
      return;
    case "progress":
      if (msg.stage === "rag_warmup") {
        store.ragWarmupChanged(
          msg.detail === "done"
            ? "ready"
            : msg.detail?.startsWith("error")
              ? "unavailable"
              : "loading",
        );
        return;
      }
      if (msg.sessionId === undefined || msg.sessionId !== sessionId) return;
      store.progressReceived(msg.stage);
      {
        const { kind, text } = progressLabel(msg.stage, msg.detail);
        store.log(kind, text, msg.stage);
      }
      return;
    default:
      break;
  }
  if (!("sessionId" in msg) || msg.sessionId !== sessionId) return;
  switch (msg.type) {
    case "agent_event":
      store.agentEvent(msg.kind, msg.detail, msg.data);
      return;
    case "context_usage":
      store.contextUsageReceived(msg.tokenUsage);
      return;
    case "answer":
      store.answerReceived(msg.text);
      return;
    case "ask":
      if (msg.status === "expired") store.askExpired(msg.requestId);
      else store.askReceived(msg.requestId, msg.questions, msg.waitSeconds, msg.kind);
      return;
    case "team_task": {
      const previous = store
        .getState()
        .teamTasks.find((task) => task.id === msg.task.id);
      store.teamTaskReceived(msg.task);
      if (previous?.status.kind !== msg.task.status.kind) {
        if (msg.task.status.kind === "candidate_ready") {
          store.log(
            "agent",
            i18n.t("remote.chat.teamCandidateReady", { revision: msg.task.candidate?.revision ?? "?" }),
          );
        } else if (msg.task.status.kind === "applied") {
          store.log("agent", i18n.t("remote.chat.teamApplied"));
        } else if (msg.task.status.kind === "failed") {
          store.log("warn", i18n.t("remote.chat.teamFailed", { reason: msg.task.status.reason }));
        }
      }
      if (msg.continuation) store.chatSent();
      return;
    }
    case "worker":
      if (msg.continuation) store.chatSent();
      return;
    case "steer":
      if (msg.kind === "applied") {
        store.steerApplied(msg.steers);
      } else if (msg.kind === "turn") {
        store.steerTurnStarted(msg.steers, msg.clientTurnId);
      } else {
        const returned = store.steerTaken(msg.steers.map((steer) => steer.steerId));
        if (returned.length > 0) onSteersReturned(returned);
      }
      return;
    case "plan": {
      const prior = store.getState().plan;
      if (prior !== null && prior.revision !== msg.revision) {
        store.log("agent", i18n.t("remote.chat.planUpdated", { revision: prior.revision }));
      }
      store.planReceived(msg.markdown, msg.revision);
      store.log("agent", i18n.t("remote.chat.planArrived", { revision: msg.revision }));
      return;
    }
    case "git":
      store.gitCommitted(msg);
      return;
    case "error":
      store.errorReceived(msg.message);
      store.log("error", i18n.t("remote.chat.error", { message: providerChatErrorText(msg.message) }));
      return;
    case "session_activity":
      if (msg.activity === "running_write") {
        store.log("ok", i18n.t("remote.chat.isolatedStart"));
      }
      setActivity(msg.activity);
      return;
    default:
      return;
  }
}

export function RemoteChat({ sessionId: initialSessionId, projectName, onBack }: RemoteChatProps) {
  const { t, i18n: i18nHook } = useTranslation();
  const [sessionId, setSessionId] = useState(initialSessionId);
  const sessionIdRef = useRef(initialSessionId);
  // A resync (reconnect, dropped events) reloads the conversation into a
  // fresh store, exactly as opening it again would.
  const [generation, setGeneration] = useState(0);
  const store = useMemo(() => createPanelStore(), [generation]);
  const storeRef = useRef(store);
  storeRef.current = store;
  const state = useSyncExternalStore(store.subscribe, store.getState, store.getState);
  const [record, setRecord] = useState<SessionRecord | null>(null);
  const [activity, setActivity] = useState<SessionActivity>("idle");
  const [modelSettings, setModelSettings] = useState<SessionModelSettings | null>(null);
  const [modelBusy, setModelBusy] = useState(false);
  const [skills, setSkills] = useState<SkillSummary[]>([]);
  const [actionBusy, setActionBusy] = useState(false);
  const [draft, setDraft] = useState<ChatPayload | null>(null);
  const [planOpen, setPlanOpen] = useState(false);
  const [building, setBuilding] = useState(false);
  const [buildReport, setBuildReport] = useState<ProjectBuildReport | null>(null);
  const [buildError, setBuildError] = useState<string | null>(null);
  const [buildOpen, setBuildOpen] = useState(false);
  const clientRef = useRef<IpcClient | null>(null);
  /** A new conversation's model choice, applied once the session exists. */
  const draftModelRef = useRef<{ model: string; reasoning?: ReasoningSelection } | null>(null);
  /** This phone wrote to the conversation, so it saves the log (`logWriter.ts`). */
  const participatedRef = useRef(false);
  /**
   * Save what is pending now. `checkSaved` first compares the saved log: a
   * save scheduled before the connection dropped is dropped when the PC
   * saved a newer log meanwhile.
   */
  const flushLogRef = useRef<(checkSaved: boolean) => Promise<void>>(async () => {});
  const [recovering, setRecovering] = useState(false);
  /** Loading the saved log into the store is not a change to save. */
  const hydratingRef = useRef(false);

  // A long turn whose answer was lost is sent again with the same request
  // id once the socket is back (`connectionRecovery.ts`); the PC joins the
  // running turn or answers with its result. Once it settled the
  // conversation is reloaded, since the socket may have missed its end.
  const recovered = useCallback(async () => {
    await flushLogRef.current(true);
    setGeneration((value) => value + 1);
  }, []);
  const recoveredRef = useRef(recovered);
  recoveredRef.current = recovered;

  // One client for the screen's life; it delivers to whichever store is current.
  useEffect(() => {
    const client = new IpcClient({
      invoke: recoveringInvoke(invoke, {
        onLost: () => setRecovering(true),
        waitForOpen: () => remoteTransport().waitForOpen(),
        onRecovered: () => void recoveredRef.current(),
      }),
      onMessage: (msg) =>
        applyMessage(storeRef.current, sessionIdRef.current, msg, setActivity, (returned) =>
          setDraft(steerDraft(returned)),
        ),
      onLog: (_kind, text) => storeRef.current.log("warn", text),
      onOpenChange: (open) => {
        if (open) storeRef.current.wsOpen();
        else storeRef.current.wsError();
      },
      onProjectAvailabilityChange: (available) =>
        storeRef.current.projectAvailabilityChanged(available),
    });
    clientRef.current = client;
    void client.connect();
    let unlisten: (() => void) | undefined;
    let disposed = false;
    // The reload replaces the store: its unsaved rows are saved first.
    void listen(RESYNC_EVENT, () => {
      void flushLogRef.current(true).finally(() => setGeneration((value) => value + 1));
    }).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
      client.stop();
      clientRef.current = null;
    };
  }, []);

  useEffect(() => {
    let active = true;
    void skillList()
      .then((list) => {
        if (active) setSkills(list);
      })
      .catch(() => {});
    return () => {
      active = false;
    };
    // Skill titles come in the app language; reload them when it changes.
  }, [i18nHook.language]);

  // Load (or reload) the conversation into the current store.
  useEffect(() => {
    let active = true;
    store.wsOpen();
    const client = clientRef.current;
    client?.invalidateProject();
    void client?.refresh();
    const id = sessionIdRef.current;
    if (id === null) return;
    void (async () => {
      try {
        let loaded = await invoke<SessionRecord>("session_load", { id });
        if (!active) return;
        hydratingRef.current = true;
        try {
          store.hydrate(loaded.panelLog ?? emptyPanelLog());
        } finally {
          hydratingRef.current = false;
        }
        if (loaded.contextUsage !== undefined) store.contextUsageReceived(loaded.contextUsage);
        store.setTeamTasks(loaded.teamTasks ?? []);
        setRecord(loaded);
        if (loaded.pendingRequestIds.length > 0) {
          setActivity("review");
          loaded = await invoke<SessionRecord>("session_open", { id });
          if (!active) return;
          setRecord(loaded);
        }
        const pending = await invoke<PendingAsk | null>("ask_pending", { sessionId: id });
        if (active && pending !== null && pending.sessionId === id) {
          setActivity("waiting_input");
          store.askReceived(
            pending.requestId,
            pending.questions,
            pending.waitSeconds,
            pending.kind,
          );
        }
      } catch (error) {
        if (active) store.log("error", i18n.t("remote.chat.loadFailed", { error: String(error) }));
      } finally {
        if (active) setRecovering(false);
      }
    })();
    return () => {
      active = false;
    };
  }, [store]);

  useEffect(() => {
    let active = true;
    if (sessionId === null) {
      draftModelRef.current = null;
      void draftModelSettings()
        .then((settings) => {
          if (active) setModelSettings(settings);
        })
        .catch(() => {
          if (active) setModelSettings(null);
        });
      return () => {
        active = false;
      };
    }
    void sessionModelSettingsGet(sessionId)
      .then((settings) => {
        if (active) setModelSettings(settings);
      })
      .catch(() => {
        if (active) setModelSettings(null);
      });
    return () => {
      active = false;
    };
  }, [sessionId]);

  // The conversation log is saved with the session, as the desktop does,
  // unless a PC window that this phone only watches saves it (`logWriter.ts`).
  // A streaming turn is saved at least every few seconds, not only once it
  // pauses.
  useEffect(() => {
    let observed = store.getState().log;
    let checkSaved = false;
    // The reconnect count this store's conversation was loaded at: a save
    // after a later reconnect is checked against the PC's log first.
    const loadedAt = remoteTransport().reconnectCount();
    const save = async () => {
      const check = checkSaved;
      checkSaved = false;
      const id = sessionIdRef.current;
      if (id === null) return;
      await saveLog({
        invoke,
        id,
        participated: participatedRef.current,
        projectId: remoteTransport().attachedProjectId(),
        log: () => serializePanelLog(store.getState().log),
        transport: remoteTransport(),
        loadedAt,
        checkSaved: check,
      });
    };
    const saver = createLogSaver(save);
    flushLogRef.current = async (check) => {
      if (!saver.pending()) return;
      checkSaved = check;
      await saver.flush();
    };
    const unsubscribe = store.subscribe((snapshot) => {
      if (snapshot.log === observed) return;
      observed = snapshot.log;
      if (sessionIdRef.current === null || hydratingRef.current) return;
      saver.schedule();
    });
    return () => {
      unsubscribe();
      saver.cancel();
    };
  }, [store]);

  useEffect(() => {
    if (state.phase === "plan_review" && state.plan !== null) setPlanOpen(true);
  }, [state.phase, state.plan?.revision]);

  const send = useCallback(
    async (payload: ChatPayload) => {
      setDraft(null);
      const client = clientRef.current;
      if (!client) return;
      const clientTurnId = payload.clientTurnId ?? crypto.randomUUID();
      const restore = () => setDraft({ ...payload, clientTurnId });
      participatedRef.current = true;
      const compact =
        payload.text.trim() === "/compact" &&
        payload.attachments.length === 0 &&
        payload.mentions.length === 0;
      try {
        if (compact) {
          const id = sessionIdRef.current;
          if (id === null) {
            toast.error(i18n.t("remote.chat.nothingToCompact"));
            return;
          }
          setActionBusy(true);
          store.log("info", i18n.t("remote.chat.compacting"));
          try {
            await compactSession(id);
            store.log("ok", i18n.t("remote.chat.compacted"));
          } catch (error) {
            store.log("error", i18n.t("remote.chat.compactFailed", { error: String(error) }));
          } finally {
            setActionBusy(false);
          }
          return;
        }
        let id = sessionIdRef.current;
        if (id === null) {
          const seed =
            payload.text.trim() ||
            (payload.mentions.length > 0
              ? i18n.t("app.chat.seedMention")
              : i18n.t("app.chat.seedAttachment"));
          const created = await invoke<SessionRecord>("session_create", { firstText: seed });
          id = created.id;
          sessionIdRef.current = id;
          setSessionId(id);
          setRecord(created);
          const chosen = draftModelRef.current;
          draftModelRef.current = null;
          if (chosen) {
            try {
              await sessionModelSettingsSave(id, chosen.model, chosen.reasoning);
            } catch {
              toast.error(i18n.t("remote.chat.modelFallback"));
            }
          }
        }
        if (store.getState().phase === "plan_review") {
          store.log("you", payload.text, undefined, payload.attachments, payload.mentions, clientTurnId);
          store.log("agent", i18n.t("remote.chat.planRevisionRequested"));
          store.planFeedbackSent();
          const sent = await client.send({
            type: "plan_feedback",
            sessionId: id,
            clientTurnId,
            text: payload.text,
            attachments: payload.attachments.map((attachment) => attachment.id),
            mentions: payload.mentions,
          });
          if (!sent) {
            store.errorReceived(i18n.t("remote.chat.planRevisionFailed"));
            restore();
          }
          return;
        }
        store.log("you", payload.text, undefined, payload.attachments, payload.mentions, clientTurnId);
        store.chatSent();
        const sent = await client.send({
          type: "chat",
          sessionId: id,
          clientTurnId,
          text: payload.text,
          attachments: payload.attachments.map((attachment) => attachment.id),
          mentions: payload.mentions,
          executionMode: payload.executionMode,
          clarify: payload.clarify,
          ...(payload.skillSelections === undefined
            ? {}
            : { skillSelections: { ...payload.skillSelections } }),
        });
        if (!sent) {
          store.errorReceived(i18n.t("remote.chat.requestFailedShort"));
          restore();
        }
      } catch (error) {
        restore();
        store.errorReceived(String(error));
        store.log("error", i18n.t("remote.chat.requestFailed", { error: String(error) }));
      }
    },
    [store],
  );

  /** Send a message composed while the turn runs to that turn; without a
   * running turn it is sent as an ordinary message once the turn has ended. */
  const steer = useCallback(
    async (payload: SteerPayload) => {
      const id = sessionIdRef.current;
      if (id === null) {
        await send(steerAsChat(payload));
        return;
      }
      participatedRef.current = true;
      const steerId = crypto.randomUUID();
      store.steerSent({ steerId, ...payload });
      try {
        // A lost answer is sent again under the same request id, so the PC
        // never pushes it twice; when the page gives up the conversation is
        // reloaded from the PC, which knows whether it arrived.
        const result = (await invokeResending(
          invoke,
          "chat_steer",
          {
            sessionId: id,
            steerId,
            text: payload.text,
            attachments: payload.attachments.map((attachment) => attachment.id),
            mentions: payload.mentions,
          },
          {
            onLost: () => setRecovering(true),
            waitForOpen: () => remoteTransport().waitForOpen(),
            onRecovered: () => void recoveredRef.current(),
          },
        )) as SteerPushResult | null;
        if (result !== "no_turn") return;
        await whenTurnEnds(store);
        const [unsent] = store.steerTaken([steerId]);
        if (unsent === undefined) return;
        if (steerReturnsToComposer(store.getState())) setDraft(steerDraft([unsent]));
        else await send(steerAsChat(unsent));
      } catch (error) {
        const returned = store.steerTaken([steerId]);
        if (returned.length > 0) setDraft(steerDraft(returned));
        store.log("error", i18n.t("remote.chat.steerFailed", { error: String(error) }));
      }
    },
    [send, store],
  );

  const cancellable =
    sessionId !== null &&
    (isBusyPhase(state.phase) ||
      activity === "running_read" ||
      activity === "running_write" ||
      activity === "waiting_input");

  const cancel = useCallback(async () => {
    const id = sessionIdRef.current;
    const client = clientRef.current;
    if (id === null || !client || actionBusy) return;
    participatedRef.current = true;
    setActionBusy(true);
    try {
      if (await client.send({ type: "cancel", sessionId: id })) {
        store.cancelSent();
      } else {
        store.errorReceived(i18n.t("remote.chat.stopFailed"));
        store.log("error", i18n.t("remote.chat.stopFailed"));
      }
    } finally {
      setActionBusy(false);
    }
  }, [actionBusy, store]);

  const approvePlan = useCallback(async () => {
    const id = sessionIdRef.current;
    const client = clientRef.current;
    if (id === null || !client || store.getState().phase !== "plan_review") return;
    participatedRef.current = true;
    const revision = store.getState().plan?.revision;
    store.log(
      "agent",
      revision !== undefined ? i18n.t("remote.chat.planApprovedRev", { revision: revision }) : i18n.t("remote.chat.planApproved"),
    );
    store.planApproveSent();
    setPlanOpen(false);
    if (!(await client.send({ type: "plan_approve", sessionId: id }))) {
      store.errorReceived(i18n.t("remote.chat.planApproveFailed"));
    }
  }, [store]);

  const answerAsk = useCallback(
    async (answers: Record<string, AskAnswer>) => {
      const id = sessionIdRef.current;
      const client = clientRef.current;
      const ask = store.getState().ask;
      if (id === null || !client || !ask || ask.submitting) return;
      participatedRef.current = true;
      store.askSubmitStarted();
      const sent = await client.send({
        type: "ask_response",
        sessionId: id,
        requestId: ask.requestId,
        answers,
      });
      if (sent) store.askAnswered();
      else store.askSubmitFailed();
    },
    [store],
  );

  const changeModel = useCallback(
    async (model: string, reasoning: ReasoningSelection | undefined) => {
      // A select whose options changed under it reports an empty level; that
      // is not a choice.
      if (reasoning !== undefined && !reasoning.level) return;
      const id = sessionIdRef.current;
      if (id === null) {
        // Kept on the phone until the conversation exists; the PC's default
        // model for new conversations stays as it is.
        draftModelRef.current = { model, ...(reasoning ? { reasoning } : {}) };
        setModelSettings((current) =>
          current === null
            ? null
            : {
                provider: current.provider,
                models: current.models,
                selectedModel: model,
                ...(reasoning ? { selectedReasoning: reasoning } : {}),
              },
        );
        return;
      }
      setModelBusy(true);
      try {
        setModelSettings(await sessionModelSettingsSave(id, model, reasoning));
      } catch {
        toast.error(i18n.t("remote.chat.modelSaveFailed"));
      } finally {
        setModelBusy(false);
      }
    },
    [],
  );

  const build = useCallback(async () => {
    setBuilding(true);
    try {
      setBuildReport(await runProjectBuild());
      setBuildError(null);
    } catch (reason) {
      setBuildReport(null);
      setBuildError(String(reason));
    } finally {
      setBuilding(false);
      setBuildOpen(true);
    }
  }, []);

  const handleMentionSearch = useCallback(
    (request: MentionSearchRequest) => mentionSearch(request),
    [],
  );

  const name = record?.name ?? t("remote.chat.newConversation");
  const activityKey = ACTIVITY_KEYS[activity];
  const activityText = activityKey ? t(activityKey) : undefined;
  const ask = state.ask;

  return (
    <div className="flex h-full min-w-0 flex-col overflow-hidden bg-background text-foreground">
      <header className="flex min-h-14 shrink-0 items-center gap-1 border-b border-border pl-2 pr-4">
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="size-11 shrink-0"
          aria-label={t("remote.chat.backToList")}
          onClick={onBack}
        >
          <ChevronLeft aria-hidden className="size-5" />
        </Button>
        <div className="min-w-0 flex-1">
          <h1 className="truncate text-sm font-semibold" title={name}>
            {name}
          </h1>
          <p className="truncate text-xs text-muted-foreground">
            {projectName}
            {activityText ? ` · ${activityText}` : ""}
          </p>
        </div>
        {state.plan !== null && (
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="size-11 shrink-0"
            aria-label={t("remote.chat.viewPlan", { revision: state.plan.revision })}
            onClick={() => setPlanOpen(true)}
          >
            <ClipboardList aria-hidden className="size-5" />
          </Button>
        )}
        {cancellable && (
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="size-11 shrink-0"
            aria-label={t("remote.chat.stop")}
            disabled={actionBusy}
            onClick={() => void cancel()}
          >
            <Square aria-hidden className="size-4" />
          </Button>
        )}
        <Button
          type="button"
          variant="outline"
          className="min-h-11 shrink-0 gap-1.5 px-3"
          disabled={building}
          onClick={() => void build()}
        >
          {building ? (
            <LoaderCircle aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
          ) : (
            <Hammer aria-hidden className="size-4" />
          )}
          {t("remote.chat.build")}
        </Button>
      </header>

      {recovering && (
        <p
          role="status"
          className="flex shrink-0 items-center gap-2 border-b border-border px-4 py-2 text-xs text-muted-foreground"
        >
          <LoaderCircle aria-hidden className="size-3.5 animate-spin motion-reduce:animate-none" />
          {recoveryNote()}
        </p>
      )}
      <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
        <ConversationLog
          log={state.log}
          phase={state.phase}
          turn={state.turn}
          pendingSteers={state.pendingSteers}
          ragLoading={state.rag === "loading"}
          onSuggestion={(text) => {
            if (!store.getState().canSend) return;
            void send({ text, attachments: [], mentions: [], executionMode: "interactive" });
          }}
          suggestionsEnabled={state.canSend && !actionBusy}
        />
        {ask && (
          <AskCard
            key={ask.requestId}
            requestId={ask.requestId}
            questions={ask.questions}
            submitting={ask.submitting}
            kind={ask.kind}
            waitSeconds={ask.waitSeconds}
            receivedAt={ask.receivedAt}
            onSubmit={(answers) => void answerAsk(answers)}
            draft={{
              value: ask.draft ?? EMPTY_ASK_DRAFT,
              onChange: (value) => store.askDraftChanged(ask.requestId, value),
            }}
          />
        )}
      </div>

      <div className="shrink-0 pb-[env(safe-area-inset-bottom)]">
        <InstructionBox
          state={state}
          onSend={(payload) => void send(payload)}
          onSteer={(payload) => void steer(payload)}
          onMentionSearch={handleMentionSearch}
          projectIdentity={state.project}
          scopeIdentity={sessionId ?? `draft:${state.project}`}
          onStageAttachment={stageAttachment}
          onDiscardAttachment={discardAttachment}
          onCancel={() => void cancel()}
          draft={draft}
          actionBusy={actionBusy}
          modelSettings={modelSettings}
          modelSettingsBusy={modelBusy}
          onModelSettingsChange={(model, reasoning) => void changeModel(model, reasoning)}
          onModelSettingsReload={() => {
            if (sessionId !== null) {
              void sessionModelSettingsGet(sessionId).then(setModelSettings);
            } else if (draftModelRef.current === null) {
              void draftModelSettings()
                .then(setModelSettings)
                .catch(() => {});
            }
          }}
          skills={skills}
          skillsAvailable
        />
      </div>

      <Dialog open={planOpen && state.plan !== null} onOpenChange={setPlanOpen}>
        <DialogContent className="flex max-h-[90dvh] w-[calc(100vw-2rem)] max-w-lg flex-col overflow-hidden p-0">
          <DialogHeader className="px-4 pt-4">
            <DialogTitle>{t("remote.chat.planReviewTitle")}</DialogTitle>
            <DialogDescription>{t("remote.chat.planReviewDesc")}</DialogDescription>
          </DialogHeader>
          {state.plan !== null && (
            <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
              <PlanView
                plan={state.plan}
                reviewable={state.phase === "plan_review"}
                pending={actionBusy}
                onApprove={() => void approvePlan()}
              />
            </div>
          )}
        </DialogContent>
      </Dialog>

      <BuildResultDialog
        open={buildOpen}
        report={buildReport}
        error={buildError}
        onOpenChange={setBuildOpen}
      />
    </div>
  );
}
