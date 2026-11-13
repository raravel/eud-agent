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
import i18n from "@/lib/i18n";

/**
 * Stage → live-progress label (single source of truth; App.tsx imports this).
 * These are the "in flight" labels; rag_warmup's done/error variants are derived
 * in {@link progressLabel}, not stored here.
 */
export const STAGE_LABELS: Record<string, string> = {
  get rag() { return i18n.t("messages.progress.rag"); },
  get rag_warmup() { return i18n.t("messages.progress.ragWarmup"); },
  // The codex stage covers ANY turn (greetings/answers included), so the label
  // must not claim code generation specifically.
  get codex() { return i18n.t("messages.progress.codex"); },
  get provider() { return i18n.t("messages.progress.provider"); },
  get compaction() { return i18n.t("messages.progress.compaction"); },
  get task_state_warning() { return i18n.t("messages.progress.taskStateWarning"); },
  get workspace() { return i18n.t("messages.progress.workspace"); },
  get lsp() { return i18n.t("messages.progress.lsp"); },
  get waiting_build() { return i18n.t("messages.progress.waitingBuild"); },
  get trace_test() { return i18n.t("messages.progress.traceTest"); },
  get bootstrap() { return i18n.t("messages.progress.bootstrap"); },
  get audio_probe() { return i18n.t("messages.progress.audioProbe"); },
  get audio_transcode() { return i18n.t("messages.progress.audioTranscode"); },
  get audio_validate() { return i18n.t("messages.progress.audioValidate"); },
  get waiting_map_close() { return i18n.t("messages.progress.waitingMapClose"); },
  get map_sound_write() { return i18n.t("messages.progress.mapSoundWrite"); },
  get map_sound_verify() { return i18n.t("messages.progress.mapSoundVerify"); },
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
      return { kind: "ok", text: i18n.t("messages.progress.ragReady") };
    }
    if (detail !== undefined && detail.startsWith("error")) {
      return { kind: "warn", text: i18n.t("messages.progress.ragUnavailable", { detail }) };
    }
    // "started" (or any other detail) → the in-progress label.
    return { kind: "progress", text: STAGE_LABELS.rag_warmup };
  }
  if (stage === "compaction") {
    return detail === "done"
      ? { kind: "ok", text: i18n.t("messages.progress.compactionDone") }
      : { kind: "progress", text: STAGE_LABELS.compaction };
  }
  if (stage === "trace_test") {
    if (detail === "discover") {
      return { kind: "progress", text: i18n.t("messages.progress.traceDiscover") };
    }
    if (detail === "build") {
      return { kind: "progress", text: i18n.t("messages.progress.traceBuild") };
    }
    if (detail === "launch") {
      return { kind: "progress", text: i18n.t("messages.progress.traceLaunch") };
    }
    if (detail === "run") {
      return { kind: "progress", text: i18n.t("messages.progress.traceRun") };
    }
    if (detail === "done:passed") {
      return { kind: "ok", text: i18n.t("messages.progress.tracePassed") };
    }
    if (detail?.startsWith("done:")) {
      return { kind: "warn", text: i18n.t("messages.progress.traceDiagnosed") };
    }
    return { kind: "progress", text: STAGE_LABELS.trace_test };
  }
  if (stage === "task_state_warning") {
    return {
      kind: "warn",
      text: detail ?? STAGE_LABELS.task_state_warning,
    };
  }
  if (stage === "large_context_fallback") {
    return {
      kind: "warn",
      text:
        detail ??
        i18n.t("messages.progress.largeContextFallback"),
    };
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
  return i18n.t("messages.progress.elapsed", { count: safe });
}
