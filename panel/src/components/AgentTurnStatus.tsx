import { useEffect, useState } from "react";
import { PauseIcon, PlayIcon, SquareIcon } from "lucide-react";

import { Shimmer } from "@/components/ai-elements/shimmer";
import type { AutonomousRunState } from "@/lib/ipc";
import type { TurnState } from "@/state/store";

function turnActivityLabel(turn: TurnState): string {
  for (let index = turn.tools.length - 1; index >= 0; index -= 1) {
    const tool = turn.tools[index];
    if (tool.state === "running") return `도구 실행 중 · ${tool.name}`;
  }
  if (turn.answerStarted) return "응답 작성 중";
  if (turn.tools.length > 0) return "도구 결과 확인 중";
  if (turn.reasoning.length > 0) return "추론 중";
  return "작업 준비 중";
}

function autonomousStatusLabel(status: AutonomousRunState["status"]): string {
  switch (status) {
    case "running":
      return "장시간 작업 실행 중";
    case "pausing":
      return "안전한 경계에서 일시 중지 중";
    case "paused":
      return "장시간 작업 일시 중지";
    case "paused_after_restart":
      return "앱 재시작 후 일시 중지";
    case "waiting_input":
      return "사용자 응답 대기";
    case "review":
      return "변경사항 검토 대기";
    case "safety_stopped":
      return "안전 정책으로 중단";
    case "cancelled":
      return "장시간 작업 중단됨";
    case "failed":
      return "장시간 작업 실패";
    case "completed":
      return "장시간 작업 완료";
  }
}

function formatDuration(milliseconds: number): string {
  const totalSeconds = Math.max(0, Math.floor(milliseconds / 1_000));
  const hours = Math.floor(totalSeconds / 3_600);
  const minutes = Math.floor((totalSeconds % 3_600) / 60);
  const seconds = totalSeconds % 60;
  return hours > 0
    ? `${hours}시간 ${minutes}분 ${seconds}초`
    : `${minutes}분 ${seconds}초`;
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
            <span>경과 {formatDuration(observedElapsed(autonomousRun, now))}</span>
            <span>반복 {autonomousRun.iteration}</span>
            <span>
              읽기 {autonomousRun.progress.readActions} · 쓰기{" "}
              {autonomousRun.progress.writeActions}
            </span>
            <span>
              최근 빌드{" "}
              {autonomousRun.progress.latestBuild
                ? autonomousRun.progress.latestBuild.success
                  ? "성공"
                  : `오류 ${autonomousRun.progress.latestBuild.errorCount}개`
                : "없음"}
            </span>
            {autonomousRun.progress.consecutiveNoProgress > 0 && (
              <span>
                무진전 {autonomousRun.progress.consecutiveNoProgress}회
              </span>
            )}
            {autonomousRun.policy.maxWallTimeMillis != null && (
              <span>
                실행 한도 {formatDuration(autonomousRun.policy.maxWallTimeMillis)}
              </span>
            )}
          </div>
        )}
        {autonomousRun?.blocker && (
          <p className="mt-1 text-xs text-amber-400">
            중단 이유: {autonomousRun.blocker}
          </p>
        )}
      </div>
      <div className="flex min-h-11 shrink-0 items-center justify-end gap-1">
        {autonomousRun ? (
          <>
            {autonomousRun.status === "running" && onPause && (
              <button
                type="button"
                aria-label="장시간 작업 일시 중지"
                disabled={cancelDisabled}
                onClick={onPause}
                className="flex min-h-11 cursor-pointer items-center gap-1.5 rounded-md px-3 text-sm text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-default disabled:opacity-50"
              >
                <PauseIcon aria-hidden className="size-3.5" />
                일시 중지
              </button>
            )}
            {resumable && onResume && (
              <button
                type="button"
                aria-label="장시간 작업 계속"
                disabled={cancelDisabled}
                onClick={onResume}
                className="flex min-h-11 cursor-pointer items-center gap-1.5 rounded-md px-3 text-sm text-foreground hover:bg-accent focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-default disabled:opacity-50"
              >
                <PlayIcon aria-hidden className="size-3.5 fill-current" />
                계속
              </button>
            )}
            {!terminal && onStop && (
              <button
                type="button"
                aria-label="장시간 작업 중단"
                disabled={cancelDisabled}
                onClick={onStop}
                className="flex min-h-11 cursor-pointer items-center gap-1.5 rounded-md px-3 text-sm text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-default disabled:opacity-50"
              >
                <SquareIcon aria-hidden className="size-3.5 fill-current" />
                중단
              </button>
            )}
          </>
        ) : (
          onCancel && (
            <button
              type="button"
              aria-label="작업 중단"
              disabled={cancelDisabled}
              onClick={onCancel}
              className="flex min-h-11 cursor-pointer items-center gap-1.5 rounded-md px-3 text-sm text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-default disabled:opacity-50"
            >
              <SquareIcon aria-hidden className="size-3.5 fill-current" />
              중단
            </button>
          )
        )}
      </div>
    </div>
  );
}
