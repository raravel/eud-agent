/**
 * Header (features/06 ## Behaviors → Status visibility): app title, project
 * name (from `status`), connection-state transitions (연결 중 → 연결됨 →
 * 재연결 중), and the RAG model state with elapsed seconds while loading
 * (`rag_warmup` started → done, elapsed formatted via lib/progress). Korean
 * labels throughout.
 */
import { BookText, SparklesIcon } from "lucide-react";

import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { formatElapsed } from "@/lib/progress";
import type { Phase } from "@/state/store";

/** RAG model lifecycle for the header pill. `idle` shows no pill. */
export type RagState = "idle" | "loading" | "ready" | "unavailable";

export interface HeaderProps {
  /** Editor project name from the `status` event ("" when unknown). */
  project: string;
  /** Whether the transport connection is currently open (store.connected). */
  connected: boolean;
  /** Panel phase — distinguishes "connecting" from "retry" wording. */
  phase: Phase;
  /** RAG model state + elapsed seconds (App tracks rag_warmup timing). */
  rag?: { state: RagState; elapsedSec?: number };
  /** Open the project-memory overlay. */
  onMemoryOpen?: () => void;
  /** Whether the project-memory overlay is currently visible. */
  memoryOpen?: boolean;
}

/** One status pill descriptor: label + tone classes + whether it is in flight. */
interface Pill {
  label: string;
  tone: string;
  /** True while the state is transitional — the dot pulses. */
  busy?: boolean;
}

/** Connection-state label + pill tone from connected/phase. */
function connState(connected: boolean, phase: Phase): Pill {
  if (connected) {
    return {
      label: "연결됨",
      tone: "border-emerald-500/30 bg-emerald-500/15 text-emerald-400",
    };
  }
  if (phase === "retry") {
    return {
      label: "재연결 중…",
      tone: "border-amber-500/30 bg-amber-500/15 text-amber-400",
      busy: true,
    };
  }
  return {
    label: "연결 중…",
    tone: "border-border bg-muted text-muted-foreground",
    busy: true,
  };
}

/** RAG-state pill label + tone (null = no pill, e.g. idle). */
function ragPill(rag: HeaderProps["rag"]): Pill | null {
  if (!rag || rag.state === "idle") return null;
  switch (rag.state) {
    case "loading":
      return {
        label: `RAG: 로드 중 ${formatElapsed(rag.elapsedSec ?? 0)}`,
        tone: "border-border bg-muted text-muted-foreground",
        busy: true,
      };
    case "ready":
      return {
        label: "RAG: 준비됨",
        tone: "border-emerald-500/30 bg-emerald-500/15 text-emerald-400",
      };
    case "unavailable":
      return {
        label: "RAG: 불가",
        tone: "border-amber-500/30 bg-amber-500/15 text-amber-400",
      };
  }
}

/** A rounded status pill with a state dot (color is reinforced by the label
 *  text, never the dot alone). */
function StatusPill({ pill }: { pill: Pill }) {
  return (
    <span
      className={cn(
        "flex items-center gap-1.5 rounded-full border px-2 py-0.5 text-xs font-medium",
        pill.tone,
      )}
    >
      <span
        aria-hidden
        className={cn(
          "size-1.5 shrink-0 rounded-full bg-current",
          pill.busy && "animate-pulse motion-reduce:animate-none",
        )}
      />
      {pill.label}
    </span>
  );
}

export function Header({
  project,
  connected,
  phase,
  rag,
  onMemoryOpen,
  memoryOpen = false,
}: HeaderProps) {
  const conn = connState(connected, phase);
  const ragInfo = ragPill(rag);
  return (
    <header className="flex items-center justify-between gap-3 border-b border-border bg-card/60 px-4 py-2.5 backdrop-blur">
      {/* Branding tile + title + project context (same identity tile as the
          SetupScreen, scaled down). */}
      <div className="flex min-w-0 items-center gap-2.5">
        <span
          aria-hidden
          className="flex size-8 shrink-0 items-center justify-center rounded-lg border border-emerald-500/30 bg-emerald-500/15 text-emerald-400"
        >
          <SparklesIcon className="size-4" />
        </span>
        <div className="grid min-w-0">
          <span className="truncate text-sm font-semibold leading-tight">
            EUD 에이전트
          </span>
          {project && (
            <span className="max-w-[16rem] truncate text-xs text-muted-foreground">
              {project}
            </span>
          )}
        </div>
      </div>
      <div className="flex shrink-0 items-center gap-2">
        {ragInfo && <StatusPill pill={ragInfo} />}
        <StatusPill pill={conn} />
        {onMemoryOpen && (
          <Button
            type="button"
            size="icon"
            variant="ghost"
            aria-label="메모리"
            aria-pressed={memoryOpen}
            onClick={onMemoryOpen}
          >
            <BookText className="size-4" aria-hidden="true" />
          </Button>
        )}
      </div>
    </header>
  );
}
