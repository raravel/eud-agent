/**
 * First-run bootstrap progress parsing (EUD-120).
 *
 * The Rust bootstrap emitter sends `progress { stage:"bootstrap", pct, detail }`.
 * This helper keeps that mapping pure so App can route bootstrap progress to
 * setup UI instead of logging a raw stage.
 */

export type BootstrapPhase = "downloading" | "verifying" | "error";

export interface BootstrapView {
  pct: number | null;
  label: string;
  phase: BootstrapPhase;
}

const DEFAULT_BOOTSTRAP_LABEL = "설치 준비 중…";

function normalizePct(pct: number | null | undefined): number | null {
  if (typeof pct !== "number" || !Number.isFinite(pct)) return null;
  const value = Math.floor(pct);
  return Math.min(100, Math.max(0, value));
}

/** Map bootstrap progress payload fields into setup-screen view state. */
export function bootstrapView(
  pct: number | null | undefined,
  detail?: string,
): BootstrapView {
  const label = detail?.trim() ?? "";
  if (label.length === 0) {
    return {
      pct: normalizePct(pct),
      label: DEFAULT_BOOTSTRAP_LABEL,
      phase: "downloading",
    };
  }
  const lower = label.toLowerCase();
  if (lower.startsWith("error")) {
    return { pct: normalizePct(pct), label, phase: "error" };
  }

  const phase =
    lower.includes("verify") || label.includes("검증")
      ? "verifying"
      : "downloading";
  return { pct: normalizePct(pct), label, phase };
}
