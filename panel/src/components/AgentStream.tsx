/**
 * Agent activity stream (features/06 ## Behaviors → Agent stream), rebuilt on the
 * vendored AI Elements (Reasoning + Tool) + Streamdown (decision 06):
 *   - `reasoning` text renders into the Reasoning block: dim/secondary, GPT-style,
 *     collapsible. It auto-opens while reasoning streams (no answer yet) and
 *     auto-collapses once the answer starts (`answerStarted`); the user can then
 *     MANUALLY re-expand it by clicking the trigger (GPT-style — F1 review fix).
 *     A user toggle wins over the auto behavior until the next turn resets it
 *     (the override clears when the per-turn reasoning buffer empties);
 *   - `tool_call`/`tool_result` render as Tool rows by tool name, with a
 *     "도구 호출 n건" summary row;
 *   - raw internal kind identifiers NEVER appear as literal text — this component
 *     only ever renders the reasoning text, the tool NAMES, and Korean labels.
 *
 * With no reasoning and no tools it renders nothing (null).
 */
import { useEffect, useMemo, useState } from "react";
import {
  FileIcon,
  FilePenLineIcon,
  ImageIcon,
  WrenchIcon,
} from "lucide-react";
import {
  Reasoning,
  ReasoningContent,
  ReasoningTrigger,
} from "@/components/ai-elements/reasoning";
import {
  Tool,
  ToolContent,
  ToolHeader,
} from "@/components/ai-elements/tool";
import {
  CodeBlock,
  CodeBlockCopyButton,
} from "@/components/ai-elements/code-block";
import { parseFileTool, type FileToolView } from "@/lib/fileTool";
import { parseImageTool, type ImageToolView } from "@/lib/imageTool";
import { classifyDiff } from "@/lib/diff";
import { cn } from "@/lib/utils";

/** One tool-call row (mirrors the store's per-turn AgentTool). */
export interface AgentTool {
  id: string;
  name: string;
  state: "running" | "done" | "failed";
  /** Tool-call argument text (EUD-068). */
  args?: string;
  /** Tool-result text (EUD-068). */
  detail?: string;
}

export interface AgentStreamProps {
  /** Accumulated reasoning delta text. */
  reasoning: string;
  /** True once answer delta text has begun (collapses the Reasoning block). */
  answerStarted: boolean;
  /** Tool rows from tool_call/tool_result. */
  tools: AgentTool[];
  /** True while the turn is in flight (store phase === "thinking"). */
  live: boolean;
}

export function AgentStream({
  reasoning,
  answerStarted,
  tools,
  live,
}: AgentStreamProps) {
  // User override of the auto open/collapse behavior (F1): null = follow the
  // auto behavior; true/false = the user explicitly opened/closed the block.
  const [userOpen, setUserOpen] = useState<boolean | null>(null);

  const hasReasoning = reasoning.length > 0;

  // Reset the override at the start of each turn — a fresh turn empties the
  // reasoning buffer, so a stale override does not carry across turns.
  useEffect(() => {
    if (!hasReasoning) setUserOpen(null);
  }, [hasReasoning]);

  if (!hasReasoning && tools.length === 0) return null;

  // Auto behavior: open ONLY while reasoning actively streams (live turn, answer
  // not started yet); collapse once the answer begins AND once the turn completes
  // (`live` false). A completed reasoning block ("추론 완료") therefore defaults
  // CLOSED — the store's turn-end archive resets `answerStarted` to false while
  // keeping the reasoning text, so without the `live` gate the finished block
  // would spuriously re-open. A user toggle (`userOpen`) overrides it so the
  // collapsed block can still be MANUALLY re-expanded afterwards. `open` is fully
  // controlled so the collapsed content leaves the DOM (no leaked text).
  const autoOpen = live && hasReasoning && !answerStarted;
  const reasoningOpen = userOpen !== null ? userOpen : autoOpen;
  const reasoningStreaming = live && !answerStarted;

  return (
    <div
      // No horizontal padding of its own — rendered INLINE inside the
      // conversation scroll content (which already pads, EUD-069).
      className="flex w-full max-w-[95%] flex-col gap-2"
      aria-label="에이전트 활동"
    >
      {hasReasoning && (
        <Reasoning
          isStreaming={reasoningStreaming}
          open={reasoningOpen}
          onOpenChange={setUserOpen}
        >
          <ReasoningTrigger />
          <ReasoningContent>{reasoning}</ReasoningContent>
        </Reasoning>
      )}

      {tools.length > 0 && (
        <div className="flex flex-col gap-1 my-3">
          <span className="flex items-center gap-1.5 text-xs text-muted-foreground">
            <WrenchIcon aria-hidden className="size-3.5 shrink-0" />
            도구 호출 {tools.length}건
          </span>
          <ToolList tools={tools} />
        </div>
      )}
    </div>
  );
}

/**
 * The expandable Tool card rows (shared, EUD-069): rendered live by
 * {@link AgentStream} and again by ConversationLog for ARCHIVED tool entries
 * (`LogEntry.tools`), so past turns' tool activity stays inspectable in the
 * conversation history.
 */
export function ToolList({ tools }: { tools: AgentTool[] }) {
  return (
    <>
      {tools.map((tool) => (
        <ToolRow key={tool.id} tool={tool} />
      ))}
    </>
  );
}

