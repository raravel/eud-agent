/**
 * One EPS conversation on the phone: the desktop's conversation log, composer
 * (attachments, skills, interview, model), question card, plan review and
 * build result, over the remote transport. It follows one session; the
 * desktop's split panes, queue and message editing stay on the PC.
 */
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
import { InstructionBox, type ChatPayload } from "@/components/InstructionBox";
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
  runProjectBuild,
  sessionModelSettingsGet,
  sessionModelSettingsSave,
  skillList,
  type AskAnswer,
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
import { RESYNC_EVENT } from "@/lib/remoteTransport";
import { invoke, listen } from "@/lib/transport";
import { providerChatErrorText } from "@/providers/providerCopy";
import { EMPTY_ASK_DRAFT } from "@/state/askDraft";
import { emptyPanelLog, serializePanelLog } from "@/state/panelLog";
import { createPanelStore, isBusyPhase, type PanelStore } from "@/state/store";

interface PendingAsk {
  sessionId: string;
  requestId: string;
  questions: AskQuestion[];
  waitSeconds?: number;
}

const ACTIVITY_TEXT: Partial<Record<SessionActivity, string>> = {
  running_read: "분석 중",
  running_write: "변경 중",
  waiting_input: "응답 필요",
  review: "검토 필요",
};

export interface RemoteChatProps {
  /** The session to show; `null` starts a new conversation on first send. */
  sessionId: string | null;
  projectName: string;
  onBack(): void;
}

/** Route one app message to the conversation's store. */
function applyMessage(
  store: PanelStore,
  sessionId: string | null,
  msg: ServerMessage,
  setActivity: (activity: SessionActivity) => void,
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
      else store.askReceived(msg.requestId, msg.questions, msg.waitSeconds);
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
            `맵 에이전트가 후보 r${msg.task.candidate?.revision ?? "?"}을 만들었습니다. AI가 이어서 검토합니다. PC의 맵 창에서 직접 적용하거나 폐기할 수도 있습니다.`,
          );
        } else if (msg.task.status.kind === "applied") {
          store.log("agent", "맵 후보가 적용되었습니다. 이어서 진행할 수 있습니다.");
        } else if (msg.task.status.kind === "failed") {
          store.log("warn", `맵 작업을 완료하지 못했습니다: ${msg.task.status.reason}`);
        }
      }
      if (msg.continuation) store.chatSent();
      return;
    }
    case "worker":
      if (msg.continuation) store.chatSent();
      return;
    case "plan": {
      const prior = store.getState().plan;
      if (prior !== null && prior.revision !== msg.revision) {
        store.log("agent", `계획안(rev ${prior.revision})이 갱신되었습니다.`);
      }
      store.planReceived(msg.markdown, msg.revision);
      store.log("agent", `계획안(rev ${msg.revision})이 도착했습니다.`);
      return;
    }
    case "git":
      store.gitCommitted(msg);
      return;
    case "error":
      store.errorReceived(msg.message);
      store.log("error", `오류: ${providerChatErrorText(msg.message)}`);
      return;
    case "session_activity":
      if (msg.activity === "running_write") {
        store.log("ok", "격리 워크스페이스에서 변경을 시작합니다.");
      }
      setActivity(msg.activity);
      return;
    default:
      return;
  }
}

