/**
 * Self-update notice banner (Decision 04).
 *
 * Non-blocking banner shown above the main UI once an update is available. The user
 * consents ([지금 업데이트]) or defers ([나중에]); on consent it streams the download
 * progress and, when the install finishes, relaunches into the new version. Presentational
 * + local flow state only — the actual download/install/relaunch are injected via the
 * {@link UpdateHandle} and `relaunch` props (see `@/setup/update`).
 */
import { useCallback, useState } from "react";
import {
  downloadPct,
  type DownloadProgress,
  type UpdateHandle,
} from "@/setup/update";

export interface UpdateNoticeProps {
  /** The pending update (version/notes + download action). */
  update: UpdateHandle;
  /** Restart into the freshly-installed version. */
  relaunch: () => Promise<void>;
  /** Defer the update; App hides the banner for this session. */
  onLater: () => void;
}

type Phase = "prompt" | "downloading" | "error";

export function UpdateNotice({ update, relaunch, onLater }: UpdateNoticeProps) {
  const [phase, setPhase] = useState<Phase>("prompt");
  const [progress, setProgress] = useState<DownloadProgress>({
    downloaded: 0,
    total: null,
  });
  const [error, setError] = useState<string | null>(null);

  const handleUpdate = useCallback(() => {
    setPhase("downloading");
    setError(null);
    setProgress({ downloaded: 0, total: null });
    void update
      .downloadAndInstall((p) => setProgress(p))
      .then(() => relaunch())
      .catch((e) => {
        setError(String(e));
        setPhase("error");
      });
  }, [update, relaunch]);

  const pct = downloadPct(progress);

  return (
    <section
      role="status"
      aria-label="업데이트 알림"
      className="border-b border-sky-500/30 bg-sky-500/10 px-4 py-2 text-sm text-sky-100"
    >
      {phase === "prompt" && (
        <div className="flex items-center justify-between gap-3">
          <div className="min-w-0">
            <span className="font-medium">
              새 버전 {update.version}을 사용할 수 있습니다.
            </span>{" "}
            <span className="text-sky-100/80">(현재 {update.currentVersion})</span>
            {update.notes.trim().length > 0 && (
              <p className="mt-0.5 truncate text-sky-100/70">{update.notes}</p>
            )}
          </div>
          <div className="flex shrink-0 gap-2">
            <button
              type="button"
              onClick={handleUpdate}
              className="rounded bg-sky-500 px-3 py-1 font-medium text-white hover:bg-sky-400"
            >
              지금 업데이트
            </button>
            <button
              type="button"
              onClick={onLater}
              className="rounded px-3 py-1 text-sky-100/80 hover:text-sky-100"
            >
              나중에
            </button>
          </div>
        </div>
      )}

      {phase === "downloading" && (
        <div>
          <div className="flex items-center justify-between">
            <span className="font-medium">업데이트 다운로드 중…</span>
            <span className="tabular-nums text-sky-100/80">
              {pct === null ? "" : `${pct}%`}
            </span>
          </div>
          <div
            role="progressbar"
            aria-label="업데이트 다운로드 진행률"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={pct ?? undefined}
            className="mt-1 h-1 w-full overflow-hidden rounded bg-sky-500/20"
          >
            <div
              className="h-full bg-sky-400 transition-all"
              style={{ width: pct === null ? "100%" : `${pct}%` }}
            />
          </div>
        </div>
      )}

      {phase === "error" && (
        <div className="flex items-center justify-between gap-3">
          <span className="min-w-0 truncate text-rose-200">
            업데이트에 실패했습니다: {error}
          </span>
          <div className="flex shrink-0 gap-2">
            <button
              type="button"
              onClick={handleUpdate}
              className="rounded bg-sky-500 px-3 py-1 font-medium text-white hover:bg-sky-400"
            >
              다시 시도
            </button>
            <button
              type="button"
              onClick={onLater}
              className="rounded px-3 py-1 text-sky-100/80 hover:text-sky-100"
            >
              나중에
            </button>
          </div>
        </div>
      )}
    </section>
  );
}
