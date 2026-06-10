/**
 * Progress-line labelling for the conversation log (EUD-041).
 *
 * Bug: the panel showed "RAG 모델 준비 중…" and it never cleared. NOT a hang —
 * warmup completes in ~19s. The server sends `progress {stage:"rag_warmup",
 * detail:"started"}` then `{... detail:"done"}` (or `detail:"error: ..."`), but
 * App.tsx labelled purely by `stage` via `STAGE_LABELS` and IGNORED `msg.detail`,
 * so `started` and `done` both rendered the same "준비 중" line and completion
 * was never shown.
 *
 * This pure helper maps (stage, detail) → a labelled log entry so rag_warmup
 * distinguishes started / done / error, while every other stage keeps its
 * existing STAGE_LABELS label (detail ignored). It is the single source of truth
 * for stage→label text (App.tsx imports STAGE_LABELS from here).
 *
 * The returned `kind` is a {@link LogKind} so the result feeds the store's
 * `log(kind, text, stage?)` directly.
 */
import type { LogKind } from "@/state/store";

/**
 * Stage → live-progress label (single source of truth; App.tsx imports this).
 * These are the "in flight" labels; rag_warmup's done/error variants are derived
 * in {@link progressLabel}, not stored here.
 */
export const STAGE_LABELS: Record<string, string> = {
  rag: "RAG 컨텍스트 검색 중…",
  rag_warmup: "RAG 모델 준비 중…",
  // The codex stage covers ANY turn (greetings/answers included), so the label
  // must not claim code generation specifically.
  codex: "codex 실행 중…",
  lsp: "진단 검사 중…",
  waiting_build: "에디터 빌드 완료 대기 중…",
};

/** A labelled progress line ready for `store.log(kind, text, stage)`. */
export interface ProgressLine {
  kind: LogKind;
  text: string;
}

/**
 * Map a progress event to a labelled log line.
 *
 * rag_warmup is detail-sensitive:
 *   - "started" → still in progress ("RAG 모델 준비 중…").
 *   - "done"    → completion ("RAG 모델 준비 완료").
 *   - "error*"  → warning ("RAG 사용 불가: <detail>"), so RAG-unavailable is
 *                 visible but never blocks the flow (rules.md: RAG advisory).
 *
 * Every other stage keeps its STAGE_LABELS label as a "progress" line and
 * ignores `detail`. An unknown stage falls back to the raw stage string.
 */
export function progressLabel(stage: string, detail?: string): ProgressLine {
  if (stage === "rag_warmup") {
    if (detail === "done") {
      return { kind: "ok", text: "RAG 모델 준비 완료" };
    }
    if (detail !== undefined && detail.startsWith("error")) {
      return { kind: "warn", text: `RAG 사용 불가: ${detail}` };
    }
    // "started" (or any other detail) → the in-progress label.
    return { kind: "progress", text: STAGE_LABELS.rag_warmup };
  }
  return { kind: "progress", text: STAGE_LABELS[stage] ?? stage };
}

/**
 * Format an elapsed duration (seconds) for the RAG-loading header pill
 * (features/06 ## Behaviors → Status visibility: "RAG model state with elapsed
 * seconds while loading"). Floors to whole seconds and clamps negatives to 0
 * (a clock-skew / not-yet-started guard), with the Korean 초 suffix.
 */
export function formatElapsed(seconds: number): string {
  const safe = Number.isFinite(seconds) && seconds > 0 ? Math.floor(seconds) : 0;
  return `${safe}초`;
}