function ToolRow({ tool }: { tool: AgentTool }) {
  // File reads/writes render as code; exact file edits render as an
  // ordered colored diff instead of raw JSON.
  const fileView = parseFileTool(tool);
  // A rendered map (`map_draft_render` and friends) is shown as the image,
  // not its base64. The envelope can be megabytes, so parse it once per row.
  const imageView = useMemo(() => parseImageTool(tool), [tool.detail]);
  return (
    <Tool data-testid={`tool-${tool.id}`}>
      <ToolHeader title={tool.name} state={tool.state} />
      {fileView ? (
        <ToolContent>
          <div className="p-3">
            <FileToolBlock view={fileView} />
          </div>
        </ToolContent>
      ) : (
        (tool.args || tool.detail) && (
          <ToolContent>
            <div className="flex flex-col gap-2 p-3 text-xs text-muted-foreground">
              {tool.args && (
                <div>
                  <div className="mb-1 font-medium">요청</div>
                  <pre className="overflow-x-auto whitespace-pre-wrap break-words rounded bg-muted/40 p-2">
                    {tool.args}
                  </pre>
                </div>
              )}
              {imageView ? (
                <div>
                  <div className="mb-1 font-medium">결과</div>
                  <ImageToolBlock name={tool.name} view={imageView} />
                </div>
              ) : (
                tool.detail && (
                  <div>
                    <div className="mb-1 font-medium">결과</div>
                    <pre className="overflow-x-auto whitespace-pre-wrap break-words rounded bg-muted/40 p-2">
                      {tool.detail}
                    </pre>
                  </div>
                )
              )}
            </div>
          </ToolContent>
        )
      )}
    </Tool>
  );
}

/**
 * A rendered map result: the picture itself (natural size, scaled down to the
 * row when wider), its dimensions, and — for a result replayed from a run
 * receipt that kept only the PNG's digest — a placeholder instead.
 */
function ImageToolBlock({
  name,
  view,
}: {
  name: string;
  view: ImageToolView;
}) {
  const size =
    view.width !== null && view.height !== null
      ? `${view.width} × ${view.height}`
      : null;
  return (
    <div data-testid="image-tool-block" className="flex flex-col gap-1">
      {view.src ? (
        <div className="rounded bg-muted/40 p-2">
          <img
            src={view.src}
            alt={`${name} 렌더링 결과${size ? ` (${size})` : ""}`}
            width={view.width ?? undefined}
            height={view.height ?? undefined}
            className="block h-auto max-w-full rounded"
            style={{ imageRendering: "pixelated" }}
          />
        </div>
      ) : (
        <p className="flex items-center gap-1.5 rounded bg-muted/40 p-2">
          <ImageIcon aria-hidden className="size-3.5 shrink-0" />
          렌더링된 이미지는 기록에 남지 않았습니다.
        </p>
      )}
      <div className="flex flex-wrap gap-x-3">
        {size && <span>{size}</span>}
        <span>{view.mimeType}</span>
        {view.dataBytes !== null && (
          <span>{view.dataBytes.toLocaleString()}바이트 생략</span>
        )}
      </div>
      {view.metadata && (
        <pre className="overflow-x-auto whitespace-pre-wrap break-words rounded bg-muted/40 p-2">
          {view.metadata}
        </pre>
      )}
    </div>
  );
}

/**
 * A file-tool payload as an editor-style block: filename header plus either
 * syntax-highlighted content (`read_file` / `file_write`) or an ordered colored
 * replacement diff (`file_edit`).
 */
function FileToolBlock({ view }: { view: FileToolView }) {
  const isRead = view.mode === "read";
  const Icon = isRead ? FileIcon : FilePenLineIcon;
  const label = isRead ? "읽기" : view.mode === "write" ? "쓰기" : "수정";
  return (
    <div className="flex flex-col">
      <div
        data-testid="file-tool-header"
        className="flex items-center gap-1.5 rounded-t-md border border-b-0 border-border bg-muted/60 px-2 py-1.5 text-xs"
      >
        <Icon aria-hidden className="size-3.5 shrink-0 text-muted-foreground" />
        <span className="break-all font-medium">
          {view.path || "(이름 없는 파일)"}
        </span>
        <span
          className={cn(
            "ml-auto shrink-0 font-medium",
            isRead ? "text-sky-400" : "text-amber-400",
          )}
        >
          {label}
        </span>
      </div>
      {view.mode === "edit" ? (
        <FileEditDiff diff={view.diff} />
      ) : (
        <CodeBlock
          code={view.code}
          language={view.language}
          showLineNumbers
          className="rounded-t-none"
        >
          <CodeBlockCopyButton />
        </CodeBlock>
      )}
      {view.truncated && (
        <p className="mt-1 text-xs text-amber-400">
          내용이 길어 일부만 표시됩니다.
        </p>
      )}
    </div>
  );
}

/** Exact replacements rendered with the same +/- palette as changeset diffs. */
function FileEditDiff({ diff }: { diff: string }) {
  return (
    <pre className="overflow-x-auto rounded-b-md border border-border bg-muted/40 p-2 text-xs">
      {classifyDiff(diff).map((line, index) => (
        <div
          key={index}
          data-diff={line.kind}
          className={cn(
            "whitespace-pre-wrap break-words",
            line.kind === "add" && "text-emerald-400",
            line.kind === "del" && "text-destructive",
            line.kind === "hunk" && "text-sky-400",
            line.kind === "file" && "text-muted-foreground",
          )}
        >
          {line.text || " "}
        </div>
      ))}
    </pre>
  );
}
