"use client";

import { type ComponentProps, createContext, useContext } from "react";

import { Button } from "@/components/ui/button";
import {
  HoverCard,
  HoverCardContent,
  HoverCardTrigger,
} from "@/components/ui/hover-card";
import { Progress } from "@/components/ui/progress";
import { cn } from "@/lib/utils";

const PERCENT_MAX = 100;
const ICON_RADIUS = 10;
const ICON_CENTER = 12;
const ICON_STROKE_WIDTH = 2;
const PERCENT_FORMATTER = new Intl.NumberFormat("ko-KR", {
  style: "percent",
  maximumFractionDigits: 1,
});
const TOKEN_FORMATTER = new Intl.NumberFormat("ko-KR", {
  notation: "compact",
  maximumFractionDigits: 1,
});

type ContextValue = {
  usedTokens: number;
  maxTokens: number;
};

const ContextContext = createContext<ContextValue | null>(null);

function useContextValue(): ContextValue {
  const value = useContext(ContextContext);
  if (value === null) {
    throw new Error("Context components must be used within Context");
  }
  return value;
}

function usageRatio(usedTokens: number, maxTokens: number): number {
  if (maxTokens <= 0) return 0;
  return Math.min(Math.max(usedTokens / maxTokens, 0), 1);
}

export type ContextProps = ComponentProps<typeof HoverCard> & ContextValue;

export function Context({
  usedTokens,
  maxTokens,
  ...props
}: ContextProps) {
  return (
    <ContextContext.Provider value={{ usedTokens, maxTokens }}>
      <HoverCard closeDelay={100} openDelay={100} {...props} />
    </ContextContext.Provider>
  );
}

function ContextIcon() {
  const { usedTokens, maxTokens } = useContextValue();
  const circumference = 2 * Math.PI * ICON_RADIUS;
  const ratio = usageRatio(usedTokens, maxTokens);

  return (
    <svg
      aria-hidden="true"
      className="size-5"
      viewBox="0 0 24 24"
    >
      <circle
        cx={ICON_CENTER}
        cy={ICON_CENTER}
        fill="none"
        opacity="0.25"
        r={ICON_RADIUS}
        stroke="currentColor"
        strokeWidth={ICON_STROKE_WIDTH}
      />
      <circle
        cx={ICON_CENTER}
        cy={ICON_CENTER}
        fill="none"
        opacity="0.8"
        r={ICON_RADIUS}
        stroke="currentColor"
        strokeDasharray={`${circumference} ${circumference}`}
        strokeDashoffset={circumference * (1 - ratio)}
        strokeLinecap="round"
        strokeWidth={ICON_STROKE_WIDTH}
        style={{ transform: "rotate(-90deg)", transformOrigin: "center" }}
      />
    </svg>
  );
}

export type ContextTriggerProps = ComponentProps<typeof Button>;

export function ContextTrigger({ children, ...props }: ContextTriggerProps) {
  const { usedTokens, maxTokens } = useContextValue();
  const ratio = usageRatio(usedTokens, maxTokens);
  const percentage = PERCENT_FORMATTER.format(ratio);

  return (
    <HoverCardTrigger asChild>
      {children ?? (
        <Button
          type="button"
          variant="ghost"
          size="sm"
          aria-label={`컨텍스트 ${percentage} 사용`}
          {...props}
        >
          <span className="font-mono text-xs text-muted-foreground tabular-nums">
            {percentage}
          </span>
          <ContextIcon />
        </Button>
      )}
    </HoverCardTrigger>
  );
}

export type ContextContentProps = ComponentProps<typeof HoverCardContent>;

export function ContextContent({
  className,
  ...props
}: ContextContentProps) {
  return (
    <HoverCardContent
      className={cn("min-w-64 divide-y overflow-hidden p-0", className)}
      {...props}
    />
  );
}

export type ContextContentHeaderProps = ComponentProps<"div">;

export function ContextContentHeader({
  children,
  className,
  ...props
}: ContextContentHeaderProps) {
  const { usedTokens, maxTokens } = useContextValue();
  const ratio = usageRatio(usedTokens, maxTokens);

  return (
    <div className={cn("w-full space-y-2 p-3", className)} {...props}>
      {children ?? (
        <>
          <div className="flex items-center justify-between gap-3 text-xs">
            <span className="font-medium">현재 컨텍스트</span>
            <span className="font-mono text-muted-foreground tabular-nums">
              {TOKEN_FORMATTER.format(usedTokens)} / {TOKEN_FORMATTER.format(maxTokens)}
            </span>
          </div>
          <Progress
            aria-label="컨텍스트 사용률"
            value={ratio * PERCENT_MAX}
          />
        </>
      )}
    </div>
  );
}

export type ContextContentBodyProps = ComponentProps<"div">;

export function ContextContentBody({
  children,
  className,
  ...props
}: ContextContentBodyProps) {
  return (
    <div className={cn("w-full p-3", className)} {...props}>
      {children}
    </div>
  );
}
