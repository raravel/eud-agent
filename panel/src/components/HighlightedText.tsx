/**
 * Text with search matches marked. `ranges` are `[start, end)` offsets in
 * UTF-16 code units (JS string indices), as the backend search reports them;
 * overlapping or out-of-bounds ranges are clipped rather than trusted.
 */
import type { TextRange } from "@/lib/ipc";

export interface HighlightedTextProps {
  text: string;
  ranges: readonly TextRange[];
  className?: string;
}

export function HighlightedText({ text, ranges, className }: HighlightedTextProps) {
  // Split into plain and marked runs, in order; each range starts no earlier
  // than where the previous one ended.
  const parts: { text: string; match: boolean }[] = [];
  let cursor = 0;
  for (const [rawStart, rawEnd] of [...ranges].sort((left, right) => left[0] - right[0])) {
    const start = Math.max(cursor, Math.min(rawStart, text.length));
    const end = Math.min(rawEnd, text.length);
    if (end <= start) continue;
    if (start > cursor) parts.push({ text: text.slice(cursor, start), match: false });
    parts.push({ text: text.slice(start, end), match: true });
    cursor = end;
  }
  if (cursor < text.length) parts.push({ text: text.slice(cursor), match: false });
  return (
    <span className={className}>
      {parts.map((part, index) =>
        part.match ? (
          <mark key={index} className="rounded-sm bg-primary/25 px-px text-foreground">
            {part.text}
          </mark>
        ) : (
          <span key={index}>{part.text}</span>
        ),
      )}
    </span>
  );
}
