import type { ReactNode } from "react";
import { CheckIcon } from "lucide-react";

import { cn } from "@/lib/utils";

interface ImportStepProps {
  readonly number: number;
  readonly title: string;
  readonly description: string;
  readonly state: "pending" | "current" | "complete" | "invalid";
  readonly children: ReactNode;
}

export function ImportStep({ number, title, description, state, children }: ImportStepProps) {
  return (
    <li
      aria-current={state === "current" ? "step" : undefined}
      className={cn(
        "grid grid-cols-[2rem_minmax(0,1fr)] gap-3 rounded-xl border p-4",
        state === "current" && "border-primary/50 bg-primary/5",
        state === "complete" && "border-emerald-500/30 bg-emerald-500/5",
        state === "invalid" && "border-destructive/40 bg-destructive/5",
        state === "pending" && "border-border bg-card/40",
      )}
    >
      <span
        className={cn(
          "flex size-8 items-center justify-center rounded-full border text-sm font-semibold",
          state === "complete" && "border-emerald-500/40 bg-emerald-500/15 text-emerald-300",
          state === "current" && "border-primary bg-primary text-primary-foreground",
          state === "invalid" && "border-destructive/50 text-destructive",
          state === "pending" && "border-border bg-muted text-muted-foreground",
        )}
      >
        {state === "complete" ? <CheckIcon aria-hidden className="size-4" /> : number}
      </span>
      <div className="min-w-0">
        <h3 className="text-sm font-semibold">{title}</h3>
        <p className="mt-1 break-keep text-sm leading-6 text-muted-foreground">{description}</p>
        <div className="mt-3">{children}</div>
      </div>
    </li>
  );
}
