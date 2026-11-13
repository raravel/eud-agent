import i18n from "@/lib/i18n";
import { useTranslation } from "react-i18next";
import { Suspense, lazy } from "react";
import { Save, X } from "lucide-react";

import { Button } from "@/components/ui/button";
import type { MemoryViewState } from "@/state/store";
import type { MemoryFile } from "@/lib/ipc";
import { cn } from "@/lib/utils";

const MonacoEditor = lazy(() => import("@/components/MonacoEditor"));

const tabs = (): ReadonlyArray<{ file: MemoryFile; label: string }> => [
  { file: "resources", label: i18n.t("workspace.memory.resources") },
  { file: "structure", label: i18n.t("workspace.memory.structure") },
  { file: "conventions", label: i18n.t("workspace.memory.conventions") },
  { file: "lessons", label: i18n.t("workspace.memory.lessons") },
];

export interface MemoryViewProps {
  memory: MemoryViewState;
  onClose(): void;
  embedded?: boolean;
  onTabSelected(file: MemoryFile): void;
  onEdited(file: MemoryFile, content: string): void;
  onSave(payload: { file: MemoryFile; content: string }): void;
}


export function MemoryView({
  memory,
  onClose,
  embedded = false,
  onTabSelected,
  onEdited,
  onSave,
}: MemoryViewProps) {
  const { t } = useTranslation();
  const activeFile = memory.activeTab;
  const activeValue = memory.drafts[activeFile] ?? memory.files[activeFile];
  const activeDirty = memory.dirty[activeFile];

  return (
    <section
      aria-label={t("workspace.memory.title")}
      tabIndex={-1}
      onKeyDown={(event) => {
        if (event.key === "Escape") onClose();
      }}
      className={cn(
        "flex flex-col gap-3 overflow-hidden bg-background p-3",
        embedded ? "h-full min-h-0 flex-1" : "max-h-[62vh] border-t border-border p-4",
      )}
    >
      <div className="flex items-center justify-between gap-3">
        <div className="min-w-0">
          <h2 className="text-sm font-semibold">{t("workspace.memory.title")}</h2>
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
            {t("workspace.memory.save")}
          </Button>
          {!embedded && (
            <Button
              type="button"
              size="icon"
              variant="ghost"
              aria-label={t("workspace.memory.close")}
              onClick={onClose}
            >
              <X className="size-4" aria-hidden="true" />
            </Button>
          )}
        </div>
      </div>

      <div
        role="tablist"
        aria-label={t("workspace.memory.files")}
        className="flex flex-wrap gap-1 border-b border-border"
      >
        {tabs().map(({ file, label }) => {
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

      <div className={cn("overflow-hidden rounded border border-border", embedded ? "min-h-[220px] flex-1" : "min-h-[288px]")}>
        <Suspense
          fallback={
            <div className={cn("flex min-h-[220px] items-center justify-center text-sm text-muted-foreground", embedded ? "h-full" : "h-[288px]")}>
              {t("workspace.memory.openingEditor")}
            </div>
          }
        >
          <MonacoEditor
            value={activeValue}
            onChange={(value) => onEdited(activeFile, value)}
            language="markdown"
            height={embedded ? "100%" : undefined}
          />
        </Suspense>
      </div>

    </section>
  );
}
