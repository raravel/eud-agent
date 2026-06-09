import { Suspense, lazy, useMemo } from "react";
import { Save, X } from "lucide-react";

import { Button } from "@/components/ui/button";
import type { MemoryViewState } from "@/state/store";
import type { Episode, MemoryFile } from "@/lib/ipc";
import { cn } from "@/lib/utils";

const MonacoEditor = lazy(() => import("@/components/MonacoEditor"));

const TABS: ReadonlyArray<{ file: MemoryFile; label: string }> = [
  { file: "resources", label: "리소스" },
  { file: "structure", label: "구조" },
  { file: "conventions", label: "컨벤션" },
  { file: "lessons", label: "교훈" },
];

export interface MemoryViewProps {
  memory: MemoryViewState;
  onClose(): void;
  onTabSelected(file: MemoryFile): void;
  onEdited(file: MemoryFile, content: string): void;
  onSave(payload: { file: MemoryFile; content: string }): void;
}

function episodeTime(episode: Episode): number {
  if (!episode.ts) return 0;
  const ts = Date.parse(episode.ts);
  return Number.isFinite(ts) ? ts : 0;
}

function EpisodeLine({ episode }: { episode: Episode }) {
  const title = episode.instruction?.trim() || "기록된 작업";
  const meta = [episode.request_id, episode.ts].filter(Boolean).join(" · ");
  const tools = episode.tools?.filter(Boolean).join(", ");
  const files = episode.files?.filter(Boolean).join(", ");

  return (
    <li className="grid gap-1 rounded border border-border/60 px-3 py-2 text-xs">
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
        <span className="font-medium text-foreground">{title}</span>
        {meta && <span className="text-muted-foreground">{meta}</span>}
      </div>
      {episode.decision && (
        <div className="text-muted-foreground">{episode.decision}</div>
      )}
      {(tools || files) && (
        <div className="flex flex-wrap gap-x-3 gap-y-1 text-muted-foreground">
          {tools && <span>{tools}</span>}
          {files && <span>{files}</span>}
        </div>
      )}
    </li>
  );
}

export function MemoryView({
  memory,
  onClose,
  onTabSelected,
  onEdited,
  onSave,
}: MemoryViewProps) {
  const activeFile = memory.activeTab;
  const activeValue = memory.drafts[activeFile] ?? memory.files[activeFile];
  const activeDirty = memory.dirty[activeFile];
  const episodes = useMemo(
    () => [...memory.episodes].sort((a, b) => episodeTime(b) - episodeTime(a)),
    [memory.episodes],
  );

  return (
    <section
      aria-label="프로젝트 메모리"
      tabIndex={-1}
      onKeyDown={(event) => {
        if (event.key === "Escape") onClose();
      }}
      className="flex max-h-[62vh] flex-col gap-3 overflow-hidden border-t border-border bg-background p-4"
    >
      <div className="flex items-center justify-between gap-3">
        <div className="min-w-0">
          <h2 className="text-sm font-semibold">프로젝트 메모리</h2>
          {memory.project && (
            <p className="truncate text-xs text-muted-foreground">
              {memory.project}
            </p>
          )}
        </div>
        <div className="flex items-center gap-2">
          <Button
            type="button"
            size="sm"
            disabled={!activeDirty}
            onClick={() => onSave({ file: activeFile, content: activeValue })}
          >
            <Save className="mr-1 size-3.5" aria-hidden="true" />
            저장
          </Button>
          <Button
            type="button"
            size="icon"
            variant="ghost"
            aria-label="닫기"
            onClick={onClose}
          >
            <X className="size-4" aria-hidden="true" />
          </Button>
        </div>
      </div>

      <div
        role="tablist"
        aria-label="메모리 파일"
        className="flex flex-wrap gap-1 border-b border-border"
      >
        {TABS.map(({ file, label }) => {
          const selected = file === activeFile;
          return (
            <button
              key={file}
              type="button"
              role="tab"
              aria-selected={selected}
              className={cn(
                "border-b-2 px-3 py-2 text-sm transition-colors",
                selected
                  ? "border-primary text-foreground"
                  : "border-transparent text-muted-foreground hover:text-foreground",
              )}
              onClick={() => onTabSelected(file)}
            >
              {label}
            </button>
          );
        })}
      </div>

      <div className="min-h-[288px] overflow-hidden rounded border border-border">
        <Suspense
          fallback={
            <div className="flex h-[288px] items-center justify-center text-sm text-muted-foreground">
              편집기를 여는 중…
            </div>
          }
        >
          <MonacoEditor
            value={activeValue}
            onChange={(value) => onEdited(activeFile, value)}
            language="markdown"
          />
        </Suspense>
      </div>

      <div className="min-h-0 overflow-y-auto">
        <div className="mb-2 text-xs font-medium text-muted-foreground">
          에피소드
        </div>
        {episodes.length === 0 ? (
          <p className="text-xs text-muted-foreground">기록된 에피소드가 없습니다.</p>
        ) : (
          <ul className="grid gap-2">
            {episodes.map((episode, index) => (
              <EpisodeLine
                key={`${episode.request_id ?? "episode"}-${episode.ts ?? index}`}
                episode={episode}
              />
            ))}
          </ul>
        )}
      </div>
    </section>
  );
}
