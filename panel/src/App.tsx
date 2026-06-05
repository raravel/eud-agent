/**
 * Panel app shell (v2) — wires the WS v2 client + state store to the UI.
 *
 * THIS IS THE PROTOCOL/STORE-LAYER SHELL ONLY (EUD-058). The chat-first v2 UI
 * components (PlanView / ChangesetView / AgentStream / regated InstructionBox /
 * status-rich Header / card ConversationLog) are a LATER task (features/06
 * ## Implementation). This shell keeps the build green and exercises the v2
 * store end-to-end: it dispatches every v2 server message into the store, sends
 * `chat` on submit, and renders minimal plan/changeset placeholders so the
 * review states are visible without the full UI.
 *
 * Data flow: WsClient (real WebSocket factory + window.location) → store actions
 * + log entries → React snapshot via useSyncExternalStore → components → user
 * intents call client.send + the matching store action.
 */
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useSyncExternalStore,
} from "react";
import { Header } from "@/components/Header";
import { ConversationLog } from "@/components/ConversationLog";
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
          break;
        }
        case "agent_event":
          store.agentEvent(msg.kind, msg.detail);
          break;
        case "answer":
          store.answerReceived(msg.text);
          store.log("agent", msg.text);
          break;
        case "plan":
          store.planReceived(msg.markdown, msg.revision);
          store.log("agent", `계획안(rev ${msg.revision})이 도착했습니다.`);
          break;
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
      }
    },
    [store],
  );

  return (
    <div className="flex h-screen flex-col bg-background text-foreground">
      <Header project={state.project} connected={state.connected} phase={state.phase} />

      <ConversationLog log={state.log} phase={state.phase} />

      {/* Minimal review placeholders — the full PlanView / ChangesetView UI is a
          later task (features/06 ## Implementation). */}
      {state.plan && (
        <section
          aria-label="계획 검토"
          className="border-t border-border px-4 py-2 text-sm text-muted-foreground"
        >
          계획안(rev {state.plan.revision})을 검토하세요.
        </section>
      )}
      {state.changeset && state.phase === "changeset_review" && (
        <section
          aria-label="변경사항 검토"
          className="border-t border-border px-4 py-2 text-sm text-muted-foreground"
        >
          변경사항 {state.changeset.items.length}건이 검토 대기 중입니다.
        </section>
      )}

      <InstructionBox state={state} onSend={handleSend} />
    </div>
  );
}
