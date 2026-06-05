// Vendored from Vercel AI Elements (registry.ai-sdk.dev/shimmer.json), decision 06.
//
// ADAPTATION: the upstream Shimmer animates a moving gradient via `motion/react`.
// To avoid pulling the `motion` package into the bundle (zero-runtime-CDN budget,
// rules.md), this keeps the SAME API (`{ children, as, className }`) but renders a
// lightweight CSS pulse instead of the motion gradient. Used only by Plan/Reasoning
// titles while streaming — a dim animated label, not a load-bearing visual.
import { cn } from "@/lib/utils";
import { type ElementType, memo } from "react";

export type TextShimmerProps = {
  children: string;
  as?: ElementType;
  className?: string;
  duration?: number;
  spread?: number;
};

const ShimmerComponent = ({
  children,
  as: Component = "span",
  className,
}: TextShimmerProps) => {
  const C = Component as ElementType;
  return (
    <C
      className={cn(
        "inline-block animate-pulse text-muted-foreground",
        className,
      )}
    >
      {children}
    </C>
  );
};

export const Shimmer = memo(ShimmerComponent);
