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
