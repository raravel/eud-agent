/**
 * Quick Open (Ctrl+P): jump to a project file by name.
 *
 * Purely local and instant: the listed project tree is ranked with the same
 * `rankProjectFiles` the composer's `@` search uses (name prefix, then name
 * substring, then path substring), `.git/` excluded, top 50 shown. ↑/↓ move,
 * Enter opens the file as a document tab, Esc closes.
 */
import { useEffect, useMemo, useState } from "react";
import { File as FileIcon } from "lucide-react";

import { HighlightedText } from "@/components/HighlightedText";
import {
  Command,
  CommandEmpty,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import type { TextRange, WorkspaceFileEntry, WorkspaceListResponse } from "@/lib/ipc";
import { rankProjectFiles } from "@/lib/projectFiles";

export interface QuickOpenDialogProps {
  open: boolean;
  onOpenChange(open: boolean): void;
  workspace: WorkspaceListResponse;
  onOpenFile(file: WorkspaceFileEntry): void;
}

const QUICK_OPEN_LIMIT = 50;

function isGitPath(path: string): boolean {
  return path === ".git" || path.startsWith(".git/");
}

export function QuickOpenDialog({
  open,
  onOpenChange,
  workspace,
  onOpenFile,
}: QuickOpenDialogProps) {
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState("");
  const files = useMemo(
    () => workspace.files.filter((file) => !isGitPath(file.path)),
    [workspace.files],
  );
  const results = useMemo(
    () => rankProjectFiles(workspace.workspaceId, files, query, QUICK_OPEN_LIMIT),
    [files, query, workspace.workspaceId],
  );

  // Every open starts from an empty query.
  useEffect(() => {
    if (open) setQuery("");
  }, [open]);

  // The best match stays highlighted as the query narrows the list.
  useEffect(() => {
    setSelected(results[0]?.path ?? "");
  }, [results]);

  const needle = query.trim().toLowerCase();
  const openPath = (path: string) => {
    const file = files.find((candidate) => candidate.path === path);
    if (!file) return;
    onOpenChange(false);
    onOpenFile(file);
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className="top-[15%] translate-y-0 overflow-hidden p-0 sm:max-w-xl"
        showCloseButton={false}
      >
        <DialogHeader className="sr-only">
          <DialogTitle>파일로 이동</DialogTitle>
          <DialogDescription>
            파일 이름을 입력하고 Enter로 엽니다. Esc로 닫습니다.
          </DialogDescription>
        </DialogHeader>
        <Command
          shouldFilter={false}
          value={selected}
          onValueChange={setSelected}
          label="파일로 이동"
        >
          <CommandInput
            value={query}
            onValueChange={setQuery}
            placeholder="파일 이름으로 검색"
            aria-label="파일 이름으로 검색"
          />
          <CommandList className="max-h-[min(60vh,420px)]">
            <CommandEmpty>일치하는 파일이 없습니다.</CommandEmpty>
            {results.map((entry) => {
              const separator = entry.path.lastIndexOf("/");
              const name = entry.path.slice(separator + 1);
              const directory = separator === -1 ? "" : entry.path.slice(0, separator);
              const nameMatch = needle ? name.toLowerCase().indexOf(needle) : -1;
              const ranges: TextRange[] =
                nameMatch === -1 ? [] : [[nameMatch, nameMatch + needle.length]];
              return (
                <CommandItem
                  key={entry.path}
                  value={entry.path}
                  title={entry.path}
                  className="gap-2 py-1.5 text-xs"
                  onSelect={openPath}
                >
                  <FileIcon className="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
                  <HighlightedText text={name} ranges={ranges} className="shrink-0 truncate" />
                  <span className="min-w-0 flex-1 truncate text-[11px] text-muted-foreground">
                    {directory}
                  </span>
                </CommandItem>
              );
            })}
          </CommandList>
        </Command>
      </DialogContent>
    </Dialog>
  );
}
