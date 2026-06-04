/**
 * Instruction box (features/03 ## Behaviors → Instruct):
 *   instruct {instruction, target, useContext}; useContext checkbox default ON;
 *   Send disabled while working/applying/waiting and when the store gates it off
 *   (no settable picker selection / no project).
 *
 * INSTRUCT TARGET CONTRACT (features/02 orchestrator.py): instruct.target MUST
 * be an EXISTING settable file — the server GETs it for the diff stage. New-file
 * creation happens ONLY via apply {mode:"neweps"}; new-file mode does NOT change
 * instruct.target. So the picker's selected settable file is ALWAYS the instruct
 * target, and Send is gated on `canSendSet` even in new-file mode (an empty
 * project with zero settable files cannot produce a diff target → instruct can't
 * run at all). The NEWEPS filename is used only by the ApplyBar.
 *
 * Composed from plain primitives (Textarea + Checkbox + Button) — NOT the
 * vendored PromptInput, which pulls cmdk/ai/dropdown/hovercard the panel does
 * not need (dep-pruning carry-forward).
 */
import { useState } from "react";
import { SendIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Textarea } from "@/components/ui/textarea";
import type { PanelState } from "@/state/store";

export interface InstructPayload {
  instruction: string;
  /** ALWAYS the picker's selected settable file (never the NEWEPS name). */
  target: string;
  useContext: boolean;
}

export interface InstructionBoxProps {
  state: PanelState;
  onSend(msg: InstructPayload): void;
}

export function InstructionBox({ state, onSend }: InstructionBoxProps) {
  const [instruction, setInstruction] = useState("");
  const [useContext, setUseContext] = useState(true);

  // Instruct ALWAYS needs a settable picker selection (the diff target), in
  // either mode — gate on canSendSet. The button's enabled state tracks the
  // STORE gate only; empty instruction text is guarded at send time (a no-op),
  // not by disabling the control.
  const canSend = state.canSendSet;

  // The instruct target is the picker selection, regardless of new-file mode.
  const target = state.selectedTarget;

  function handleSend() {
    const text = instruction.trim();
    if (!canSend || text.length === 0) return;
    onSend({ instruction: text, target, useContext });
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
      <div className="flex items-center justify-between">
        <label className="flex items-center gap-2 text-sm text-muted-foreground">
          <Checkbox
            checked={useContext}
            onCheckedChange={(v) => setUseContext(v === true)}
            aria-label="컨텍스트 사용"
          />
          컨텍스트 사용
        </label>
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
