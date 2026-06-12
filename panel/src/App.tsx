/**
 * Panel app shell (v2) - wires the Tauri IPC v2 client + state store to the
 * chat-first review UI (features/06_changeset-review-panel.md).
 *
 * Components: a status-rich Header (connection transitions + RAG state/elapsed),
 * the ConversationLog cards, a live AgentStream under the turn, the PlanView
 * feedback/approve surface, the ChangesetView accept/reject surface, and the
 * regated InstructionBox. Plan cards are archived into the conversation log as
 * agent entries when a plan arrives, is superseded by a higher revision, or is
 * approved.
 *
 * Data flow: IpcClient (Tauri invoke + listen) -> store actions + log entries
 * -> React snapshot via useSyncExternalStore -> components -> user intents call
 * client.send + the matching store action. Two pieces of UI-only state live here
 * (not protocol state): the current turn's agent_event list (for AgentStream)
 * and the RAG warmup state/timing (for the Header pill).
 */
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { Header, type RagState } from "@/components/Header";
import { ConversationLog } from "@/components/ConversationLog";
import { ChangesetView } from "@/components/ChangesetView";
import { PlanView } from "@/components/PlanView";
import { MemoryView } from "@/components/MemoryView";
import { InstructionBox, type ChatPayload } from "@/components/InstructionBox";
import { ConnectionNotice } from "@/components/ConnectionNotice";
import { createPanelStore } from "@/state/store";
import {
  IpcClient,
  type MemoryFile,
  type ServerMessage,
  type SetupMessage,
} from "@/lib/ipc";
import { progressLabel } from "@/lib/progress";
import {
  bootstrapView,
  type BootstrapView,
} from "@/setup/bootstrap";
import { SetupScreen } from "@/setup/SetupScreen";
import { UpdateNotice } from "@/components/UpdateNotice";
import { createUpdater, type UpdateHandle } from "@/setup/update";
import { invoke } from "@tauri-apps/api/core";

/** codex login probe result (mirrors the Rust `CodexAuthState`). */
interface CodexAuthState {
  resolved: boolean;
  authed: boolean;
  detail: string;
}

/** OAuth poll cadence + ceiling: codex's browser flow rarely exceeds a minute. */
const CODEX_POLL_MS = 2000;
const CODEX_POLL_TIMEOUT_MS = 180000;

/**
 * Editor-liveness poll cadence. The bridge writes heartbeat.txt every ~1s and
 * the core treats a >3s-old heartbeat as stale, so a 2s probe recovers a downed
 * editor within ~1 cycle without churning the file IPC (the probe is the cheap
 * status-only path; the heavier `list` round-trip runs only on edges).
 */
const EDITOR_POLL_MS = 2000;

interface BootstrapState {
  active: boolean;
  view: BootstrapView;
  error: string | null;
}

