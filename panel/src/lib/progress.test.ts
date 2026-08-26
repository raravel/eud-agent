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
 * This suite pins a pure helper that maps (stage, detail) → a labelled entry so
 * rag_warmup distinguishes started / done / error, while other stages keep their
 * existing label unchanged.
 *
 * Contract (Step B implements `@/lib/progress`):
 *   export type ProgressKind = "progress" | "ok" | "info" | "warn";
 *   export interface ProgressLine { kind: ProgressKind; text: string; }
 *   export function progressLabel(stage: string, detail?: string): ProgressLine;
 *
 * Expected mapping:
 *   ("rag_warmup", "started") → { kind: "progress", text: "RAG 모델 준비 중…" }
 *   ("rag_warmup", "done")    → completion, e.g. { kind: "ok"|"info", text: "RAG 모델 준비 완료" }
 *   ("rag_warmup", "error: <d>") → { kind: "warn", text: "RAG 사용 불가: <detail>" }
 *   (other stages)            → existing STAGE_LABELS label, { kind: "progress" }
 */
import { describe, it, expect } from "vitest";
import { progressLabel, formatElapsed } from "./progress";

const WARMUP_PENDING = "RAG 모델 준비 중…";

describe("progressLabel", () => {
  it("rag_warmup/started keeps the '준비 중' label (kind progress)", () => {
    const out = progressLabel("rag_warmup", "started");
    expect(out.text).toBe(WARMUP_PENDING);
    expect(out.kind).toBe("progress");
  });

  it("rag_warmup/done is a completion (not the '준비 중' label; kind ok or info)", () => {
    const out = progressLabel("rag_warmup", "done");
    expect(out.text).not.toBe(WARMUP_PENDING);
    expect(out.text).toContain("완료");
    expect(["ok", "info"]).toContain(out.kind);
  });

  it("rag_warmup/error is a warning whose text includes the detail (kind warn)", () => {
    const out = progressLabel("rag_warmup", "error: boom");
    expect(out.kind).toBe("warn");
    expect(out.text).toContain("error: boom");
  });

  it("other stages keep the existing label unchanged (codex, kind progress)", () => {
    const out = progressLabel("codex", undefined);
    // Turn-agnostic wording: the codex stage also covers answer-only turns.
    expect(out.text).toBe("codex 실행 중…");
    expect(out.kind).toBe("progress");
  });
  it("shows native auto-compaction start and completion", () => {
    expect(progressLabel("compaction", "started")).toEqual({
      kind: "progress",
      text: "대화 컨텍스트 자동 압축 중…",
    });
    expect(progressLabel("compaction", "done")).toEqual({
      kind: "ok",
      text: "대화 컨텍스트 자동 압축 완료",
    });
  });

  it("surfaces task-state compiler failure without failing the foreground turn", () => {
    expect(
      progressLabel(
        "task_state_warning",
        "작업 결과는 유지되지만 구조화 상태를 갱신하지 못했습니다.",
      ),
    ).toEqual({
      kind: "warn",
      text: "작업 결과는 유지되지만 구조화 상태를 갱신하지 못했습니다.",
    });
  });

  it("surfaces the Codex-clamped 1M fallback as a warning", () => {
    const detail =
      "gpt-test의 1M 컨텍스트 요청이 Codex에서 제한되어 258400 토큰 컨텍스트를 사용합니다.";
    expect(progressLabel("large_context_fallback", detail)).toEqual({
      kind: "warn",
      text: detail,
    });
    expect(progressLabel("large_context_fallback")).toEqual({
      kind: "warn",
      text: "1M 컨텍스트 요청이 Codex에서 제한되어 보고된 컨텍스트를 사용합니다.",
    });
  });
  it("shows isolated runtime-test phases and advisory terminal states", () => {
    expect(progressLabel("trace_test", "discover")).toEqual({
      kind: "progress",
      text: "영구 회귀 테스트 탐색 중…",
    });
    expect(progressLabel("trace_test", "build")).toEqual({
      kind: "progress",
      text: "임시 테스트 맵 빌드 중…",
    });
    expect(progressLabel("trace_test", "launch").text).toContain("StarCraft");
    expect(progressLabel("trace_test", "run").text).toContain("트레이스");
    expect(progressLabel("trace_test", "done:passed")).toEqual({
      kind: "ok",
      text: "런타임 자동테스트 통과",
    });
    expect(progressLabel("trace_test", "done:failed").kind).toBe("warn");
    expect(progressLabel("trace_test", "done:inconclusive").kind).toBe("warn");
  });
  it("labels each bounded audio conversion and map-write stage", () => {
    expect(progressLabel("audio_probe").text).toContain("오디오 검사");
    expect(progressLabel("audio_transcode").text).toContain("OGG Vorbis");
    expect(progressLabel("audio_validate").text).toContain("검증");
    expect(progressLabel("waiting_map_close").text).toContain("SCMDraft");
    expect(progressLabel("map_sound_write").text).toContain("등록");
    expect(progressLabel("map_sound_verify").text).toContain("검증");
  });


});

describe("formatElapsed (RAG loading elapsed seconds — features/06 header)", () => {
  it("formats whole seconds with the 초 suffix", () => {
    expect(formatElapsed(7)).toBe("7초");
  });

  it("floors fractional seconds (a sub-second elapsed reads 0초)", () => {
    expect(formatElapsed(0.9)).toBe("0초");
  });

  it("clamps a negative elapsed to 0초 (clock skew guard)", () => {
    expect(formatElapsed(-3)).toBe("0초");
  });

  it("formats large elapsed values", () => {
    expect(formatElapsed(125)).toBe("125초");
  });
});
