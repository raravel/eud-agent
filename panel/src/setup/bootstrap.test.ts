/**
 * First-run bootstrap progress parsing (EUD-120).
 *
 * The Rust bootstrap emitter sends setup progress as
 * `progress { stage: "bootstrap", pct, detail }`. The panel uses the numeric
 * pct field directly and keeps the human-readable detail as the setup label.
 *
 * Contract (Step B implements `@/setup/bootstrap`):
 *   export type BootstrapPhase = "downloading" | "verifying" | "error";
 *   export interface BootstrapView {
 *     pct: number | null;
 *     label: string;
 *     phase: BootstrapPhase;
 *   }
 *   export function bootstrapView(pct: number | null | undefined, detail?: string): BootstrapView;
 */
import { describe, it, expect } from "vitest";
import { bootstrapView } from "@/setup/bootstrap";

describe("bootstrapView", () => {
  it("uses the Korean setup-preparing label when detail is undefined", () => {
    expect(bootstrapView(null, undefined)).toEqual({
      pct: null,
      label: "설치 준비 중…",
      phase: "downloading",
    });
  });

  it("uses the Korean setup-preparing label when detail is empty", () => {
    expect(bootstrapView(undefined, "")).toEqual({
      pct: null,
      label: "설치 준비 중…",
      phase: "downloading",
    });
  });

  it("uses the numeric percent field for download progress", () => {
    const out = bootstrapView(45, "downloading bge-m3 model");
    expect(out.pct).toBe(45);
    expect(out.phase).toBe("downloading");
    expect(out.label).toContain("bge-m3");
  });

  it("does not treat pct 100 as whole-bootstrap completion", () => {
    const out = bootstrapView(100, "rag index installed");
    expect(out.pct).toBe(100);
    expect(out.phase).toBe("downloading");
  });

  it("marks verify details as verifying", () => {
    const out = bootstrapView(100, "RAG 인덱스 검증 중");
    expect(out.phase).toBe("verifying");
    expect(out.pct).toBe(100);
  });

  it("maps an error detail to error phase", () => {
    const out = bootstrapView(null, "error: 디스크 공간 부족");
    expect(out.phase).toBe("error");
    expect(out.pct).toBeNull();
    expect(out.label).toContain("디스크");
  });

  it("clamps percentages to 0..100", () => {
    expect(bootstrapView(150, "x").pct).toBe(100);
    expect(bootstrapView(-5, "x").pct).toBe(0);
  });

  it("floors decimal percentages", () => {
    expect(bootstrapView(12.9, "x").pct).toBe(12);
  });
});