export default function App() {
  // Store + client live for the lifetime of the app (created once).
  const store = useMemo(() => createPanelStore(), []);
  const clientRef = useRef<IpcClient | null>(null);
  // Self-update seam (Decision 04). Created once; the real plugin in prod, swappable
  // in tests. The check runs once after first-run setup is satisfied.
  const updater = useMemo(() => createUpdater(), []);

  // Subscribe React to the framework-agnostic store.
  const state = useSyncExternalStore(store.subscribe, store.getState, store.getState);

  // ---- UI-only state (not protocol state) ----
  // The per-turn streaming buffers (reasoning / answer / tools) live in the STORE
  // (state.turn) now — the AgentStream + live AgentAnswer render from there, and
  // the store resets them per turn. No App-local agent_event list is needed.
  // RAG warmup visibility for the Header pill. `startedAt` drives the elapsed
  // counter while loading; a 1s tick re-renders so the seconds advance.
  const [ragState, setRagState] = useState<RagState>("idle");
  const ragStartRef = useRef<number | null>(null);
  const [ragElapsedSec, setRagElapsedSec] = useState(0);
  const [bootstrap, setBootstrap] = useState<BootstrapState>(() => ({
    active: false,
    view: bootstrapView(null, undefined),
    error: null,
  }));
  const bootstrapActiveRef = useRef(false);
  // First-run manifest check (EUD-132). null until the first `setup` snapshot
  // arrives; setup_required routes the whole panel to the SetupScreen.
  const [setup, setSetup] = useState<SetupMessage | null>(null);
  const bootstrapRunningRef = useRef(false);
  // Editor-liveness poll gate. Flips true once first-run setup is satisfied (or
  // a failed setup_status falls back to "assume configured"); the poll effect
  // then probes the editor every EDITOR_POLL_MS so a stale heartbeat at boot or
  // a mid-session editor restart recovers automatically.
  const [editorPollEnabled, setEditorPollEnabled] = useState(false);
  // codex login step (setup screen step 3). The OAuth path spawns the browser
  // flow in the backend and polls codex_login_status until it flips.
  const [codexBusy, setCodexBusy] = useState(false);
  const [codexError, setCodexError] = useState<string | null>(null);
  const codexPollRef = useRef<number | null>(null);
  // Self-update banner state: the pending update (null until found) and a
  // session-scoped "나중에" dismissal. The check fires once (guarded by the ref).
  const [update, setUpdate] = useState<UpdateHandle | null>(null);
  const [updateDismissed, setUpdateDismissed] = useState(false);
  const updateCheckedRef = useRef(false);

  useEffect(() => {
    bootstrapActiveRef.current = bootstrap.active;
  }, [bootstrap.active]);

  // Tick the RAG elapsed counter once a second while loading.
  useEffect(() => {
    if (ragState !== "loading") return;
    const id = setInterval(() => {
      if (ragStartRef.current !== null) {
        setRagElapsedSec((Date.now() - ragStartRef.current) / 1000);
      }
    }, 1000);
    return () => clearInterval(id);
  }, [ragState]);

  // Dispatch an inbound v2 server message to store actions + log entries.
  const onMessage = useCallback(
    (msg: ServerMessage) => {
      switch (msg.type) {
        case "status":
          store.applyStatus({ compiling: msg.compiling, project: msg.project });
          break;
        case "list":
          store.applyList({ files: msg.files, error: msg.error });
          break;
        case "memory":
          store.memoryReceived(msg.project, msg.files, msg.episodes);
          break;
        case "memory_saved":
          store.memorySaved(msg.file);
          store.log("ok", "메모리를 저장했습니다.");
          break;
        case "setup":
          setSetup(msg);
          if (!msg.setup_required) {
            // Setup finished (or was never needed): drop the overlay and arm the
            // editor-liveness poll (which pulls the first status/list snapshot).
            bootstrapActiveRef.current = false;
            setBootstrap((prev) =>
              prev.active ? { ...prev, active: false } : prev,
            );
            setEditorPollEnabled(true);
          }
          break;
        case "progress": {
          if (msg.stage === "bootstrap") {
            // Final "done" closes the overlay; the fresh setup snapshot flips
            // setup_required off and refreshes the status/list snapshots.
            if (msg.detail === "done") {
              bootstrapActiveRef.current = false;
              setBootstrap((prev) => ({ ...prev, active: false, error: null }));
              void clientRef.current?.send({ type: "setup_status" });
              break;
            }
            const view = bootstrapView(msg.pct, msg.detail);
            bootstrapActiveRef.current = true;
            setBootstrap({
              active: true,
              view,
              error: view.phase === "error" ? view.label : null,
            });
            break;
          }
          if (bootstrapActiveRef.current) {
            bootstrapActiveRef.current = false;
            setBootstrap((prev) => ({ ...prev, active: false }));
          }
          store.progressReceived(msg.stage);
          // RAG warmup drives the Header pill (started → loading w/ elapsed,
          // done → ready, error → unavailable) AND the store send gate. The
          // core replays the current warmup state to every new client,
          // so transitions are logged only on a real change (a fresh "done"
          // snapshot must not re-log completion), and the loading state
          // is NOT logged at all — the ConversationLog shimmer row covers it.
          if (msg.stage === "rag_warmup") {
            const prev = store.getState().rag;
            let next: "loading" | "ready" | "unavailable";
            if (msg.detail === "done") {
              next = "ready";
            } else if (msg.detail !== undefined && msg.detail.startsWith("error")) {
              next = "unavailable";
            } else {
              next = "loading";
            }
            store.ragWarmupChanged(next);
            if (next !== prev && next !== "loading") {
              const { kind, text } = progressLabel(msg.stage, msg.detail);
              store.log(kind, text, msg.stage);
            }
            if (next === "loading") {
              ragStartRef.current = Date.now();
              setRagElapsedSec(0);
            }
            setRagState(next);
            break;
          }
          const { kind, text } = progressLabel(msg.stage, msg.detail);
          store.log(kind, text, msg.stage);
          break;
        }
        case "agent_event":
          // Accumulated into the store's per-turn buffers (reasoning/answer/tools);
          // raw kinds never reach the log. `data` carries tool args/result (EUD-068).
          store.agentEvent(msg.kind, msg.detail, msg.data);
          break;
        case "answer":
          // answerReceived archives the turn itself: the streamed blocks land
          // in the log in arrival order (tools/prose interleaved), and the
          // final text is logged only when no prose was streamed — logging
          // msg.text here again would duplicate the streamed prose.
          store.answerReceived(msg.text);
          break;
        case "plan": {
          // F2: planReceived archives any prose streamed via `delta` before this
          // turn-end (the live AgentAnswer renders turn.answer only while
          // thinking, so it would otherwise vanish at the transition).
          // Archive the prior plan card before it is replaced: a higher revision
          // supersedes the active card (the store keeps only the latest), so log
          // the supersession so the iteration history stays in the conversation.
          const prior = store.getState().plan;
          if (prior !== null && prior.revision !== msg.revision) {
            store.log("agent", `계획안(rev ${prior.revision})이 갱신되었습니다.`);
          }
          store.planReceived(msg.markdown, msg.revision);
          store.log("agent", `계획안(rev ${msg.revision})이 도착했습니다.`);
          break;
        }
        case "changeset":
          // F2: changesetReceived archives any prose streamed before this turn-end.
          store.changesetReceived(msg.request_id, msg.items);
          store.log("agent", `변경사항 ${msg.items.length}건을 검토하세요.`);
          break;
        case "rollback_result": {
          // Read the recorded decision BEFORE rollbackResult() clears it — the
          // inbound message has no accept/reject discriminator, so the log label
          // (적용 유지 vs 되돌림) must come from what the user chose.
          const decision = store.getState().pendingDecision?.decision;
          const count = msg.ids.length;
          store.rollbackResult(msg.ids, msg.ok);
          if (decision === "accept") {
            store.log("ok", count > 0 ? `적용 유지 (${count}건)` : "적용 유지");
          } else if (msg.ok) {
            store.log("ok", `되돌림 (${count}건)`);
          } else {
            store.log("warn", `되돌리기 일부 실패 (${count}건)`);
          }
          break;
        }
        case "error":
          // F2: errorReceived archives any prose streamed before the turn errored.
          if (bootstrapActiveRef.current) {
            setBootstrap((prev) => ({
              ...prev,
              active: true,
              error: msg.message,
            }));
          }
          store.errorReceived(msg.message);
          store.log("error", `오류: ${msg.message}`);
          break;
        default:
          break;
      }
    },
    [store],
  );

  // Boot the IPC client once. Lifecycle maps to the store phases; logs flow
  // through store.log so they render in the conversation.
  useEffect(() => {
    store.wsConnecting();
    const client = new IpcClient({
      onMessage,
      onLog: (kind, text) => {
        if (kind === "info") store.log("info", text);
        else store.log("warn", text); // unknown / bad payload
      },
      onOpenChange: (open) => {
        if (open) store.wsOpen();
        else store.wsError();
      },
      onEditorChange: (connected) => {
        // Editor liveness is separate from the transport: this only flips the
        // send gate + ConnectionNotice banner, never the reconnect UI. Log just
        // the recovery edge (the down state is shown by the banner, and logging
        // it would misleadingly read as "disconnected" when the editor simply
        // was not up yet at boot).
        const wasConnected = store.getState().editorConnected;
        store.editorConnectionChanged(connected);
        if (connected && !wasConnected) {
          store.log("ok", "에디터에 다시 연결되었습니다.");
        }
      },
    });
    clientRef.current = client;
    // Register push listeners first (bootstrap progress must not be missed),
    // then route by the first-run manifest check: setup_required renders the
    // SetupScreen without ever requesting the doomed status/list snapshot (no
    // misleading "connect failed" log); the ready path pulls the snapshot via
    // refresh() from the `setup` handler below.
    void client.connect().then(() =>
      client.send({ type: "setup_status" }).then((ok) => {
        // Unexpected setup_status failure: assume an already-configured app and
        // arm the editor poll so it still comes up.
        if (!ok) setEditorPollEnabled(true);
      }),
    );
    return () => {
      client.stop();
      clientRef.current = null;
    };
  }, [store, onMessage]);

  // Editor-liveness poll. Once armed (first-run setup satisfied), probe the
  // editor every EDITOR_POLL_MS. The transport stays open throughout, so this
  // only drives editorConnected (send gate + ConnectionNotice) and recovers a
  // stale-heartbeat-at-boot or a mid-session editor restart with no user action.
  useEffect(() => {
    if (!editorPollEnabled) return;
    const client = clientRef.current;
    if (!client) return;
    let cancelled = false;
    const probe = () => {
      if (!cancelled) void client.refresh();
    };
    probe(); // immediate — no EDITOR_POLL_MS dead window before the first probe
    const id = window.setInterval(probe, EDITOR_POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, [editorPollEnabled]);

  // Setup flow, download step: once the editor folder is picked (or was already
  // configured) and assets are still missing, start the bootstrap download.
  // Progress streams in as `progress {stage: "bootstrap"}`; the final "done"
  // re-queries setup_status, which dismisses the SetupScreen.
  useEffect(() => {
    if (!setup?.setup_required || !setup.editor_valid || setup.assets_ready) return;
    if (bootstrapRunningRef.current) return;
    bootstrapRunningRef.current = true;
    void clientRef.current?.send({ type: "bootstrap_run" }).then(() => {
      bootstrapRunningRef.current = false;
    });
  }, [setup]);

  // Once first-run setup is satisfied, check for an app self-update exactly once.
  // Non-blocking: an updater error (offline, no release yet) just leaves the banner
  // hidden — it never gates the panel.
  useEffect(() => {
    if (!setup || setup.setup_required) return;
    if (updateCheckedRef.current) return;
    updateCheckedRef.current = true;
    void updater
      .check()
      .then((found) => {
        if (found) setUpdate(found);
      })
      .catch(() => {
        /* no release / offline — no banner */
      });
  }, [setup, updater]);

  // ---- user intents ----
  // The MAIN prompt input routes by phase (EUD-074): during plan_review the
  // typed text IS the plan feedback (plan_feedback{text} — the PlanView
  // feedback textarea is removed); otherwise it starts a chat turn.
  //
  // Turn-starting commands resolve only when the WHOLE codex turn ends, while
  // its progress/answer events stream in the meantime — so the user bubble and
  // the thinking phase are recorded BEFORE awaiting, or the answer would render
  // above the user's own message and the late phase flip would strand the UI
  // in "생각하는 중…" after the turn already finished.
  const handleSend = useCallback(
    async (payload: ChatPayload) => {
      if (store.getState().phase === "plan_review") {
        store.log("you", payload.text);
        store.log("agent", "계획 수정을 요청했습니다.");
        store.planFeedbackSent();
        const sent = await clientRef.current?.send({
          type: "plan_feedback",
          text: payload.text,
        });
        if (!sent) {
          store.errorReceived("계획 수정 요청을 처리하지 못했습니다.");
        }
        return;
      }
      store.log("you", payload.text);
      store.chatSent(); // a new turn — the store resets the per-turn buffers.
      const sent = await clientRef.current?.send({
        type: "chat",
        text: payload.text,
      });
      if (!sent) {
        // The send failure detail is already logged by the client (onLog);
        // this returns the phase to ready so the input is usable again.
        store.errorReceived("요청을 처리하지 못했습니다.");
      }
    },
    [store],
  );

  // Empty-conversation suggestion chip → the same chat path as the
  // InstructionBox (the chips render only in the ready phase, so this never
  // routes to plan_feedback). Guarded by canSend in case gating flipped
  // between render and click.
  const handleSuggestion = useCallback(
    (text: string) => {
      if (!store.getState().canSend) return;
      void handleSend({ text });
    },
    [store, handleSend],
  );

  // New conversation: send reset{} (the server drops the retained codex thread,
  // EUD-064) and clear the client log / plan / changeset / per-turn buffers.
  const handleReset = useCallback(async () => {
    const sent = await clientRef.current?.send({ type: "reset" });
    if (sent) {
      store.resetSent();
    }
  }, [store]);

  // Retry re-runs the backend download command (it re-fetches the release
  // manifest and skips already-verified assets), replacing the old full-reload
  // fallback from before bootstrap_run existed.
  const handleBootstrapRetry = useCallback(() => {
    if (bootstrapRunningRef.current) return;
    setBootstrap((prev) => ({
      ...prev,
      error: null,
      view: bootstrapView(null, undefined),
    }));
    bootstrapRunningRef.current = true;
    void clientRef.current?.send({ type: "bootstrap_run" }).then(() => {
      bootstrapRunningRef.current = false;
    });
  }, []);

  const handlePickEditorPath = useCallback(() => {
    void clientRef.current?.send({ type: "setup_pick_editor_path" });
  }, []);

  // Re-query the setup gate after a login attempt so codex_authed (and thus
  // setup_required) refreshes and the SetupScreen dismisses on success.
  const refreshSetup = useCallback(() => {
    void clientRef.current?.send({ type: "setup_status" });
  }, []);

  const stopCodexPoll = useCallback(() => {
    if (codexPollRef.current !== null) {
      window.clearInterval(codexPollRef.current);
      codexPollRef.current = null;
    }
  }, []);

  // Install: the backend downloads the standalone codex binary and places it
  // where resolve_codex_cmd finds it; refreshing the gate flips codex_resolved
  // and the login controls take over.
  const handleCodexInstall = useCallback(() => {
    stopCodexPoll();
    setCodexError(null);
    setCodexBusy(true);
    void invoke<CodexAuthState>("codex_install")
      .then((state) => {
        setCodexBusy(false);
        refreshSetup();
        if (!state.resolved) {
          setCodexError("codex 설치 후에도 실행 파일을 찾지 못했습니다.");
        }
      })
      .catch((error) => {
        setCodexBusy(false);
        setCodexError(String(error));
      });
  }, [refreshSetup, stopCodexPoll]);

  // OAuth: the backend launches `codex login` (opens the browser); we poll
  // codex_login_status until it reports authed, then refresh the gate.
  const handleCodexOAuth = useCallback(() => {
    stopCodexPoll();
    setCodexError(null);
    setCodexBusy(true);
    void invoke("codex_login_start")
      .then(() => {
        const startedAt = Date.now();
        codexPollRef.current = window.setInterval(() => {
          void invoke<CodexAuthState>("codex_login_status")
            .then((state) => {
              if (state.authed) {
                stopCodexPoll();
                setCodexBusy(false);
                refreshSetup();
              } else if (Date.now() - startedAt > CODEX_POLL_TIMEOUT_MS) {
                stopCodexPoll();
                setCodexBusy(false);
                setCodexError(
                  "로그인이 완료되지 않았습니다. 브라우저에서 인증을 마친 뒤 다시 시도해 주세요.",
                );
              }
            })
            .catch((error) => {
              stopCodexPoll();
              setCodexBusy(false);
              setCodexError(String(error));
            });
        }, CODEX_POLL_MS);
      })
      .catch((error) => {
        setCodexBusy(false);
        setCodexError(String(error));
      });
  }, [refreshSetup, stopCodexPoll]);

  // API key: piped to the backend (stdin), awaited; success refreshes the gate.
  const handleCodexApiKey = useCallback(
    (key: string) => {
      stopCodexPoll();
      setCodexError(null);
      setCodexBusy(true);
      void invoke<CodexAuthState>("codex_login_with_api_key", { key })
        .then((state) => {
          setCodexBusy(false);
          if (state.authed) refreshSetup();
          else setCodexError(state.detail || "API 키 로그인에 실패했습니다.");
        })
        .catch((error) => {
          setCodexBusy(false);
          setCodexError(String(error));
        });
    },
    [refreshSetup, stopCodexPoll],
  );

  // Stop any in-flight OAuth poll on unmount.
  useEffect(() => stopCodexPoll, [stopCodexPoll]);

  // Plan approval: archive the approval into the log and start the apply turn
  // BEFORE awaiting (plan_approve also resolves only at turn end — see
  // handleSend); a failed send returns the flow to ready.
  const handlePlanApprove = useCallback(async () => {
    const rev = store.getState().plan?.revision;
    store.log("agent", rev !== undefined ? `계획안(rev ${rev})을 승인했습니다.` : "계획을 승인했습니다.");
    store.planApproveSent();
    const sent = await clientRef.current?.send({ type: "plan_approve" });
    if (!sent) {
      store.errorReceived("계획 승인 요청을 처리하지 못했습니다.");
    }
  }, [store]);

  // Fire a changeset_decision and record it in the store (so the matching
  // rollback_result is labelled per accept/reject). The ids are the literal
  // "all" (bulk) or the item's ids (ChangesetView resolves dat group ids).
  const handleDecide = useCallback(
    async (decision: "accept" | "reject", ids: "all" | string[]) => {
      // Record the pending decision BEFORE issuing the command: the core emits
      // `rollback_result` WHILE the changeset_decision invoke is in flight, so
      // setting pendingDecision after `await send` lets the event arrive first
      // (found with no pending decision, it no-ops) and the "결정 처리 중…"
      // spinner would never clear. Mirrors handleSend/handlePlanApprove.
      store.decisionSent(decision, ids);
      const sent = await clientRef.current?.send({
        type: "changeset_decision",
        decision,
        ids,
      });
      // The command never ran → no rollback_result will arrive; unlock the
      // controls so the review is not stranded mid-spinner.
      if (!sent) store.decisionFailed();
    },
    [store],
  );

  const handleMemoryOpen = useCallback(async () => {
    store.memoryOpened();
    await clientRef.current?.send({ type: "memory_get" });
  }, [store]);

  const handleMemorySave = useCallback(
    async ({ file, content }: { file: MemoryFile; content: string }) => {
      const sent = await clientRef.current?.send({
        type: "memory_save",
        file,
        content,
      });
      if (sent) store.memorySaveSent(file);
    },
    [store],
  );

  const rag = ragState === "idle" ? undefined : { state: ragState, elapsedSec: ragElapsedSec };

  if (setup?.setup_required || bootstrap.active) {
    return (
      <SetupScreen
        editorValid={setup?.editor_valid ?? true}
        pickError={setup?.error ?? null}
        onPick={handlePickEditorPath}
        view={bootstrap.view}
        error={bootstrap.error}
        onRetry={handleBootstrapRetry}
        assetsReady={setup?.assets_ready ?? false}
        codexResolved={setup?.codex_resolved ?? true}
        codexAuthed={setup?.codex_authed ?? true}
        codexBusy={codexBusy}
        codexError={codexError}
        onCodexInstall={handleCodexInstall}
        onCodexOAuth={handleCodexOAuth}
        onCodexApiKey={handleCodexApiKey}
      />
    );
  }

  return (
    <div className="flex h-screen flex-col bg-background text-foreground">
      <Header
        project={state.project}
        connected={state.connected}
        phase={state.phase}
        rag={rag}
        memoryOpen={state.memoryOpen}
        onMemoryOpen={handleMemoryOpen}
      />

      {update && !updateDismissed && (
        <UpdateNotice
          update={update}
          relaunch={updater.relaunch}
          onLater={() => setUpdateDismissed(true)}
        />
      )}

      {!state.editorConnected && <ConnectionNotice />}

      {/* The live agent activity (reasoning / tool rows / streamed answer)
          renders INLINE inside the conversation scroll area (EUD-069) — a fixed
          band here grew unbounded and crushed the log + plan card to 0px/33px
          in the live E2E. ConversationLog owns the placement now. */}
      <ConversationLog
        log={state.log}
        phase={state.phase}
        turn={state.turn}
        ragLoading={state.rag === "loading"}
        onSuggestion={handleSuggestion}
        suggestionsEnabled={state.canSend}
      />

      {/* Plan review — markdown card + feedback/approve (features/06). The card
          stays visible across the iteration turn (plan_review while awaiting a
          decision, thinking while the feedback/approve turn runs) and only
          disappears when the store clears the plan (chat / transport re-open) or a
          changeset opens. Controls disable (`pending`) once the turn is in
          flight, i.e. when the phase has left plan_review. */}
      {state.plan && (state.phase === "plan_review" || state.phase === "thinking") && (
        <PlanView
          plan={state.plan}
          pending={state.phase !== "plan_review"}
          onApprove={handlePlanApprove}
        />
      )}

      {state.changeset && state.phase === "changeset_review" && (
        <ChangesetView
          changeset={state.changeset}
          pending={state.pendingDecision !== null}
          onDecide={handleDecide}
        />
      )}

      {state.memoryOpen && state.memory && (
        <MemoryView
          memory={state.memory}
          onClose={store.memoryClosed}
          onTabSelected={store.memoryTabSelected}
          onEdited={store.memoryEdited}
          onSave={handleMemorySave}
        />
      )}

      {state.memoryOpen && !state.memory && (
        <section
          aria-label="프로젝트 메모리"
          className="border-t border-border p-4 text-sm text-muted-foreground"
        >
          메모리를 여는 중…
        </section>
      )}

      <InstructionBox state={state} onSend={handleSend} onReset={handleReset} />
    </div>
  );
}
