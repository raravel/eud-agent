/**
 * Session restore UI (features/sessions.md ## Panel changes).
 *
 * A modal listing the saved sessions (`session_list`, most-recently-updated
 * first) with open / rename / delete actions. Conversations auto-save (the core
 * auto-creates a session on the first turn and the panel pushes its log after
 * each change) — there is no save button. The currently-active session is
 * highlighted.
 *
 * The list is the ONLY entry point to a saved conversation: opening calls
 * `session_open` (App hydrates the store from the returned record BEFORE the
 * live connect flow repopulates transient state). This component never re-opens
 * a review surface from the log — review surfaces gate on live
 * `state.changeset`/`state.plan`, and a reconnected changeset arrives as a live
 * `changeset` event after open.
 *
 * Korean user-facing strings throughout (project convention).
 */
import { useEffect, useState } from "react";
import { Pencil, Trash2, X } from "lucide-react";

import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import type { SessionMeta } from "@/lib/ipc";

export interface SessionListProps {
  /** Whether the overlay is visible. */
  open: boolean;
  /** Close the overlay. */
  onClose(): void;
  /** The active session id (highlighted in the list), or null if none. */
  currentId: string | null;
  /** Fetch the session list (resolves the `session_list` result). */
  onList(): Promise<SessionMeta[]>;
  /** Open a saved session by id (App hydrates from the returned record). */
  onOpen(id: string): void;
  /** Rename a session; the resolved promise reconciles the optimistic row. */
  onRename(id: string, name: string): void | Promise<void>;
  /** Delete a session; the resolved promise reconciles the optimistic row. */
  onDelete(id: string): void | Promise<void>;
}

/** Format a Unix-seconds timestamp as a short local date-time, defensively. */
function formatUpdated(updatedAt: number): string {
  if (!Number.isFinite(updatedAt) || updatedAt <= 0) return "";
  const d = new Date(updatedAt * 1000);
  if (Number.isNaN(d.getTime())) return "";
  return d.toLocaleString();
}

export function SessionList({
  open,
  onClose,
  currentId,
  onList,
  onOpen,
  onRename,
  onDelete,
}: SessionListProps) {
  const [sessions, setSessions] = useState<SessionMeta[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Refresh the list each time the overlay opens (and never poll while closed).
  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setLoading(true);
    setError(null);
    onList()
      .then((rows) => {
        if (!cancelled) setSessions(rows);
      })
      .catch((err) => {
        if (!cancelled) setError(String(err));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [open, onList]);

  if (!open) return null;

  const refresh = () => {
    onList()
      .then(setSessions)
      .catch((err) => setError(String(err)));
  };

  const handleRename = (session: SessionMeta) => {
    const name = window.prompt("새 이름을 입력하세요.", session.name);
    const trimmed = name?.trim();
    if (!trimmed || trimmed === session.name) return;
    // Optimistic; reconcile with the authoritative core list on settle so a
    // failed rename reverts instead of silently diverging.
    setSessions((prev) =>
      prev.map((s) => (s.id === session.id ? { ...s, name: trimmed } : s)),
    );
    void Promise.resolve(onRename(session.id, trimmed)).finally(refresh);
  };

  const handleDelete = (session: SessionMeta) => {
    if (!window.confirm(`'${session.name}' 대화를 삭제할까요?`)) return;
    setSessions((prev) => prev.filter((s) => s.id !== session.id));
    void Promise.resolve(onDelete(session.id)).finally(refresh);
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
      onClick={onClose}
    >
      <section
        role="dialog"
        aria-label="대화 목록"
        tabIndex={-1}
        onClick={(event) => event.stopPropagation()}
        onKeyDown={(event) => {
          if (event.key === "Escape") onClose();
        }}
        className="flex max-h-[80vh] w-full max-w-lg flex-col gap-3 overflow-hidden rounded-lg border border-border bg-background p-5 shadow-lg"
      >
        <div className="flex items-center justify-between gap-3">
          <h2 className="text-sm font-semibold">대화 목록</h2>
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

        {error && (
          <p className="text-xs text-destructive">목록을 불러오지 못했습니다: {error}</p>
        )}

        <div className="min-h-0 flex-1 overflow-y-auto">
          {loading ? (
            <p className="text-sm text-muted-foreground">불러오는 중…</p>
          ) : sessions.length === 0 ? (
            <p className="text-sm text-muted-foreground">저장된 대화가 없습니다.</p>
          ) : (
            <ul className="grid gap-2">
              {sessions.map((session) => {
                const updated = formatUpdated(session.updatedAt);
                const isCurrent = session.id === currentId;
                return (
                  <li
                    key={session.id}
                    className={cn(
                      "flex items-center gap-2 rounded border px-3 py-2",
                      isCurrent
                        ? "border-primary/60 bg-primary/5"
                        : "border-border/60",
                    )}
                  >
                    <button
                      type="button"
                      onClick={() => onOpen(session.id)}
                      className={cn(
                        "min-w-0 flex-1 text-left",
                        "rounded outline-none focus-visible:ring-2 focus-visible:ring-ring",
                      )}
                    >
                      <span className="flex items-center gap-1.5 truncate text-sm font-medium text-foreground">
                        <span className="truncate">{session.name}</span>
                        {isCurrent && (
                          <span className="shrink-0 rounded bg-primary/15 px-1 text-[10px] font-normal text-primary">
                            현재
                          </span>
                        )}
                      </span>
                      <span className="block truncate text-xs text-muted-foreground">
                        {[session.project, updated].filter(Boolean).join(" · ")}
                      </span>
                    </button>
                    <Button
                      type="button"
                      size="icon-sm"
                      variant="ghost"
                      aria-label="이름 변경"
                      onClick={() => handleRename(session)}
                    >
                      <Pencil className="size-3.5" aria-hidden="true" />
                    </Button>
                    <Button
                      type="button"
                      size="icon-sm"
                      variant="ghost"
                      aria-label="삭제"
                      onClick={() => handleDelete(session)}
                    >
                      <Trash2 className="size-3.5" aria-hidden="true" />
                    </Button>
                  </li>
                );
              })}
            </ul>
          )}
        </div>
      </section>
    </div>
  );
}
