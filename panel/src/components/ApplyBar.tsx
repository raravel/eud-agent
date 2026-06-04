/**
 * Apply bar (features/03 ## Behaviors → Apply):
 *   - SET    → apply {mode:"set",    target, code} — code is the Monaco buffer.
 *   - NEWEPS → apply {mode:"neweps", target:<filename>, code} — filename validated.
 *   - Cancel returns to ready.
 *   - Buttons gated by the store (canSendSet / canSendNewEps) + applying/waiting.
 *
 * Diagnostics NEVER reach this component, so advisory diagnostics structurally
 * cannot block Apply (rules.md: diagnostics are advisory only).
 */
import { Button } from "@/components/ui/button";
import { validateNewEpsName, type PanelState } from "@/state/store";

export interface ApplyPayload {
  mode: "set" | "neweps";
  target: string;
  code: string;
}

export interface ApplyBarProps {
  state: PanelState;
  /** Monaco buffer — the Apply SET/NEWEPS body. */
  editedCode: string;
  /** Raw NEWEPS filename (validated here for the target). */
  newEpsName: string;
  onApply(payload: ApplyPayload): void;
  onCancel(): void;
}

/** Phases in which apply is in flight (buttons disabled). */
const APPLYING = new Set<PanelState["phase"]>(["applying", "waiting"]);

export function ApplyBar({
  state,
  editedCode,
  newEpsName,
  onApply,
  onCancel,
}: ApplyBarProps) {
  const inFlight = APPLYING.has(state.phase);
  const nameValidation = validateNewEpsName(newEpsName);

  const canSet = state.canSendSet && !inFlight;
  const canNewEps = state.canSendNewEps && nameValidation.ok && !inFlight;

  function applySet() {
    if (!canSet) return;
    onApply({ mode: "set", target: state.selectedTarget, code: editedCode });
  }

  function applyNewEps() {
    if (!canNewEps || !nameValidation.ok) return;
    onApply({ mode: "neweps", target: nameValidation.name, code: editedCode });
  }

  return (
    <div className="flex items-center gap-2 border-t border-border px-4 py-3">
      <Button type="button" onClick={applySet} disabled={!canSet} aria-label="적용 (SET)">
        적용 (SET)
      </Button>
      <Button
        type="button"
        variant="secondary"
        onClick={applyNewEps}
        disabled={!canNewEps}
        aria-label="새 파일로 적용 (NEWEPS)"
      >
        새 파일로 적용 (NEWEPS)
      </Button>
      <Button type="button" variant="ghost" onClick={onCancel} aria-label="취소">
        취소
      </Button>
    </div>
  );
}
