/**
 * One pane of the chat tab's split layout: a session header and that
 * session's conversation, read from its own store. Only the focused pane
 * (the selected session) takes input; the others show their session live and
 * take focus on click.
 */
import { useSyncExternalStore, type ReactNode } from "react";
import { Columns2, Rows2, Square, X } from "lucide-react";

import { ConversationLog } from "@/components/ConversationLog";
import type { SessionActivity } from "@/components/SessionSidebar";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useDefaultLayout } from "react-resizable-panels";
import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup,
} from "@/components/ui/resizable";
import type { WorkerStatusKind } from "@/lib/protocol";
import { WORKER_PROFILE_LABELS, type WorkerProfile } from "@/lib/workerProfiles";
import {
  paneSizeStorage,
  type PaneDirection,
  type PaneNode,
  type PaneSplit,
} from "@/lib/paneLayout";
import { cn } from "@/lib/utils";
import type { LogEntry, PanelStore } from "@/state/store";

export const WORKER_STATUS_LABELS: Record<WorkerStatusKind, string> = {
  queued: "대기",
  running: "실행 중",
  settled: "완료",
  failed: "실패",
  cancelled: "취소됨",
  interrupted: "중단됨",
};

export interface SessionPaneProps {
  sessionId: string;
  name: string;
  providerLabel: string;
  activity: SessionActivity;
  store: PanelStore;
  focused: boolean;
  /** Name of the parent session when this session is a background worker. */
  workerParentName?: string;
  workerStatus?: WorkerStatusKind;
  readonly workerProfile?: WorkerProfile;
  canSplit: boolean;
  canClose: boolean;
  cancellable: boolean;
  onFocus(): void;
  onSplit(direction: PaneDirection): void;
  onClose(): void;
  onCancel(): void;
  /** Focused pane only: suggestion chips and message editing. */
  onSuggestion?: (text: string) => void;
  suggestionsEnabled?: boolean;
  onEditMessage?: (entry: LogEntry) => void;
  editDisabled?: boolean;
  /** Interactive cards (a question to answer) under the focused pane's log. */
  children?: ReactNode;
}

function ActivityText({ activity }: { activity: SessionActivity }) {
  switch (activity) {
    case "running_read":
      return <span className="text-primary">분석 중</span>;
    case "running_write":
      return <span className="text-primary">변경 중 · 격리 워크스페이스</span>;
    case "waiting_input":
      return <span className="text-amber-400">응답 필요</span>;
    case "review":
      return <span className="text-amber-400">검토 필요</span>;
    default:
      return null;
  }
}

/**
 * One split of the pane tree: two resizable panes. Sizes persist per split id
 * through the library's default-layout storage.
 */
export function PaneSplitView({
  split,
  renderNode,
}: {
  split: PaneSplit;
  renderNode(node: PaneNode): ReactNode;
}) {
  const { defaultLayout, onLayoutChanged } = useDefaultLayout({
    id: `eud-agent.pane-split:${split.id}`,
    panelIds: [split.a.id, split.b.id],
    storage: paneSizeStorage,
  });
  return (
    <ResizablePanelGroup
      id={split.id}
      orientation={split.direction}
      defaultLayout={defaultLayout}
      onLayoutChanged={onLayoutChanged}
    >
      <ResizablePanel id={split.a.id} minSize="15%" className="h-full min-h-0 min-w-0">
        {renderNode(split.a)}
      </ResizablePanel>
      <ResizableHandle
        withHandle
        aria-label={split.direction === "horizontal" ? "좌우 창 크기 조절" : "상하 창 크기 조절"}
      />
      <ResizablePanel id={split.b.id} minSize="15%" className="h-full min-h-0 min-w-0">
        {renderNode(split.b)}
      </ResizablePanel>
    </ResizablePanelGroup>
  );
}

