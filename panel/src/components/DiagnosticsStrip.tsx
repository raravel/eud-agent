/**
 * Advisory diagnostics strip (features/03 ## Behaviors → Diagnostics):
 *   "advisory strip below the review area; dismissible; NEVER blocks Apply."
 * rules.md: epscript-lsp diagnostics are advisory only — they annotate, never
 * block apply; absence must not break the flow. This component owns ZERO Apply
 * gating; it only displays + dismisses.
 */
import { XIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { Diagnostic } from "@/ws/protocol";

export interface DiagnosticsStripProps {
  diagnostics: Diagnostic[];
  dismissed: boolean;
  onDismiss(): void;
}

/** Best-effort text extraction from a string or structured diagnostic. */
function diagText(d: Diagnostic): string {
  if (typeof d === "string") return d;
  const parts: string[] = [];
  if (typeof d.line === "number") parts.push(`L${d.line}`);
  const msg = d.message ?? d.text;
  if (msg) parts.push(msg);
  const text = parts.join(": ");
  return text || JSON.stringify(d);
}

export function DiagnosticsStrip({
  diagnostics,
  dismissed,
  onDismiss,
}: DiagnosticsStripProps) {
  if (dismissed || diagnostics.length === 0) {
    return null;
  }
  return (
    <div className="mx-4 mb-2 flex items-start gap-2 rounded-md border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs text-amber-300">
      <ul className="flex-1 space-y-0.5">
        {diagnostics.map((d, i) => (
          <li key={i} className="whitespace-pre-wrap break-words">
            {diagText(d)}
          </li>
        ))}
      </ul>
      <Button
        type="button"
        variant="ghost"
        size="icon-xs"
        onClick={onDismiss}
        aria-label="닫기"
        className="shrink-0 text-amber-300 hover:text-amber-200"
      >
        <XIcon className="size-3" />
      </Button>
    </div>
  );
}
