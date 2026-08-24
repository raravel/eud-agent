import { useEffect, useRef, useState } from "react";
import { Database, FileText, FolderTree, RefreshCw } from "lucide-react";

import { MemoryView } from "@/components/MemoryView";
import { WikiView } from "@/components/WikiView";
import { WorkspaceView } from "@/components/WorkspaceView";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import type { LedgerEntry, WorkspaceFileEntry, WorkspaceListResponse } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import type { MemoryViewState, WikiState } from "@/state/store";

export type ProjectPanelTab = "wiki" | "memory" | "workspace";

export interface ProjectSidebarProps {
  open: boolean;
  project: string;
  activeTab: ProjectPanelTab;
  wiki: WikiState;
  memory: MemoryViewState | null;
  workspace: WorkspaceListResponse | null;
  workspacePath: string | null;
  workspaceContent: string | null;
  workspaceLoading: boolean;
  workspaceError: string | null;
  onTabChange(tab: ProjectPanelTab): void;
  onClose(): void;
  onWikiSave(entries: Record<string, LedgerEntry>): void;
  onMemoryTabSelected(file: MemoryViewState["activeTab"]): void;
  onMemoryEdited(file: MemoryViewState["activeTab"], content: string): void;
  onMemorySave(payload: { file: MemoryViewState["activeTab"]; content: string }): void;
  onWorkspaceSelect(file: WorkspaceFileEntry): void;
  onWorkspaceSearch(query: string): Promise<string[]>;
  onWorkspaceRefresh(): void;
}

const WIDTH_KEY = "eud.project-sidebar.width";
const DEFAULT_WIDTH = 344;
const MIN_WIDTH = 300;
const MAX_WIDTH = 520;

function storedWidth(): number {
  if (typeof localStorage === "undefined") return DEFAULT_WIDTH;
  const stored = localStorage.getItem(WIDTH_KEY);
  if (stored === null) return DEFAULT_WIDTH;
  const value = Number(stored);
  return Number.isFinite(value)
    ? Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, value))
    : DEFAULT_WIDTH;
}

const TABS: ReadonlyArray<{ id: ProjectPanelTab; label: string; icon: typeof Database }> = [
  { id: "wiki", label: "DAT 위키", icon: Database },
  { id: "memory", label: "메모리", icon: FileText },
  { id: "workspace", label: "파일", icon: FolderTree },
];

export function ProjectSidebar({
  open,
  project,
  activeTab,
  wiki,
  memory,
  workspace,
  workspacePath,
  workspaceContent,
  workspaceLoading,
  workspaceError,
  onTabChange,
  onClose,
  onWikiSave,
  onMemoryTabSelected,
  onMemoryEdited,
  onMemorySave,
  onWorkspaceSelect,
  onWorkspaceSearch,
  onWorkspaceRefresh,
}: ProjectSidebarProps) {
  const [width, setWidth] = useState(storedWidth);
  const dragRef = useRef<{ startX: number; startWidth: number } | null>(null);

  useEffect(() => {
    try {
      localStorage.setItem(WIDTH_KEY, String(width));
    } catch {
      // Width persistence is optional; the current window keeps local state.
    }
  }, [width]);

  if (!open) return null;

  return (
    <aside
      aria-label="프로젝트 도구"
      style={{ width }}
      className="relative z-20 flex h-full shrink-0 flex-col border-l border-border bg-background shadow-sm max-[1140px]:fixed max-[1140px]:inset-y-0 max-[1140px]:right-0 max-[1140px]:shadow-2xl"
    >
      <header className="border-b border-border bg-card/40">
        <div className="flex min-h-12 items-center gap-2 px-3">
          <div className="min-w-0 flex-1">
            <h2 className="text-sm font-semibold">프로젝트</h2>
            <p className="truncate text-[11px] text-muted-foreground" title={project}>{project || "프로젝트 없음"}</p>
          </div>
        </div>
        <div role="tablist" aria-label="프로젝트 도구" className="grid grid-cols-3 px-2">
          {TABS.map(({ id, label, icon: Icon }) => (
            <button
              key={id}
              type="button"
              role="tab"
              aria-selected={activeTab === id}
              onClick={() => onTabChange(id)}
              className={cn(
                "flex min-h-10 items-center justify-center gap-1.5 border-b-2 px-2 text-xs outline-none transition-colors focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
                activeTab === id
                  ? "border-primary text-foreground"
                  : "border-transparent text-muted-foreground hover:text-foreground",
              )}
            >
              <Icon className="size-3.5" aria-hidden="true" />
              {label}
            </button>
          ))}
        </div>
      </header>

      <div className="flex min-h-0 flex-1 overflow-hidden">
        {activeTab === "wiki" && (
          <WikiView wiki={wiki} embedded onClose={onClose} onSave={onWikiSave} />
        )}
        {activeTab === "memory" && memory && (
          <MemoryView
            memory={memory}
            embedded
            onClose={onClose}
            onTabSelected={onMemoryTabSelected}
            onEdited={onMemoryEdited}
            onSave={onMemorySave}
          />
        )}
        {activeTab === "memory" && !memory && (
          <div className="flex flex-1 items-center justify-center gap-2 p-4 text-xs text-muted-foreground">
            <Spinner className="size-4" /> 메모리를 여는 중…
          </div>
        )}
        {activeTab === "workspace" && workspace && (
          <WorkspaceView
            key={workspace.workspaceId}
            workspace={workspace}
            selectedPath={workspacePath}
            selectedContent={workspaceContent}
            loading={workspaceLoading}
            error={workspaceError}
            embedded
            onSelect={onWorkspaceSelect}
            onSearch={onWorkspaceSearch}
            onRefresh={onWorkspaceRefresh}
            onClose={onClose}
          />
        )}
        {activeTab === "workspace" && !workspace && (
          <div className="flex flex-1 flex-col items-center justify-center gap-3 p-4 text-center text-xs text-muted-foreground">
            {workspaceLoading ? <Spinner className="size-4" /> : <FolderTree className="size-5" />}
            <span>{workspaceError ?? "워크스페이스를 여는 중…"}</span>
            {!workspaceLoading && (
              <Button type="button" size="sm" variant="outline" onClick={onWorkspaceRefresh}>
                <RefreshCw className="mr-1.5 size-3.5" aria-hidden="true" /> 다시 시도
              </Button>
            )}
          </div>
        )}
      </div>

      <div
        role="separator"
        aria-orientation="vertical"
        aria-label="프로젝트 사이드바 너비 조절"
        className="absolute inset-y-0 left-0 z-30 w-1.5 -translate-x-1/2 cursor-col-resize transition-colors hover:bg-primary/40 active:bg-primary/60"
        onPointerDown={(event) => {
          dragRef.current = { startX: event.clientX, startWidth: width };
          event.currentTarget.setPointerCapture(event.pointerId);
          event.preventDefault();
        }}
        onPointerMove={(event) => {
          const drag = dragRef.current;
          if (!drag) return;
          setWidth(Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, drag.startWidth - (event.clientX - drag.startX))));
        }}
        onPointerUp={(event) => {
          if (!dragRef.current) return;
          dragRef.current = null;
          event.currentTarget.releasePointerCapture(event.pointerId);
        }}
      />
    </aside>
  );
}
