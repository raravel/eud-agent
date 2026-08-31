import { useMemo, useState } from "react";
import {
  Check,
  History,
  LoaderCircle,
  MessageSquareText,
  Pencil,
  Plus,
  Search,
  Trash2,
  X,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import type { SessionMeta } from "@/lib/protocol";
import { cn } from "@/lib/utils";
import { PROVIDER_LABELS } from "@/providers/providerCopy";

export interface MapSessionHistoryDialogProps {
  open: boolean;
  sessions: SessionMeta[];
  activeId: string;
  loading?: boolean;
  busy?: boolean;
  onOpenChange(open: boolean): void;
  onReload(): void;
  onCreate(): void;
  onLoad(id: string): void;
  onRename(id: string, name: string): void;
  onDelete(id: string): void;
}

function formatConversationTime(timestamp: number): string {
  if (!Number.isFinite(timestamp) || timestamp <= 0) return "대화 기록 없음";
  const date = new Date(timestamp);
  if (Number.isNaN(date.getTime())) return "대화 기록 없음";
  return new Intl.DateTimeFormat(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  }).format(date);
}

export function MapSessionHistoryDialog({
  open,
  sessions,
  activeId,
  loading = false,
  busy = false,
  onOpenChange,
  onReload,
  onCreate,
  onLoad,
  onRename,
  onDelete,
}: MapSessionHistoryDialogProps) {
  const [query, setQuery] = useState("");
  const filtered = useMemo(() => {
    const normalized = query.trim().toLocaleLowerCase();
    if (!normalized) return sessions;
    return sessions.filter((session) =>
      session.name.toLocaleLowerCase().includes(normalized),
    );
  }, [query, sessions]);

  const rename = (session: SessionMeta) => {
    const name = window.prompt("새 이름을 입력하세요.", session.name)?.trim();
    if (name && name !== session.name) onRename(session.id, name);
  };

  const remove = (session: SessionMeta) => {
    if (session.id === activeId) return;
    if (
      window.confirm(
        `'${session.name}' 맵 작업과 연결된 후보 히스토리를 삭제할까요?`,
      )
    ) {
      onDelete(session.id);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        showCloseButton={false}
        className="max-h-[calc(100vh-4rem)] gap-0 overflow-hidden p-0 sm:max-w-2xl"
      >
        <DialogHeader className="relative border-b border-border px-6 py-5 pr-16 text-left">
          <DialogTitle className="flex items-center gap-2">
            <History className="size-4 text-primary" aria-hidden="true" />
            맵 작업 히스토리
          </DialogTitle>
          <DialogDescription>
            현재 저장 맵에 연결된 이전 Map Agent 대화를 불러옵니다.
          </DialogDescription>
          <DialogClose asChild>
            <Button
              type="button"
              size="icon"
              variant="ghost"
              className="absolute right-4 top-4 size-11"
              aria-label="맵 작업 히스토리 닫기"
            >
              <X className="size-4" aria-hidden="true" />
            </Button>
          </DialogClose>
        </DialogHeader>

        <div className="flex min-h-0 flex-col gap-3 p-4">
          <div className="flex items-center gap-2">
            <label className="relative min-w-0 flex-1">
              <Search
                className="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground"
                aria-hidden="true"
              />
              <span className="sr-only">맵 작업 검색</span>
              <Input
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                className="h-11 pl-9"
                placeholder="작업 이름 검색"
              />
            </label>
            <Button
              type="button"
              className="h-11 shrink-0 gap-2"
              disabled={busy || loading}
              onClick={onCreate}
            >
              <Plus className="size-4" aria-hidden="true" />
              새 작업
            </Button>
          </div>

          <div className="flex items-center justify-between text-xs text-muted-foreground">
            <span>최근 대화 순</span>
            {loading ? (
              <span role="status" className="flex items-center gap-1.5">
                <LoaderCircle
                  className="size-3.5 animate-spin motion-reduce:animate-none"
                  aria-hidden="true"
                />
                불러오는 중
              </span>
            ) : (
              <span className="tabular-nums">{filtered.length}개</span>
            )}
          </div>

          <nav
            aria-label="맵 작업 히스토리"
            className="max-h-[min(28rem,55vh)] min-h-56 overflow-y-auto rounded-lg border border-border bg-card/35 p-1.5"
          >
            {!loading && filtered.length === 0 ? (
              <div className="flex min-h-52 flex-col items-center justify-center px-6 text-center">
                <MessageSquareText
                  className="size-8 text-muted-foreground/60"
                  aria-hidden="true"
                />
                <p className="mt-3 text-sm font-medium">표시할 맵 작업이 없습니다</p>
                <p className="mt-1 text-xs text-muted-foreground">
                  검색어를 지우거나 새 작업을 시작하세요.
                </p>
              </div>
            ) : (
              <ul className="grid gap-1">
                {filtered.map((session) => {
                  const active = session.id === activeId;
                  return (
                    <li
                      key={session.id}
                      className={cn(
                        "group flex min-w-0 items-center gap-1 rounded-md border border-transparent p-1",
                        active && "border-primary/20 bg-primary/10",
                      )}
                    >
                      <button
                        type="button"
                        className="flex min-h-14 min-w-0 flex-1 items-center gap-3 rounded-md px-2.5 text-left outline-none hover:bg-muted/60 focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-default disabled:hover:bg-transparent"
                        aria-current={active ? "page" : undefined}
                        aria-label={
                          active
                            ? `${session.name}, 현재 작업`
                            : `${session.name} 불러오기`
                        }
                        disabled={active || busy}
                        onClick={() => onLoad(session.id)}
                      >
                        <span
                          className={cn(
                            "flex size-8 shrink-0 items-center justify-center rounded-md border border-border bg-background/50",
                            active && "border-primary/30 text-primary",
                          )}
                        >
                          {active ? (
                            <Check className="size-4" aria-hidden="true" />
                          ) : (
                            <MessageSquareText className="size-4" aria-hidden="true" />
                          )}
                        </span>
                        <span className="min-w-0 flex-1">
                          <span className="block truncate text-sm font-medium text-foreground">
                            {session.name}
                          </span>
                          <span className="mt-0.5 block truncate text-[11px] text-muted-foreground">
                            {active ? "현재 작업 · " : ""}
                            {PROVIDER_LABELS[session.provider]} ·{" "}
                            {formatConversationTime(session.lastConversationAt)}
                          </span>
                        </span>
                      </button>
                      <Button
                        type="button"
                        size="icon"
                        variant="ghost"
                        className="size-11 shrink-0"
                        aria-label={`${session.name} 이름 변경`}
                        disabled={busy}
                        onClick={() => rename(session)}
                      >
                        <Pencil className="size-4" aria-hidden="true" />
                      </Button>
                      <Button
                        type="button"
                        size="icon"
                        variant="ghost"
                        className="size-11 shrink-0 text-muted-foreground hover:text-destructive"
                        aria-label={`${session.name} 삭제`}
                        disabled={active || busy}
                        title={active ? "현재 작업은 삭제할 수 없습니다." : undefined}
                        onClick={() => remove(session)}
                      >
                        <Trash2 className="size-4" aria-hidden="true" />
                      </Button>
                    </li>
                  );
                })}
              </ul>
            )}
          </nav>

          <div className="flex items-center justify-between gap-3 border-t border-border pt-3 text-[11px] text-muted-foreground">
            <span>후보 맵과 선택 영역은 작업별로 분리되어 복원됩니다.</span>
            <Button
              type="button"
              size="sm"
              variant="ghost"
              disabled={loading}
              onClick={onReload}
            >
              새로고침
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