function PaneAction({
  label,
  disabled,
  onClick,
  children,
}: {
  label: string;
  disabled?: boolean;
  onClick(): void;
  children: ReactNode;
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          type="button"
          size="icon"
          variant="ghost"
          className="size-7"
          aria-label={label}
          disabled={disabled}
          onClick={(event) => {
            event.stopPropagation();
            onClick();
          }}
        >
          {children}
        </Button>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}

export function SessionPane({
  sessionId,
  name,
  providerLabel,
  activity,
  store,
  focused,
  workerParentName,
  workerStatus,
  workerProfile,
  canSplit,
  canClose,
  cancellable,
  onFocus,
  onSplit,
  onClose,
  onCancel,
  onSuggestion,
  suggestionsEnabled,
  onEditMessage,
  editDisabled,
  children,
}: SessionPaneProps) {
  const state = useSyncExternalStore(store.subscribe, store.getState, store.getState);
  const awaitsInput = !focused && (state.ask !== null || state.phase === "plan_review");
  return (
    <section
      aria-label={`${name} 대화 창${focused ? " (선택됨)" : ""}`}
      data-session-id={sessionId}
      data-focused={focused ? "true" : "false"}
      onMouseDownCapture={() => {
        if (!focused) onFocus();
      }}
      onFocusCapture={() => {
        if (!focused) onFocus();
      }}
      className={cn(
        "flex h-full min-h-0 w-full min-w-0 flex-col overflow-hidden",
        focused ? "ring-1 ring-inset ring-primary/60" : "",
      )}
    >
      <div
        className={cn(
          // A container, so the lower-priority items below drop out as the
          // pane narrows: the activity label first, then the provider chip.
          "@container flex min-h-10 items-center gap-2 border-b border-border px-3 text-xs",
          focused ? "bg-primary/5" : "bg-card/20",
        )}
      >
        <span className="min-w-12 flex-1 truncate font-medium text-foreground" title={name}>
          {name}
        </span>
        {workerParentName !== undefined && (
          <Badge
            variant="outline"
            className="max-w-44 shrink justify-start"
            title={`${workerProfile ? WORKER_PROFILE_LABELS[workerProfile] : "작업자"} · ${workerParentName}`}
          >
            <span className="shrink-0">{workerProfile ? WORKER_PROFILE_LABELS[workerProfile] : "작업자"} ·</span>
            <span className="min-w-0 truncate">{workerParentName}</span>
          </Badge>
        )}
        {workerStatus !== undefined && (
          <span className="shrink-0 text-muted-foreground">
            {WORKER_STATUS_LABELS[workerStatus]}
          </span>
        )}
        <span className="hidden shrink-0 rounded-full border border-border bg-muted/50 px-2 py-0.5 text-[11px] text-muted-foreground @min-[26rem]:inline-block">
          {providerLabel}
        </span>
        {awaitsInput ? (
          <Badge variant="secondary" className="shrink-0 text-amber-600 dark:text-amber-400">
            응답 필요
          </Badge>
        ) : (
          <span className="hidden min-w-0 shrink truncate @min-[36rem]:block">
            <ActivityText activity={activity} />
          </span>
        )}
        <div className="flex shrink-0 items-center">
          <PaneAction label="좌우 분할" disabled={!canSplit} onClick={() => onSplit("horizontal")}>
            <Columns2 className="size-4" aria-hidden="true" />
          </PaneAction>
          <PaneAction label="상하 분할" disabled={!canSplit} onClick={() => onSplit("vertical")}>
            <Rows2 className="size-4" aria-hidden="true" />
          </PaneAction>
          {cancellable && (
            <PaneAction label="중단" onClick={onCancel}>
              <Square className="size-3.5" aria-hidden="true" />
            </PaneAction>
          )}
          <PaneAction label="창 닫기" disabled={!canClose} onClick={onClose}>
            <X className="size-4" aria-hidden="true" />
          </PaneAction>
        </div>
      </div>
      <ConversationLog
        log={state.log}
        phase={state.phase}
        turn={state.turn}
        ragLoading={state.rag === "loading"}
        onSuggestion={focused ? onSuggestion : undefined}
        suggestionsEnabled={focused && suggestionsEnabled}
        onEditMessage={focused ? onEditMessage : undefined}
        editDisabled={!focused || editDisabled}
      />
      {focused && children}
    </section>
  );
}
