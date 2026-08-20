/**
 * Header (features/06 ## Behaviors → Status visibility): app title, project
 * name (from `status`), connection-state transitions (연결 중 → 연결됨 →
 * 재연결 중), and the RAG model state with elapsed seconds while loading
 * (`rag_warmup` started → done, elapsed formatted via lib/progress). Korean
 * labels throughout.
 */
import { MonitorPlay, PanelRightClose, PanelRightOpen } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
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
  /** Whether the EUD Editor bridge is currently connected (store.editorConnected).
   *  Drives the "에디터 켜기" button's disabled state — a connected editor must not be
   *  re-launched (single-instance topology) — and the connection chip's no-project
   *  suffix. */
  editorConnected?: boolean;
  /** Whether a project is open in the editor (store.hasProject). When the editor is
   *  connected but this is false, the connection chip reads "연결됨 · 프로젝트 없음". */
  hasProject?: boolean;
  /** Whether a launch request is in flight (App awaits `launch_editor`). */
  launchPending?: boolean;
  /** Launch the configured EUD Editor 3 install. */
  onLaunchEditor?: () => void;
  /** Toggle the project tools sidebar. */
  onProjectPanelToggle?: () => void;
  /** Whether the project tools sidebar is currently visible. */
  projectPanelOpen?: boolean;
}

/** One status pill descriptor: label + tone classes + whether it is in flight. */
interface Pill {
  label: string;
  tone: string;
  /** True while the state is transitional — the dot pulses. */
  busy?: boolean;
}

/** Connection-state label + pill tone from connected/phase.
 *
 * When the editor is connected but no project is open (`editorConnected && !hasProject`),
 * the connected label carries a "· 프로젝트 없음" suffix — the no-project state is shown
 * here rather than as a failed-`list` log line. The suffix is gated on `editorConnected`
 * so a downed editor (handled by the ConnectionNotice banner) is not mislabeled. */
function connState(
  connected: boolean,
  phase: Phase,
  editorConnected: boolean,
  hasProject: boolean,
): Pill {
  if (connected) {
    return {
      label:
        editorConnected && !hasProject ? "연결됨 · 프로젝트 없음" : "연결됨",
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
  editorConnected = false,
  hasProject = true,
  launchPending = false,
  onLaunchEditor,
  onProjectPanelToggle,
  projectPanelOpen = false,
}: HeaderProps) {
  const conn = connState(connected, phase, editorConnected, hasProject);
  const ragInfo = ragPill(rag);
  return (
    <TooltipProvider delayDuration={300}>
    <header className="flex items-center justify-between gap-3 border-b border-border bg-card/60 px-4 py-2.5 backdrop-blur">
      {/* Branding tile + title + project context (same identity tile as the
          SetupScreen, scaled down). */}
      <div className="flex min-w-0 items-center gap-2.5">
        <span
          aria-hidden
          className="flex size-8 shrink-0 items-center justify-center overflow-hidden rounded-lg border border-emerald-500/30 bg-emerald-500/15"
        >
          <img
            src="/eud-agent.png"
            alt=""
            className="size-6 rounded-md object-contain"
          />
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
        {onLaunchEditor && (
          <Button
            type="button"
            size="sm"
            variant="outline"
            className="gap-1.5"
            // A connected editor must not be re-launched (single-instance topology);
            // stays visible-but-disabled so the control's home in the header is stable.
            disabled={editorConnected || launchPending}
            onClick={onLaunchEditor}
          >
            <MonitorPlay className="size-4" aria-hidden="true" />
            {launchPending ? "여는 중…" : "에디터 켜기"}
          </Button>
        )}
        {ragInfo && <StatusPill pill={ragInfo} />}
        <StatusPill pill={conn} />
        {onProjectPanelToggle && (
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                type="button"
                size="icon"
                variant="ghost"
                aria-label={`프로젝트 도구 ${projectPanelOpen ? "닫기" : "열기"}`}
                aria-pressed={projectPanelOpen}
                onClick={onProjectPanelToggle}
              >
                {projectPanelOpen ? (
                  <PanelRightClose className="size-4" aria-hidden="true" />
                ) : (
                  <PanelRightOpen className="size-4" aria-hidden="true" />
                )}
              </Button>
            </TooltipTrigger>
            <TooltipContent>
              프로젝트 도구 {projectPanelOpen ? "닫기" : "열기"}
            </TooltipContent>
          </Tooltip>
        )}
      </div>
    </header>
    </TooltipProvider>
  );
}
