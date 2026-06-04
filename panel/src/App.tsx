/**
 * Panel app shell — wires the EUD-033 WS client + state store to the UI
 * components (Header, ConversationLog, TargetPicker, ReviewTabs,
 * DiagnosticsStrip, ApplyBar, InstructionBox).
 *
 * Data flow: WsClient (real WebSocket factory + window.location) → store
 * actions + log entries → React snapshot via useSyncExternalStore → components
 * → user intents call client.send + the matching store action.
 *
 * Notes:
 *  - The client treats any WS close as "retry" (it NEVER branches on close
 *    codes — the pre-accept 4403 surfaces as a 1006 handshake failure; carry-
 *    forward from EUD-033).
 *  - The Monaco buffer (editedCode) is the single source of truth for Apply.
 */
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { Header } from "@/components/Header";
import { ConversationLog } from "@/components/ConversationLog";
import { TargetPicker } from "@/components/TargetPicker";
import { InstructionBox, type InstructPayload } from "@/components/InstructionBox";
import { ReviewTabs } from "@/components/ReviewTabs";
import { DiagnosticsStrip } from "@/components/DiagnosticsStrip";
import { ApplyBar, type ApplyPayload } from "@/components/ApplyBar";
import { createPanelStore } from "@/state/store";
import { WsClient } from "@/ws/client";
import type { ServerMessage } from "@/ws/protocol";
import { progressLabel } from "@/lib/progress";

export default function App() {
  // Store + client live for the lifetime of the app (created once).
  const store = useMemo(() => createPanelStore(), []);
  const clientRef = useRef<WsClient | null>(null);

  // Local UI state not owned by the store: the NEWEPS filename, the Monaco
  // edit buffer (Apply source of truth), and the diagnostics-dismissed flag.
  const [newEpsName, setNewEpsName] = useState("");
  const [editedCode, setEditedCode] = useState("");
  const [diagDismissed, setDiagDismissed] = useState(false);

  // Subscribe React to the framework-agnostic store.
  const state = useSyncExternalStore(store.subscribe, store.getState, store.getState);

  // Dispatch an inbound server message to store actions + log entries.
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
        case "code":
          store.codeReceived({
            code: msg.code,
            lang: msg.lang,
            diff: msg.diff,
            diagnostics: msg.diagnostics,
          });
          setEditedCode(msg.code); // seed the Monaco buffer with the new code
          setDiagDismissed(false); // a fresh review un-dismisses diagnostics
          break;
        case "applied":
          store.appliedReceived(msg.target);
          store.log("ok", `${msg.target}에 적용되었습니다.`);
          break;
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
        if (kind === "disconnect") store.log("warn", text);
        else if (kind === "info") store.log("info", text);
        else store.log("warn", text); // unknown / badjson
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
    (payload: InstructPayload) => {
      const sent = clientRef.current?.send({
        type: "instruct",
        instruction: payload.instruction,
        target: payload.target,
        useContext: payload.useContext,
      });
      if (sent) {
        store.log("you", payload.instruction);
        store.instructSent();
      }
    },
    [store],
  );

  const handleApply = useCallback(
    (payload: ApplyPayload) => {
      const sent = clientRef.current?.send({
        type: "apply",
        mode: payload.mode,
        target: payload.target,
        code: payload.code,
      });
      if (sent) {
        store.log(
          "info",
          payload.mode === "set"
            ? `${payload.target} 적용 중…`
            : `새 파일 ${payload.target} 생성 중…`,
        );
        store.applySent();
      }
    },
    [store],
  );

  const handleRefresh = useCallback(() => {
    clientRef.current?.send({ type: "status" });
    clientRef.current?.send({ type: "list" });
  }, []);

  const handleCancel = useCallback(() => store.cancelReview(), [store]);

  const reviewing =
    state.review !== null &&
    (state.phase === "reviewing" ||
      state.phase === "applying" ||
      state.phase === "waiting");

  return (
    <div className="flex h-screen flex-col bg-background text-foreground">
      <Header project={state.project} connected={state.connected} phase={state.phase} />

      <ConversationLog log={state.log} phase={state.phase} />

      <TargetPicker
        state={state}
        newEpsName={newEpsName}
        onSelectTarget={(p) => store.selectTarget(p)}
        onToggleNewFile={(on) => store.setNewFileMode(on)}
        onRefresh={handleRefresh}
        onChangeNewEpsName={setNewEpsName}
      />

      {reviewing && state.review && (
        <section aria-label="코드 검토" className="flex flex-col gap-2 py-2">
          <ReviewTabs
            review={state.review}
            newFileMode={state.newFileMode}
            editedCode={editedCode}
            onEditCode={setEditedCode}
          />
          <DiagnosticsStrip
            diagnostics={state.review.diagnostics}
            dismissed={diagDismissed}
            onDismiss={() => setDiagDismissed(true)}
          />
          <ApplyBar
            state={state}
            editedCode={editedCode}
            newEpsName={newEpsName}
            onApply={handleApply}
            onCancel={handleCancel}
          />
        </section>
      )}

      <InstructionBox state={state} onSend={handleSend} />
    </div>
  );
}
