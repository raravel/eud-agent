// Vendored from Vercel AI Elements (registry.ai-sdk.dev/shimmer.json), decision 06.
//
// ADAPTATION: the upstream Shimmer animates a moving gradient via `motion/react`.
// To avoid pulling the `motion` package into the bundle (zero-runtime-CDN budget,
// rules.md), the SAME moving-gradient look is reproduced with a pure-CSS
// animation: a background-clipped text gradient swept by the `animate-shimmer`
// keyframes (src/index.css). Same API (`{ children, as, className, duration }`),
// zero new runtime dependency; `spread` is accepted but unused (the highlight
// width is fixed in the gradient). Used by Plan/Reasoning titles and the
// ConversationLog waiting indicator while streaming.
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
  duration = 2,
}: TextShimmerProps) => {
  const C = Component as ElementType;
  return (
    <C
      className={cn(
        "inline-block animate-shimmer bg-clip-text text-transparent",
        "bg-[length:200%_100%]",
        "bg-[linear-gradient(90deg,var(--color-muted-foreground)_0%,var(--color-muted-foreground)_35%,var(--color-foreground)_50%,var(--color-muted-foreground)_65%,var(--color-muted-foreground)_100%)]",
        className,
      )}
      style={{ animationDuration: `${duration}s` }}
    >
      {children}
    </C>
  );
};

export const Shimmer = memo(ShimmerComponent);
