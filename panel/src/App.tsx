/**
 * Panel app shell (v2) — wires the WS v2 client + state store to the chat-first
 * review UI (features/06_changeset-review-panel.md).
 *
 * Components: a status-rich Header (connection transitions + RAG state/elapsed),
 * the ConversationLog cards, a live AgentStream under the turn, the PlanView
 * feedback/approve surface, the ChangesetView accept/reject surface, and the
 * regated InstructionBox. Plan cards are archived into the conversation log as
 * agent entries when a plan arrives, is superseded by a higher revision, or is
 * approved.
 *
 * Data flow: WsClient (real WebSocket factory + window.location) → store actions
 * + log entries → React snapshot via useSyncExternalStore → components → user
 * intents call client.send + the matching store action. Two pieces of UI-only
 * state live here (not protocol state): the current turn's agent_event list (for
 * AgentStream) and the RAG warmup state/timing (for the Header pill).
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
import { AgentStream, type AgentActivity } from "@/components/AgentStream";
import { ChangesetView } from "@/components/ChangesetView";
import { PlanView } from "@/components/PlanView";
import { InstructionBox, type ChatPayload } from "@/components/InstructionBox";
import { createPanelStore } from "@/state/store";
import { WsClient } from "@/ws/client";
import type { ServerMessage } from "@/ws/protocol";
import { progressLabel } from "@/lib/progress";

export default function App() {
  // Store + client live for the lifetime of the app (created once).
  const store = useMemo(() => createPanelStore(), []);
  const clientRef = useRef<WsClient | null>(null);

  // Subscribe React to the framework-agnostic store.
  const state = useSyncExternalStore(store.subscribe, store.getState, store.getState);

  // ---- UI-only state (not protocol state) ----
  // The current turn's agent_event stream (AgentStream). Reset when a new turn
  // starts (chat/plan_feedback/plan_approve sent).
  const [agentEvents, setAgentEvents] = useState<AgentActivity[]>([]);
  // RAG warmup visibility for the Header pill. `startedAt` drives the elapsed
  // counter while loading; a 1s tick re-renders so the seconds advance.
  const [ragState, setRagState] = useState<RagState>("idle");
  const ragStartRef = useRef<number | null>(null);
  const [ragElapsedSec, setRagElapsedSec] = useState(0);

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
        case "progress": {
          store.progressReceived(msg.stage);
          const { kind, text } = progressLabel(msg.stage, msg.detail);
          store.log(kind, text, msg.stage);
          // RAG warmup drives the Header pill (started → loading w/ elapsed,
          // done → ready, error → unavailable).
          if (msg.stage === "rag_warmup") {
            if (msg.detail === "done") {
              setRagState("ready");
            } else if (msg.detail !== undefined && msg.detail.startsWith("error")) {
              setRagState("unavailable");
            } else {
              ragStartRef.current = Date.now();
              setRagElapsedSec(0);
              setRagState("loading");
            }
          }
          break;
        }
        case "agent_event":
          store.agentEvent(msg.kind, msg.detail);
          setAgentEvents((prev) => [...prev, { kind: msg.kind, detail: msg.detail }]);
          break;
        case "answer":
          store.answerReceived(msg.text);
          store.log("agent", msg.text);
          break;
        case "plan": {
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
          store.errorReceived(msg.message);
          store.log("error", `오류: ${msg.message}`);
          break;
        default:
          break;
      }
    },
    [store],
  );

  // Boot the WS client once. Lifecycle maps to the store phases; logs flow
  // through store.log so they render in the conversation.
  useEffect(() => {
    store.wsConnecting();
    const client = new WsClient({
      onMessage,
      onLog: (kind, text) => {
        if (kind === "info") store.log("info", text);
        else store.log("warn", text); // disconnect / unknown / badjson
      },
      onOpenChange: (open) => {
        if (open) store.wsOpen();
        else store.wsError();
      },
    });
    clientRef.current = client;
    client.connect();
    return () => {
      client.stop();
      clientRef.current = null;
    };
  }, [store, onMessage]);

  // ---- user intents ----
  const handleSend = useCallback(
    (payload: ChatPayload) => {
      const sent = clientRef.current?.send({ type: "chat", text: payload.text });
      if (sent) {
        store.log("you", payload.text);
        store.chatSent();
        setAgentEvents([]); // a new turn — reset the activity stream.
      }
    },
    [store],
  );

  // Plan iteration: send plan_feedback{text}, archive the request into the log,
  // and start a new turn (reset the activity stream). The panel stays in
  // plan_review until the next plan{revision+1} replaces the card.
  const handlePlanFeedback = useCallback(
    (text: string) => {
      const sent = clientRef.current?.send({ type: "plan_feedback", text });
      if (sent) {
        store.log("you", text);
        store.log("agent", "계획 수정을 요청했습니다.");
        store.planFeedbackSent();
        setAgentEvents([]); // a new turn — reset the activity stream.
      }
    },
    [store],
  );

  // Plan approval: send plan_approve{}, archive the approval into the log, and
  // start the apply turn (reset the activity stream).
  const handlePlanApprove = useCallback(() => {
    const sent = clientRef.current?.send({ type: "plan_approve" });
    if (sent) {
      const rev = store.getState().plan?.revision;
      store.log("agent", rev !== undefined ? `계획안(rev ${rev})을 승인했습니다.` : "계획을 승인했습니다.");
      store.planApproveSent();
      setAgentEvents([]); // a new turn — reset the activity stream.
    }
  }, [store]);

  // Fire a changeset_decision and record it in the store (so the matching
  // rollback_result is labelled per accept/reject). The ids are the literal
  // "all" (bulk) or the item's ids (ChangesetView resolves dat group ids).
  const handleDecide = useCallback(
    (decision: "accept" | "reject", ids: "all" | string[]) => {
      const sent = clientRef.current?.send({
        type: "changeset_decision",
        decision,
        ids,
      });
      if (sent) store.decisionSent(decision, ids);
    },
    [store],
  );

  const rag = ragState === "idle" ? undefined : { state: ragState, elapsedSec: ragElapsedSec };

  return (
    <div className="flex h-screen flex-col bg-background text-foreground">
      <Header
        project={state.project}
        connected={state.connected}
        phase={state.phase}
        rag={rag}
      />

      <ConversationLog log={state.log} phase={state.phase} />

      {/* Live agent activity (collapses to a summary when the turn ends). */}
      <AgentStream events={agentEvents} live={state.phase === "thinking"} />

      {/* Plan review — markdown card + feedback/approve (features/06). The card
          stays visible across the iteration turn (plan_review while awaiting a
          decision, thinking while the feedback/approve turn runs) and only
          disappears when the store clears the plan (chat / reconnect) or a
          changeset opens. Controls disable (`pending`) once the turn is in
          flight, i.e. when the phase has left plan_review. */}
      {state.plan && (state.phase === "plan_review" || state.phase === "thinking") && (
        <PlanView
          plan={state.plan}
          pending={state.phase !== "plan_review"}
          onFeedback={handlePlanFeedback}
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

      <InstructionBox state={state} onSend={handleSend} />
    </div>
  );
}
