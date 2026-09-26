/**
 * One object's graphic, played the way its iscript plays it.
 *
 * The Rust side runs one animation slot of the image's script and hands back
 * every frame that slot shows as one grid image, plus the timeline: which cell,
 * for how many game ticks, mirrored or not, and the step the script loops back
 * to. This component only walks that timeline — it never interprets iscript.
 *
 * Ticks are drawn at StarCraft's "fastest" speed. An animation that ends (a
 * Death, an explosion) rests on its last frame for a moment and starts over, so
 * it can be watched more than once. Under reduced motion it starts paused on
 * its first frame, and the play button is always there to start or stop it.
 */
import { useEffect, useState } from "react";
import { Pause, Play } from "lucide-react";

import { Button } from "@/components/ui/button";
import type { DatWikiGraphic } from "@/lib/ipc";
import { cn } from "@/lib/utils";

/** Milliseconds per game tick at the "fastest" game speed. */
export const TICK_MS = 42;
/** How long a finished animation rests before it plays again. */
export const END_PAUSE_MS = 1000;

function prefersReducedMotion(): boolean {
  return (
    typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches
  );
}

/** Whether the timeline ever shows more than one picture. */
function isAnimated(graphic: DatWikiGraphic): boolean {
  const first = graphic.steps[0];
  return graphic.steps.some((step) => step.cell !== first.cell || step.flip !== first.flip);
}

export interface DatWikiAnimationProps {
  graphic: DatWikiGraphic;
  /** How much the GRP's own pixel size is enlarged. */
  scale: number;
  /** What the picture shows, for anyone not looking at it. */
  label: string;
  className?: string;
}

export function DatWikiAnimation({ graphic, scale, label, className }: DatWikiAnimationProps) {
  const animated = isAnimated(graphic);
  const [index, setIndex] = useState(0);
  const [playing, setPlaying] = useState(() => !prefersReducedMotion());

  // A new graphic (another object, another slot) starts from its first step.
  useEffect(() => {
    setIndex(0);
  }, [graphic]);

  useEffect(() => {
    if (!animated || !playing) return;
    const step = graphic.steps[index] ?? graphic.steps[0];
    const last = index >= graphic.steps.length - 1;
    const ends = last && graphic.loopStart === undefined;
    const delay = Math.max(1, step.ticks) * TICK_MS + (ends ? END_PAUSE_MS : 0);
    const timer = window.setTimeout(() => {
      setIndex(last ? (graphic.loopStart ?? 0) : index + 1);
    }, delay);
    return () => window.clearTimeout(timer);
  }, [animated, playing, graphic, index]);

  const step = graphic.steps[index] ?? graphic.steps[0];
  const width = graphic.frameWidth * scale;
  const height = graphic.frameHeight * scale;
  const rows = Math.max(1, Math.ceil(graphic.frames / graphic.columns));
  const column = step.cell % graphic.columns;
  const row = Math.floor(step.cell / graphic.columns);

  return (
    <div className={cn("flex flex-col items-center gap-1", className)}>
      <span
        role="img"
        aria-label={label}
        data-cell={step.cell}
        className="block shrink-0 rounded bg-muted/40"
        style={{
          width,
          height,
          backgroundImage: `url(${graphic.png})`,
          backgroundSize: `${graphic.columns * width}px ${rows * height}px`,
          backgroundPosition: `-${column * width}px -${row * height}px`,
          backgroundRepeat: "no-repeat",
          imageRendering: "pixelated",
          transform: step.flip ? "scaleX(-1)" : undefined,
        }}
      />
      {animated && (
        <Button
          type="button"
          size="sm"
          variant="ghost"
          className="h-6 px-2 text-xs"
          aria-pressed={playing}
          onClick={() => setPlaying((value) => !value)}
        >
          {playing ? (
            <Pause className="size-3.5" aria-hidden="true" />
          ) : (
            <Play className="size-3.5" aria-hidden="true" />
          )}
          {playing ? "일시 정지" : "재생"}
        </Button>
      )}
    </div>
  );
}
