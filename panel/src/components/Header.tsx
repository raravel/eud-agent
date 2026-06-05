/**
 * Header (features/06 ## Behaviors → Status visibility): app title, project
 * name (from `status`), connection-state transitions (연결 중 → 연결됨 →
 * 재연결 중), and the RAG model state with elapsed seconds while loading
 * (`rag_warmup` started → done, elapsed formatted via lib/progress). Korean
 * labels throughout.
 */
import { cn } from "@/lib/utils";
import { formatElapsed } from "@/lib/progress";
import type { Phase } from "@/state/store";

/** RAG model lifecycle for the header pill. `idle` shows no pill. */
export type RagState = "idle" | "loading" | "ready" | "unavailable";

export interface HeaderProps {
  /** Editor project name from the `status` event ("" when unknown). */
  project: string;
  /** Whether the WS connection is currently open (store.connected). */
  connected: boolean;
  /** Panel phase — distinguishes "connecting" from "retry" wording. */
  phase: Phase;
  /** RAG model state + elapsed seconds (App tracks rag_warmup timing). */
  rag?: { state: RagState; elapsedSec?: number };
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
    return { label: "재연결 중…", tone: "bg-amber-500/15 text-amber-400" };
  }
  return { label: "연결 중…", tone: "bg-muted text-muted-foreground" };
}

/** RAG-state pill label + tone (null = no pill, e.g. idle). */
function ragPill(
  rag: HeaderProps["rag"],
): { label: string; tone: string } | null {
  if (!rag || rag.state === "idle") return null;
  switch (rag.state) {
    case "loading":
      return {
        label: `RAG: 로드 중 ${formatElapsed(rag.elapsedSec ?? 0)}`,
        tone: "bg-muted text-muted-foreground",
      };
    case "ready":
      return { label: "RAG: 준비됨", tone: "bg-emerald-500/15 text-emerald-400" };
    case "unavailable":
      return { label: "RAG: 불가", tone: "bg-amber-500/15 text-amber-400" };
  }
}

export function Header({ project, connected, phase, rag }: HeaderProps) {
  const conn = connState(connected, phase);
  const ragInfo = ragPill(rag);
  return (
    <header className="flex items-center justify-between border-b border-border px-4 py-2">
      <span className="font-semibold">EUD 에이전트</span>
      <div className="flex items-center gap-3">
        {project && (
          <span className="max-w-[16rem] truncate text-sm text-muted-foreground">
            {project}
          </span>
        )}
        {ragInfo && (
          <span
            className={cn(
              "rounded-full px-2 py-0.5 text-xs font-medium",
              ragInfo.tone,
            )}
          >
            {ragInfo.label}
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
