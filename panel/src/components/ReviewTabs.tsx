/**
 * Review area Tabs (features/03 ## Behaviors → Review):
 *   - preview: read-only code (escaped) + lang label; display truncated at 1 MiB
 *     (UTF-16 metric) with a notice. Apply ALWAYS sends full text (the edit
 *     buffer), never this truncated preview.
 *   - diff: server-supplied unified diff with per-line +/- coloring; HIDDEN in
 *     new-file mode (there is no original to diff against).
 *   - edit: Monaco (local-bundle wiring) seeded with the edit buffer — the
 *     Monaco buffer is the SINGLE SOURCE OF TRUTH for Apply.
 *
 * DEP PRUNING (carry-forward): the preview is a plain escaped <pre> (React
 * escapes text children), NOT the vendored shiki CodeBlock — keeps shiki out of
 * the eager bundle. The diff is classified by src/lib/diff (server computes it).
 */
import { lazy, Suspense } from "react";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Spinner } from "@/components/ui/spinner";
import { classifyDiff, type DiffLineKind } from "@/lib/diff";
import { truncateForDisplay } from "@/lib/truncate";
import { cn } from "@/lib/utils";
import type { ReviewState } from "@/state/store";

// Monaco (and its local-bundle wiring + workers) is multi-MB; load it lazily so
// it lands in a separate async chunk and the eager entry stays small. The whole
// editor — including the `@/editor/monaco` side-effect — lives in MonacoEditor.
const MonacoEditor = lazy(() => import("@/components/MonacoEditor"));

export interface ReviewTabsProps {
  review: ReviewState;
  newFileMode: boolean;
  /** Current Monaco buffer — the Apply source of truth. */
  editedCode: string;
  onEditCode(next: string): void;
}

/** Per-kind coloring + testid for a classified diff line. */
const DIFF_LINE_STYLE: Record<DiffLineKind, string> = {
  add: "bg-emerald-500/10 text-emerald-400",
  del: "bg-red-500/10 text-red-400",
  hunk: "bg-sky-500/10 text-sky-400",
  file: "font-semibold text-muted-foreground",
  context: "text-foreground/80",
};

export function ReviewTabs({
  review,
  newFileMode,
  editedCode,
  onEditCode,
}: ReviewTabsProps) {
  const preview = truncateForDisplay(review.code);
  const diffLines = classifyDiff(review.diff);

  return (
    <Tabs defaultValue="preview" className="px-4">
      <TabsList variant="line">
        <TabsTrigger value="preview">미리보기</TabsTrigger>
        {!newFileMode && <TabsTrigger value="diff">변경</TabsTrigger>}
        <TabsTrigger value="edit">편집</TabsTrigger>
      </TabsList>

      <TabsContent value="preview">
        <div className="overflow-hidden rounded-md border border-border">
          <div className="flex items-center justify-between border-b border-border bg-muted/60 px-3 py-1.5 text-xs text-muted-foreground">
            <span>{review.lang}</span>
            {preview.truncated && (
              <span data-testid="preview-truncated-notice" className="text-amber-400">
                미리보기가 1 MiB로 잘렸습니다 (적용 시 전체 코드 전송)
              </span>
            )}
          </div>
          <pre
            data-testid="preview-code"
            className="max-h-72 overflow-auto p-3 font-mono text-xs leading-relaxed text-foreground"
          >
            {preview.text}
          </pre>
        </div>
      </TabsContent>

      {!newFileMode && (
        <TabsContent value="diff">
          <pre className="max-h-72 overflow-auto rounded-md border border-border p-3 font-mono text-xs leading-relaxed">
            {diffLines.map((line, i) => (
              <div
                key={i}
                data-testid={`diff-line-${line.kind}`}
                className={cn("whitespace-pre-wrap", DIFF_LINE_STYLE[line.kind])}
              >
                {line.text === "" ? " " : line.text}
              </div>
            ))}
          </pre>
        </TabsContent>
      )}

      <TabsContent value="edit">
        <div className="overflow-hidden rounded-md border border-border">
          <Suspense
            fallback={
              <div className="flex h-72 items-center justify-center text-muted-foreground">
                <Spinner className="size-5" />
              </div>
            }
          >
            <MonacoEditor value={editedCode} onChange={onEditCode} />
          </Suspense>
        </div>
      </TabsContent>
    </Tabs>
  );
}
