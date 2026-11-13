import i18n from "@/lib/i18n";
import { useTranslation } from "react-i18next";
import { useEffect, useState } from "react";
import { PauseIcon, PlayIcon, SquareIcon } from "lucide-react";

import { Shimmer } from "@/components/ai-elements/shimmer";
import type { AutonomousRunState } from "@/lib/ipc";
import type { TurnState } from "@/state/store";

function turnActivityLabel(turn: TurnState): string {
  for (let index = turn.tools.length - 1; index >= 0; index -= 1) {
    const tool = turn.tools[index];
    if (tool.state === "running") return i18n.t("chat.turn.toolRunning", { name: tool.name });
  }
  if (turn.answerStarted) return i18n.t("chat.turn.writingAnswer");
  if (turn.tools.length > 0) return i18n.t("chat.turn.checkingTools");
  if (turn.reasoning.length > 0) return i18n.t("chat.turn.reasoning");
  return i18n.t("chat.turn.preparing");
}

function autonomousStatusLabel(status: AutonomousRunState["status"]): string {
  switch (status) {
    case "running":
      return i18n.t("chat.turn.longRunning");
    case "pausing":
      return i18n.t("chat.turn.longPausing");
    case "paused":
      return i18n.t("chat.turn.longPaused");
    case "paused_after_restart":
      return i18n.t("chat.turn.longPausedAfterRestart");
    case "waiting_input":
      return i18n.t("chat.turn.longWaitingInput");
    case "review":
      return i18n.t("chat.turn.longReview");
    case "safety_stopped":
      return i18n.t("chat.turn.longSafetyStopped");
    case "cancelled":
      return i18n.t("chat.turn.longCancelled");
    case "failed":
      return i18n.t("chat.turn.longFailed");
    case "completed":
      return i18n.t("chat.turn.longCompleted");
  }
}

function formatDuration(milliseconds: number): string {
  const totalSeconds = Math.max(0, Math.floor(milliseconds / 1_000));
  const hours = Math.floor(totalSeconds / 3_600);
  const minutes = Math.floor((totalSeconds % 3_600) / 60);
  const seconds = totalSeconds % 60;
  return hours > 0
    ? i18n.t("chat.turn.durationHms", { hours, minutes, seconds })
    : i18n.t("chat.turn.durationMs", { minutes, seconds });
}

function observedElapsed(run: AutonomousRunState, now: number): number {
  const active =
    run.activeStartedAt !== undefined &&
    (run.status === "running" || run.status === "pausing")
      ? Math.max(0, now - run.activeStartedAt)
      : 0;
  return run.progress.elapsedActiveMillis + active;
}

export interface AgentTurnStatusProps {
  turn: TurnState;
  autonomousRun?: AutonomousRunState | null;
  onCancel?(): void;
  onPause?(): void;
  onResume?(): void;
  onStop?(): void;
  cancelDisabled?: boolean;
}

