/**
 * Center-column tab strip (Orca-style): a pinned "대화" tab plus one tab per
 * open workspace document. Tabs activate the center surface; document tabs can
 * be closed individually or all at once. The strip owns no document state —
 * App keeps open tabs and per-path contents so switching tabs never refetches.
 */
import { MessageSquare, X } from "lucide-react";

import { cn } from "@/lib/utils";

export type DocumentTabId = "chat" | string;

export interface DocumentTab {
  /** "chat" or the workspace-relative file path. */
  id: DocumentTabId;
  label: string;
}

export interface DocumentTabStripProps {
  tabs: DocumentTab[];
  activeTab: DocumentTabId;
  onSelect(id: DocumentTabId): void;
  onClose(id: DocumentTabId): void;
  onCloseAll(): void;
}

export function DocumentTabStrip({
  tabs,
  activeTab,
  onSelect,
  onClose,
  onCloseAll,
}: DocumentTabStripProps) {
  const documentCount = tabs.length - 1;
  return (
    <div
      role="tablist"
      aria-label="열린 문서"
      className="flex h-10 shrink-0 items-stretch gap-0.5 overflow-x-auto border-b border-border bg-card/40 px-1"
    >
      {tabs.map((tab) => {
        const active = tab.id === activeTab;
        return (
          <div
            key={tab.id}
            className={cn(
              "group flex min-w-0 max-w-56 items-stretch border-b-2 text-xs transition-colors",
              active
                ? "border-primary bg-background text-foreground"
                : "border-transparent text-muted-foreground hover:bg-muted hover:text-foreground",
            )}
          >
            <button
              type="button"
              role="tab"
              id={`document-tab-${tab.id}`}
              aria-selected={active}
              aria-controls={`document-panel-${tab.id}`}
              aria-label={tab.id === "chat" ? tab.label : `${tab.label} 문서 탭`}
              title={tab.id === "chat" ? tab.label : tab.id}
              className="flex min-w-0 flex-1 cursor-pointer items-center gap-1.5 px-2.5 outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
              onClick={() => onSelect(tab.id)}
            >
              {tab.id === "chat" ? (
                <MessageSquare className="size-3.5 shrink-0" aria-hidden="true" />
              ) : null}
              <span className="min-w-0 flex-1 truncate text-left">{tab.label}</span>
            </button>
            {tab.id !== "chat" && (
              <button
                type="button"
                aria-label={`${tab.label} 탭 닫기`}
                title="탭 닫기 (Ctrl+W)"
                onClick={() => onClose(tab.id)}
              >
                <X className="size-3" aria-hidden="true" />
              </button>
            )}
          </div>
        );
      })}
      {documentCount > 0 && (
        <button
          type="button"
          className="ml-auto flex shrink-0 items-center px-2 text-[11px] text-muted-foreground hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
          onClick={onCloseAll}
        >
          모두 닫기
        </button>
      )}
    </div>
  );
}