export function RemoteChat({ sessionId: initialSessionId, projectName, onBack }: RemoteChatProps) {
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

  // One client for the screen's life; it delivers to whichever store is current.
  useEffect(() => {
    const client = new IpcClient({
      onMessage: (msg) => applyMessage(storeRef.current, sessionIdRef.current, msg, setActivity),
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
    void listen(RESYNC_EVENT, () => setGeneration((value) => value + 1)).then((fn) => {
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
  }, []);

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
        store.hydrate(loaded.panelLog ?? emptyPanelLog());
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
          store.askReceived(pending.requestId, pending.questions, pending.waitSeconds);
        }
      } catch (error) {
        if (active) store.log("error", `대화를 불러오지 못했습니다: ${String(error)}`);
      }
    })();
    return () => {
      active = false;
    };
  }, [store]);

  useEffect(() => {
    if (sessionId === null) {
      setModelSettings(null);
      return;
    }
    let active = true;
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

  // The conversation log is saved with the session, as the desktop does.
  useEffect(() => {
    let observed = store.getState().log;
    let timer: number | undefined;
    const unsubscribe = store.subscribe((snapshot) => {
      if (snapshot.log === observed) return;
      observed = snapshot.log;
      const id = sessionIdRef.current;
      if (id === null) return;
      if (timer !== undefined) window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        void invoke("session_update_log", {
          id,
          panelLog: serializePanelLog(store.getState().log),
        }).catch(() => {
          // Best-effort; the next change saves again.
        });
      }, 500);
    });
    return () => {
      unsubscribe();
      if (timer !== undefined) window.clearTimeout(timer);
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
      const compact =
        payload.text.trim() === "/compact" &&
        payload.attachments.length === 0 &&
        payload.mentions.length === 0;
      try {
        if (compact) {
          const id = sessionIdRef.current;
          if (id === null) {
            toast.error("압축할 대화가 없습니다. 먼저 메시지를 보내 주세요.");
            return;
          }
          setActionBusy(true);
          store.log("info", "대화 컨텍스트 압축 중…");
          try {
            await compactSession(id);
            store.log("ok", "대화 컨텍스트를 압축했습니다.");
          } catch (error) {
            store.log("error", `대화 컨텍스트를 압축하지 못했습니다: ${String(error)}`);
          } finally {
            setActionBusy(false);
          }
          return;
        }
        let id = sessionIdRef.current;
        if (id === null) {
          const seed =
            payload.text.trim() ||
            (payload.mentions.length > 0 ? "리소스 멘션 요청" : "첨부 파일 분석");
          const created = await invoke<SessionRecord>("session_create", { firstText: seed });
          id = created.id;
          sessionIdRef.current = id;
          setSessionId(id);
          setRecord(created);
        }
        if (store.getState().phase === "plan_review") {
          store.log("you", payload.text, undefined, payload.attachments, payload.mentions, clientTurnId);
          store.log("agent", "계획 수정을 요청했습니다.");
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
            store.errorReceived("계획 수정 요청을 처리하지 못했습니다.");
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
          store.errorReceived("요청을 처리하지 못했습니다.");
          restore();
        }
      } catch (error) {
        restore();
        store.errorReceived(String(error));
        store.log("error", `요청을 처리하지 못했습니다: ${String(error)}`);
      }
    },
    [store],
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
    setActionBusy(true);
    try {
      if (await client.send({ type: "cancel", sessionId: id })) {
        store.cancelSent();
      } else {
        store.errorReceived("작업을 중단하지 못했습니다.");
        store.log("error", "작업을 중단하지 못했습니다.");
      }
    } finally {
      setActionBusy(false);
    }
  }, [actionBusy, store]);

  const approvePlan = useCallback(async () => {
    const id = sessionIdRef.current;
    const client = clientRef.current;
    if (id === null || !client || store.getState().phase !== "plan_review") return;
    const revision = store.getState().plan?.revision;
    store.log(
      "agent",
      revision !== undefined ? `계획안(rev ${revision})을 승인했습니다.` : "계획을 승인했습니다.",
    );
    store.planApproveSent();
    setPlanOpen(false);
    if (!(await client.send({ type: "plan_approve", sessionId: id }))) {
      store.errorReceived("계획 승인 요청을 처리하지 못했습니다.");
    }
  }, [store]);

  const answerAsk = useCallback(
    async (answers: Record<string, AskAnswer>) => {
      const id = sessionIdRef.current;
      const client = clientRef.current;
      const ask = store.getState().ask;
      if (id === null || !client || !ask || ask.submitting) return;
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
      const id = sessionIdRef.current;
      if (id === null) return;
      setModelBusy(true);
      try {
        setModelSettings(await sessionModelSettingsSave(id, model, reasoning));
      } catch {
        toast.error("모델 설정을 저장하지 못했습니다.");
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

  const name = record?.name ?? "새 대화";
  const activityText = ACTIVITY_TEXT[activity];
  const ask = state.ask;

  return (
    <div className="flex h-full min-w-0 flex-col overflow-hidden bg-background text-foreground">
      <header className="flex min-h-14 shrink-0 items-center gap-1 border-b border-border pl-2 pr-4">
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="size-11 shrink-0"
          aria-label="대화 목록으로"
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
            aria-label={`계획 보기 (rev ${state.plan.revision})`}
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
            aria-label="중단"
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
          빌드
        </Button>
      </header>

      <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
        <ConversationLog
          log={state.log}
          phase={state.phase}
          turn={state.turn}
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
            if (sessionId !== null) void sessionModelSettingsGet(sessionId).then(setModelSettings);
          }}
          skills={skills}
          skillsAvailable
        />
      </div>

      <Dialog open={planOpen && state.plan !== null} onOpenChange={setPlanOpen}>
        <DialogContent className="flex max-h-[90dvh] w-[calc(100vw-2rem)] max-w-lg flex-col overflow-hidden p-0">
          <DialogHeader className="px-4 pt-4">
            <DialogTitle>계획 검토</DialogTitle>
            <DialogDescription>승인하면 이 계획대로 작업을 시작합니다. 수정할 점은 입력창에 적어 보내 주세요.</DialogDescription>
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
