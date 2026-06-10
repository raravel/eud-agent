/**
 * Instruction box (v2 — features/06 ## Behaviors → Send gating + New conversation),
 * rebuilt on the vendored AI-Elements PromptInput (decision 06 — replaces the bare
 * Textarea + Button):
 *   - `chat {text}`; Send is disabled while busy (a turn in flight / plan awaiting
 *     a decision / editor compiling) and when the store gates it off (no project).
 *     Gating is purely the store's `canSend` (connected && hasProject && !busy) —
 *     the settable-target requirement is GONE (the agent chooses files/targets).
 *   - a [새 대화] control sends `reset{}` (App invokes the command + store action)
 *     and is disabled while a turn is in flight (phase === "thinking").
 *
 * The textarea keeps the accessible name "지시 입력"; the send button "전송"; the
 * reset button "새 대화". Enter (without Shift / IME composition) submits via the
 * PromptInput form when the submit button is enabled.
 */
import { useState } from "react";
import { RotateCcwIcon, SendIcon } from "lucide-react";
import {
  PromptInput,
  PromptInputBody,
  PromptInputFooter,
  PromptInputSubmit,
  PromptInputTextarea,
  PromptInputTools,
} from "@/components/ai-elements/prompt-input";
import { PromptInputButton } from "@/components/ai-elements/prompt-input";
import type { PanelState } from "@/state/store";

export interface ChatPayload {
  text: string;
}

export interface InstructionBoxProps {
  state: PanelState;
  onSend(msg: ChatPayload): void;
  /** Send `reset{}` (new conversation). Optional — App wires it. */
  onReset?(): void;
}

export function InstructionBox({ state, onSend, onReset }: InstructionBoxProps) {
  const [instruction, setInstruction] = useState("");

  // Send gating v2: the store's single `canSend` selector (connected &&
  // hasProject && !busy). Empty text is guarded at send time (a no-op).
  const canSend = state.canSend;
  // A turn is in flight while thinking — reset is disabled then.
  const turnInFlight = state.phase === "thinking";
  // RAG warmup gate: while the model loads the store gates canSend off AND the
  // textarea is disabled (no typing before the model is ready); the placeholder
  // explains why. Fail-open for unknown/unavailable (never lock forever).
  const ragLoading = state.rag === "loading";
  const editorDisconnected = !state.editorConnected;
  // EUD-074: during plan_review the SAME input is the plan-feedback channel
  // (App routes the send to plan_feedback{}); guide the user accordingly.
  const placeholder = editorDisconnected
    ? "에디터가 연결되지 않았습니다. EUD Editor 3을 실행하세요"
    : ragLoading
      ? "RAG 모델 준비 중… 준비가 끝나면 입력할 수 있습니다"
      : state.phase === "plan_review"
        ? "계획 수정 피드백을 입력하세요 (승인은 계획 카드에서)"
      : "무엇을 만들까요? (예: 게임 시작 시 미네랄 +1000 트리거 추가)";

  function handleSend() {
    const text = instruction.trim();
    if (!canSend || text.length === 0) return;
    onSend({ text });
    setInstruction("");
  }

  return (
    <div className="border-t border-border p-3">
      {/* The footer MUST be a SIBLING of PromptInputBody (a direct child of the
          InputGroup): the group's column layout comes from CSS `:has(> ...)`
          direct-child selectors (`has-[>[data-align=block-end]]:flex-col` /
          `:h-auto`). Nested inside the `display: contents` body, those selectors
          do not match and the group stays a fixed-height ROW — the textarea
          collapses to ~24px and the placeholder renders vertically (EUD-066). */}
      <PromptInput onSubmit={handleSend}>
        <PromptInputBody>
          <PromptInputTextarea
            aria-label="지시 입력"
            value={instruction}
            onChange={(e) => setInstruction(e.target.value)}
            placeholder={placeholder}
            disabled={ragLoading}
          />
        </PromptInputBody>
        <PromptInputFooter>
          <PromptInputTools>
            <PromptInputButton
              type="button"
              aria-label="새 대화"
              disabled={turnInFlight || onReset === undefined}
              onClick={() => onReset?.()}
            >
              <RotateCcwIcon className="size-4" />
              새 대화
            </PromptInputButton>
          </PromptInputTools>
          <PromptInputSubmit aria-label="전송" disabled={!canSend}>
            <SendIcon className="size-4" />
            전송
          </PromptInputSubmit>
        </PromptInputFooter>
      </PromptInput>
    </div>
  );
}
