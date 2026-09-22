import { useEffect, useState } from "react";
import { AlertCircle, Check, Copy } from "lucide-react";

import { Button } from "@/components/ui/button";
import { shortSessionId } from "@/lib/sessionId";
import { cn } from "@/lib/utils";

export interface SessionIdCopyButtonProps {
  /** The full session id that lands on the clipboard. */
  id: string;
  /** The session name the accessible label starts with. */
  name: string;
  className?: string;
  disabled?: boolean;
}

type CopyState = "idle" | "copied" | "failed";

const RESET_DELAY_MS = 1_800;

/**
 * A chip that shows a session's short id and copies the full id to the
 * clipboard. The icon confirms the result for a moment and a live region
 * announces it, so both session lists (EPS sidebar, Map history) share one
 * behaviour without a toaster.
 */
export function SessionIdCopyButton({
  id,
  name,
  className,
  disabled = false,
}: SessionIdCopyButtonProps) {
  const [state, setState] = useState<CopyState>("idle");

  useEffect(() => {
    if (state === "idle") return;
    const timer = window.setTimeout(() => setState("idle"), RESET_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [state]);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(id);
      setState("copied");
    } catch {
      setState("failed");
    }
  };

  const status =
    state === "copied"
      ? "세션 ID를 복사했습니다."
      : state === "failed"
        ? "세션 ID를 복사하지 못했습니다. 클립보드 접근을 허용한 뒤 다시 시도하세요."
        : "";
  const Icon = state === "copied" ? Check : state === "failed" ? AlertCircle : Copy;

  return (
    <>
      <Button
        type="button"
        size="sm"
        variant="ghost"
        className={cn(
          "h-5 gap-1 rounded px-1 font-mono text-[10px] tracking-tight text-muted-foreground/80 hover:text-foreground",
          state === "copied" && "text-primary hover:text-primary",
          state === "failed" && "text-destructive hover:text-destructive",
          className,
        )}
        aria-label={`${name} 세션 ID 복사`}
        title={status || `세션 ID ${id} 복사`}
        disabled={disabled}
        onClick={() => void copy()}
      >
        <code>{shortSessionId(id)}</code>
        <Icon className="size-3" aria-hidden="true" />
      </Button>
      <span role="status" aria-live="polite" className="sr-only">
        {status}
      </span>
    </>
  );
}
