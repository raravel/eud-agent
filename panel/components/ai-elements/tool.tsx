// Vendored from Vercel AI Elements (registry.ai-sdk.dev/tool.json), decision 06.
//
// ADAPTATION: the upstream tool.tsx is typed against the AI SDK `ToolUIPart`
// (input-streaming/input-available/output-available/output-error states) and
// renders tool input/output through a `code-block` (shiki) sub-component. The
// eud-agent panel only forwards a tool NAME and a coarse running|done state via
// `agent_event` (no structured input/output), so this is adapted to a local
// `ToolState` union and the heavy `code-block` dependency is dropped. The
// Collapsible + Badge composition and styling are kept faithful to upstream.
"use client";

import { Badge } from "@/components/ui/badge";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { cn } from "@/lib/utils";
import {
  CheckCircleIcon,
  ChevronDownIcon,
  ClockIcon,
  WrenchIcon,
  XCircleIcon,
} from "lucide-react";
import type { ComponentProps, ReactNode } from "react";

/**
 * Coarse tool state forwarded by the server (tool_call → running, tool_result →
 * done, or failed when the server-reported status is not "completed" — EUD-068).
 */
export type ToolState = "running" | "done" | "failed";

export type ToolProps = ComponentProps<typeof Collapsible>;

export const Tool = ({ className, ...props }: ToolProps) => (
  <Collapsible
    className={cn("not-prose group w-full rounded-md border", className)}
    {...props}
  />
);

export type ToolHeaderProps = {
  title: string;
  state: ToolState;
  className?: string;
};

const STATE_BADGE: Record<ToolState, { label: string; icon: ReactNode }> = {
  running: {
    label: "실행 중",
    icon: <ClockIcon className="size-4 animate-pulse" />,
  },
  done: {
    label: "완료",
    icon: <CheckCircleIcon className="size-4 text-emerald-500" />,
  },
  failed: {
    label: "실패",
    icon: <XCircleIcon className="size-4 text-destructive" />,
  },
};

export const ToolHeader = ({
  className,
  title,
  state,
  ...props
}: ToolHeaderProps) => (
  <CollapsibleTrigger
    className={cn(
      "flex w-full items-center justify-between gap-4 p-3",
      className,
    )}
    {...props}
  >
    <div className="flex items-center gap-2">
      <WrenchIcon className="size-4 text-muted-foreground" />
      <span className="font-medium text-sm">{title}</span>
      <Badge className="gap-1.5 rounded-full text-xs" variant="secondary">
        {STATE_BADGE[state].icon}
        {STATE_BADGE[state].label}
      </Badge>
    </div>
    <ChevronDownIcon className="size-4 text-muted-foreground transition-transform group-data-[state=open]:rotate-180" />
  </CollapsibleTrigger>
);

export type ToolContentProps = ComponentProps<typeof CollapsibleContent>;

export const ToolContent = ({ className, ...props }: ToolContentProps) => (
  <CollapsibleContent
    className={cn(
      "data-[state=closed]:fade-out-0 data-[state=closed]:slide-out-to-top-2 data-[state=open]:slide-in-from-top-2 text-popover-foreground outline-none data-[state=closed]:animate-out data-[state=open]:animate-in",
      className,
    )}
    {...props}
  />
);
