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
  const raw = detail?.trim() ?? "";
  if (raw.length === 0) {
    return {
      pct: normalizePct(pct),
      label: DEFAULT_BOOTSTRAP_LABEL,
      phase: "downloading",
    };
  }

  const lower = raw.toLowerCase();
  if (lower.startsWith("error")) {
    const reason = raw.replace(/^error\s*:?\s*/i, "").trim();
    return {
      pct: normalizePct(pct),
      label: reason.length > 0 ? `설치 오류: ${reason}` : "설치 오류",
      phase: "error",
    };
  }

  // These details are emitted by the managed euddraft installer. Keep the
  // backend's short state machine out of the UI while retaining useful asset
  // details (for example, model names) for the existing bootstrap screen.
  const label =
    lower === "checking latest euddraft release"
      ? "최신 euddraft 릴리스를 확인하는 중…"
      : lower === "downloading euddraft"
        ? "euddraft 다운로드 중…"
        : lower === "extracting euddraft"
          ? "euddraft 압축 해제 중…"
          : lower === "euddraft ready"
            ? "euddraft 설치 완료, 설정 저장 중…"
            : lower === "euddraft configured"
              ? "euddraft 설정 완료"
              : raw;
  const phase =
    lower.includes("verify") ||
    lower.includes("extract") ||
    raw.includes("검증") ||
    raw.includes("압축 해제")
      ? "verifying"
      : "downloading";
  return { pct: normalizePct(pct), label, phase };
}
