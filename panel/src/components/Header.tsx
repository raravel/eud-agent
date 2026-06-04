/**
 * Header (features/03 ## UI layout): app title, project name (from `status`),
 * connection-state pill. Korean labels throughout.
 */
import { cn } from "@/lib/utils";
import type { Phase } from "@/state/store";

export interface HeaderProps {
  /** Editor project name from the `status` event ("" when unknown). */
  project: string;
  /** Whether the WS connection is currently open (store.connected). */
  connected: boolean;
  /** Panel phase — distinguishes "connecting" from "retry" wording. */
  phase: Phase;
}

/** Connection-state label + pill color from connected/phase. */
function connState(
  connected: boolean,
  phase: Phase,
): { label: string; tone: string } {
  if (connected) {
    return { label: "연결됨", tone: "bg-emerald-500/15 text-emerald-400" };
  }
  if (phase === "retry") {
    return { label: "재연결 대기 중…", tone: "bg-amber-500/15 text-amber-400" };
  }
  return { label: "연결 중…", tone: "bg-muted text-muted-foreground" };
}

export function Header({ project, connected, phase }: HeaderProps) {
  const conn = connState(connected, phase);
  return (
    <header className="flex items-center justify-between border-b border-border px-4 py-2">
      <span className="font-semibold">EUD 에이전트</span>
      <div className="flex items-center gap-3">
        {project && (
          <span className="max-w-[16rem] truncate text-sm text-muted-foreground">
            {project}
          </span>
        )}
        <span
          className={cn(
            "rounded-full px-2 py-0.5 text-xs font-medium",
            conn.tone,
          )}
        >
          {conn.label}
        </span>
      </div>
    </header>
  );
}