export function AgentTurnStatus({
  turn,
  autonomousRun = null,
  onCancel,
  onPause,
  onResume,
  onStop,
  cancelDisabled = false,
}: AgentTurnStatusProps) {
  const { t } = useTranslation();
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (
      autonomousRun?.status !== "running" &&
      autonomousRun?.status !== "pausing"
    ) {
      return;
    }
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [autonomousRun?.status]);

  const resumable =
    autonomousRun?.status === "paused" ||
    autonomousRun?.status === "paused_after_restart";
  const terminal =
    autonomousRun?.status === "completed" ||
    autonomousRun?.status === "cancelled" ||
    autonomousRun?.status === "failed" ||
    autonomousRun?.status === "safety_stopped";
  const activelyRunning =
    autonomousRun === null ||
    autonomousRun.status === "running" ||
    autonomousRun.status === "pausing";
  const label = autonomousRun
    ? autonomousStatusLabel(autonomousRun.status)
    : turnActivityLabel(turn);

  return (
    <div
      data-testid="active-turn-status"
      role="status"
      aria-live="polite"
      aria-atomic="true"
      className="mb-2 flex min-h-11 flex-col gap-2 rounded-lg border border-border bg-card/95 px-3 py-2 shadow-sm sm:flex-row sm:items-center sm:justify-between"
    >
      <div className="min-w-0 flex-1 text-sm">
        <div className="flex min-w-0 items-center gap-2">
          <span
            aria-hidden
            className={`size-2 shrink-0 rounded-full ${
              activelyRunning
                ? "animate-pulse bg-emerald-400 motion-reduce:animate-none"
                : terminal
                  ? "bg-muted-foreground"
                  : "bg-amber-400"
            }`}
          />
          {activelyRunning ? (
            <Shimmer className="truncate">{label}</Shimmer>
          ) : (
            <span className="truncate font-medium">{label}</span>
          )}
        </div>
        {autonomousRun && (
          <div className="mt-1 flex flex-wrap gap-x-3 gap-y-1 text-xs text-muted-foreground">
            <span>
              {t("chat.turn.elapsed", {
                duration: formatDuration(observedElapsed(autonomousRun, now)),
              })}
            </span>
            <span>{t("chat.turn.iteration", { count: autonomousRun.iteration })}</span>
            <span>
              {t("chat.turn.actions", {
                reads: autonomousRun.progress.readActions,
                writes: autonomousRun.progress.writeActions,
              })}
            </span>
            <span>
              {t("chat.turn.latestBuild", {
                result: autonomousRun.progress.latestBuild
                  ? autonomousRun.progress.latestBuild.success
                    ? t("chat.turn.buildOk")
                    : t("chat.turn.buildErrors", {
                        count: autonomousRun.progress.latestBuild.errorCount,
                      })
                  : t("chat.turn.buildNone"),
              })}
            </span>
            {autonomousRun.progress.consecutiveNoProgress > 0 && (
              <span>
                {t("chat.turn.noProgress", {
                  count: autonomousRun.progress.consecutiveNoProgress,
                })}
              </span>
            )}
            {autonomousRun.policy.maxWallTimeMillis != null && (
              <span>
                {t("chat.turn.limit", {
                  duration: formatDuration(autonomousRun.policy.maxWallTimeMillis),
                })}
              </span>
            )}
          </div>
        )}
        {autonomousRun?.blocker && (
          <p className="mt-1 text-xs text-amber-400">
            {t("chat.turn.blocker", { reason: autonomousRun.blocker })}
          </p>
        )}
      </div>
      <div className="flex min-h-11 shrink-0 items-center justify-end gap-1">
        {autonomousRun ? (
          <>
            {autonomousRun.status === "running" && onPause && (
              <button
                type="button"
                aria-label={t("chat.turn.longPaused")}
                disabled={cancelDisabled}
                onClick={onPause}
                className="flex min-h-11 cursor-pointer items-center gap-1.5 rounded-md px-3 text-sm text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-default disabled:opacity-50"
              >
                <PauseIcon aria-hidden className="size-3.5" />
                {t("chat.turn.pause")}
              </button>
            )}
            {resumable && onResume && (
              <button
                type="button"
                aria-label={t("chat.turn.longResume")}
                disabled={cancelDisabled}
                onClick={onResume}
                className="flex min-h-11 cursor-pointer items-center gap-1.5 rounded-md px-3 text-sm text-foreground hover:bg-accent focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-default disabled:opacity-50"
              >
                <PlayIcon aria-hidden className="size-3.5 fill-current" />
                {t("chat.turn.resume")}
              </button>
            )}
            {!terminal && onStop && (
              <button
                type="button"
                aria-label={t("chat.turn.longStop")}
                disabled={cancelDisabled}
                onClick={onStop}
                className="flex min-h-11 cursor-pointer items-center gap-1.5 rounded-md px-3 text-sm text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-default disabled:opacity-50"
              >
                <SquareIcon aria-hidden className="size-3.5 fill-current" />
                {t("chat.turn.stopShort")}
              </button>
            )}
          </>
        ) : (
          onCancel && (
            <button
              type="button"
              aria-label={t("chat.turn.stop")}
              disabled={cancelDisabled}
              onClick={onCancel}
              className="flex min-h-11 cursor-pointer items-center gap-1.5 rounded-md px-3 text-sm text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-default disabled:opacity-50"
            >
              <SquareIcon aria-hidden className="size-3.5 fill-current" />
              {t("chat.turn.stopShort")}
            </button>
          )
        )}
      </div>
    </div>
  );
}
