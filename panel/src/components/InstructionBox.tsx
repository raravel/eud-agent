/**
 * Instruction box (v2 — features/06 ## Behaviors → Send gating):
 *   chat {text}; Send disabled while busy (a turn in flight / plan awaiting a
 *   decision / editor compiling) and when the store gates it off (no project).
 *   The settable-target requirement is GONE — the agent chooses files/targets
 *   itself, so there is no picker selection and no useContext toggle (the server
 *   builds RAG context for every turn). Gate purely on the store's `canSend`.
 *
 * Composed from plain primitives (Textarea + Button) — NOT the vendored
 * PromptInput, which pulls cmdk/ai/dropdown/hovercard the panel does not need
 * (dep-pruning carry-forward).
 */
import { useState } from "react";
import { SendIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import type { PanelState } from "@/state/store";

export interface ChatPayload {
  text: string;
}

export interface InstructionBoxProps {
  state: PanelState;
  onSend(msg: ChatPayload): void;
}

export function InstructionBox({ state, onSend }: InstructionBoxProps) {
  const [instruction, setInstruction] = useState("");

  // Send gating v2: the store's single `canSend` selector (connected &&
  // hasProject && !busy). Empty text is guarded at send time (a no-op), not by
  // disabling the control.
  const canSend = state.canSend;

  function handleSend() {
    const text = instruction.trim();
    if (!canSend || text.length === 0) return;
    onSend({ text });
    setInstruction("");
  }

  return (
    <div className="flex flex-col gap-2 border-t border-border p-3">
      <Textarea
        aria-label="지시 입력"
        value={instruction}
        onChange={(e) => setInstruction(e.target.value)}
        placeholder="무엇을 만들까요? (예: 게임 시작 시 미네랄 +1000 트리거 추가)"
        className="max-h-40 min-h-16 resize-none"
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
            e.preventDefault();
            handleSend();
          }
        }}
      />
      <div className="flex items-center justify-end">
        <Button
          type="button"
          onClick={handleSend}
          disabled={!canSend}
          aria-label="전송"
        >
          <SendIcon className="size-4" />
          전송
        </Button>
      </div>
    </div>
  );
}
