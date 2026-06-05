/**
 * Plan review card (features/06 ## UI layout + Behaviors → Plan review):
 *   the proposed plan rendered as a markdown card; a 피드백 textarea that sends
 *   `plan_feedback{text}` (the panel STAYS in plan_review — the store keeps the
 *   card until the next `plan{revision+1}` replaces it, EUD-058); a [수정요청]
 *   button (feedback) and a [승인] button (`plan_approve{}`). Korean labels.
 *
 * Markdown rendering — DEP-PRUNING (carry-forward from ConversationLog): NO
 * streamdown/shiki pipeline and NO new runtime markdown dep. Plan markdown is
 * short and structured, so a MINIMAL line-based renderer is sufficient and
 * SAFE: every piece of source text becomes a React child (auto-escaped), never
 * dangerouslySetInnerHTML. Supported subset: ATX headings (#/##/###), unordered
 * (-/*) and ordered (1.) list items, fenced code blocks (```), blank-line
 * paragraph breaks. Inline emphasis is left as literal text by design (no HTML
 * injection surface). Anything unrecognized renders as a plain paragraph.
 *
 * Revision replacement is owned by the STORE (a higher revision replaces the
 * active plan card); this component is a thin renderer of whatever `plan` it is
 * given, so a new revision simply re-renders with the new markdown.
 */
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";
import type { PlanState } from "@/state/store";

export interface PlanViewProps {
  /** The active plan card (markdown + revision). */
  plan: PlanState;
  /** A turn is in flight (feedback/approve already sent) — disable controls. */
  pending: boolean;
  /** Send plan_feedback{text}; the App fires the WS message + store action. */
  onFeedback(text: string): void;
  /** Send plan_approve{}; the App fires the WS message + store action. */
  onApprove(): void;
}

/** A rendered markdown block (the minimal supported subset). */
type Block =
  | { kind: "heading"; level: 1 | 2 | 3; text: string }
  | { kind: "ul"; items: string[] }
  | { kind: "ol"; items: string[] }
  | { kind: "code"; text: string }
  | { kind: "p"; text: string };

const HEADING_RE = /^(#{1,3})\s+(.*)$/;
const UL_RE = /^[-*]\s+(.*)$/;
const OL_RE = /^\d+\.\s+(.*)$/;
const FENCE_RE = /^```/;

/**
 * Parse plan markdown into a flat block list. Line-based and safe: text is kept
 * verbatim as block content (rendered as React children, never as HTML).
 */
function parseMarkdown(md: string): Block[] {
  const lines = md.split("\n");
  const blocks: Block[] = [];
  let i = 0;

  while (i < lines.length) {
    const line = lines[i];

    // Fenced code block — collect until the closing fence (or EOF).
    if (FENCE_RE.test(line)) {
      const body: string[] = [];
      i += 1;
      while (i < lines.length && !FENCE_RE.test(lines[i])) {
        body.push(lines[i]);
        i += 1;
      }
      if (i < lines.length) i += 1; // consume the closing fence
      blocks.push({ kind: "code", text: body.join("\n") });
      continue;
    }

    const heading = HEADING_RE.exec(line);
    if (heading) {
      blocks.push({
        kind: "heading",
        level: heading[1].length as 1 | 2 | 3,
        text: heading[2],
      });
      i += 1;
      continue;
    }

    if (UL_RE.test(line)) {
      const items: string[] = [];
      while (i < lines.length) {
        const m = UL_RE.exec(lines[i]);
        if (!m) break;
        items.push(m[1]);
        i += 1;
      }
      blocks.push({ kind: "ul", items });
      continue;
    }

    if (OL_RE.test(line)) {
      const items: string[] = [];
      while (i < lines.length) {
        const m = OL_RE.exec(lines[i]);
        if (!m) break;
        items.push(m[1]);
        i += 1;
      }
      blocks.push({ kind: "ol", items });
      continue;
    }

    // Paragraph — accumulate non-blank lines until a blank line or a line that
    // starts a different block. Blank lines are paragraph separators.
    if (line.trim() === "") {
      i += 1;
      continue;
    }
    const para: string[] = [];
    while (
      i < lines.length &&
      lines[i].trim() !== "" &&
      !HEADING_RE.test(lines[i]) &&
      !UL_RE.test(lines[i]) &&
      !OL_RE.test(lines[i]) &&
      !FENCE_RE.test(lines[i])
    ) {
      para.push(lines[i]);
      i += 1;
    }
    blocks.push({ kind: "p", text: para.join("\n") });
  }

  return blocks;
}

const HEADING_CLASS: Record<1 | 2 | 3, string> = {
  1: "text-base font-semibold",
  2: "text-sm font-semibold",
  3: "text-sm font-medium",
};

/** Render the parsed blocks as styled text (no HTML injection). */
function Markdown({ source }: { source: string }) {
  const blocks = parseMarkdown(source);
  return (
    <div className="flex flex-col gap-2 text-sm">
      {blocks.map((block, idx) => {
        switch (block.kind) {
          case "heading":
            return (
              <div key={idx} className={HEADING_CLASS[block.level]}>
                {block.text}
              </div>
            );
          case "ul":
            return (
              <ul key={idx} className="list-disc pl-5">
                {block.items.map((item, j) => (
                  <li key={j} className="whitespace-pre-wrap break-words">
                    {item}
                  </li>
                ))}
              </ul>
            );
          case "ol":
            return (
              <ol key={idx} className="list-decimal pl-5">
                {block.items.map((item, j) => (
                  <li key={j} className="whitespace-pre-wrap break-words">
                    {item}
                  </li>
                ))}
              </ol>
            );
          case "code":
            return (
              <pre
                key={idx}
                className="overflow-x-auto rounded bg-muted/40 p-2 text-xs whitespace-pre-wrap break-words"
              >
                {block.text}
              </pre>
            );
          default:
            return (
              <p key={idx} className="whitespace-pre-wrap break-words">
                {block.text}
              </p>
            );
        }
      })}
    </div>
  );
}

export function PlanView({ plan, pending, onFeedback, onApprove }: PlanViewProps) {
  const [feedback, setFeedback] = useState("");

  function handleFeedback() {
    const text = feedback.trim();
    if (pending || text.length === 0) return;
    onFeedback(text);
    setFeedback("");
  }

  return (
    <section
      aria-label="계획 검토"
      className="flex max-h-[40vh] flex-col gap-3 overflow-y-auto border-t border-border p-4"
    >
      <div className="flex items-center gap-2">
        <span className="text-xs font-medium text-muted-foreground">
          계획안 (rev {plan.revision})
        </span>
      </div>

      <div className="rounded-lg border border-border p-3">
        <Markdown source={plan.markdown} />
      </div>

      <Textarea
        aria-label="피드백 입력"
        value={feedback}
        onChange={(e) => setFeedback(e.target.value)}
        placeholder="계획을 수정하려면 피드백을 입력하세요 (예: HP는 100으로)"
        className={cn("max-h-40 min-h-16 resize-none")}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
            e.preventDefault();
            handleFeedback();
          }
        }}
      />

      <div className="flex items-center justify-end gap-2">
        <Button
          type="button"
          variant="outline"
          disabled={pending}
          onClick={handleFeedback}
        >
          수정요청
        </Button>
        <Button type="button" disabled={pending} onClick={() => onApprove()}>
          승인
        </Button>
      </div>
    </section>
  );
}
