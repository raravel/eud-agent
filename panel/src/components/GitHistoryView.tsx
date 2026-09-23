/**
 * Project history — what replaced request changeset review.
 *
 * A turn no longer stops to ask whether its changes may stay. It applies them,
 * settles, and the app commits the project at the turn boundary. So the panel's
 * review surface becomes a record: the recent commits newest-first, the files
 * each one touched, and their patches. The way back is 되돌리기, which records
 * an inverse commit (`git revert`) rather than erasing anything — the
 * confirmation says so, because "되돌리기" otherwise reads like an undo that
 * removes the entry.
 *
 * The component owns its own loading/selection state and receives the three
 * commands as async props ({@link GitHistoryViewProps.loadLog} /
 * `loadDetail` / `revert`), the same seam E3sImportDialog uses, so it is
 * testable without a Tauri runtime. `revision` is App's cue that the history
 * moved (a new project, or a `git` event) and must be re-read.
 *
 * Added and removed lines carry a +/- gutter as well as their color: the diff
 * has to be readable without relying on color alone. A file the core could not
 * diff (a map, an image, an oversized change) carries `omitted` Korean prose
 * instead of a patch, and that prose is shown verbatim.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import {
  ChevronsUpDownIcon,
  FileDiffIcon,
  HistoryIcon,
  RefreshCwIcon,
  Undo2Icon,
} from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Spinner } from "@/components/ui/spinner";
import { cn } from "@/lib/utils";
import { truncateForDisplay } from "@/lib/truncate";
import {
  diffGutter,
  formatCommitTime,
  parseCommitPatch,
  parseCommitTrace,
  shortSha,
} from "@/lib/gitHistory";
import type {
  CommitDetail,
  CommitFile,
  CommitRecord,
  CommitSummary,
} from "@/lib/ipc";

export interface GitHistoryViewProps {
  /** Bumped by the App when the history moved (project opened, turn committed). */
  revision: number;
  /** Whether the body is expanded. */
  open: boolean;
  /** Persist expansion in the owning layout. */
  onOpenChange(open: boolean): void;
  /** `git_log` — the project's recent commits, newest first. */
  loadLog(): Promise<CommitSummary[]>;
  /** `git_commit_detail` — one commit's files and patches. */
  loadDetail(sha: string): Promise<CommitDetail>;
  /** `git_revert` — record the inverse of one commit. */
  revert(sha: string): Promise<CommitRecord>;
}

function errorText(error: unknown): string {
  if (error instanceof Error) return error.message;
  const text = String(error);
  return text.trim() === "" ? "알 수 없는 오류가 발생했습니다." : text;
}

/** One file's unified diff, or the Korean reason the core omitted it. */
function FilePatch({ file }: { file: CommitFile }) {
  if (file.patch === undefined) {
    return (
      <p className="border-t border-border px-2 py-1.5 text-xs text-muted-foreground">
        {file.omitted ?? "내용 비교를 표시하지 않습니다."}
      </p>
    );
  }
  const { text, truncated } = truncateForDisplay(file.patch);
  const { note, lines } = parseCommitPatch(text);
  return (
    <div className="border-t border-border">
      {note && (
        <p className="bg-muted/30 px-2 py-1 text-[11px] text-muted-foreground">
          {note}
        </p>
      )}
      {lines.length > 0 && (
        <pre className="overflow-x-auto bg-muted/40 p-2 text-xs">
          {lines.map((line, index) => (
            <div
              key={index}
              data-diff={line.kind}
              className={cn(
                "flex gap-2",
                line.kind === "add" && "text-emerald-400",
                line.kind === "del" && "text-destructive",
                line.kind === "hunk" && "text-sky-400",
                line.kind === "file" && "text-muted-foreground",
              )}
            >
              {/* The gutter repeats what the color says, so the diff still
                  reads on a monochrome or color-blind viewing. */}
              <span
                aria-hidden
                className="w-3 shrink-0 select-none text-center font-semibold text-muted-foreground"
              >
                {diffGutter(line.kind)}
              </span>
              <span className="min-w-0 whitespace-pre-wrap break-words">
                {line.kind === "add" || line.kind === "del"
                  ? line.text.slice(1) || " "
                  : line.text || " "}
              </span>
            </div>
          ))}
        </pre>
      )}
      {truncated && (
        <p className="px-2 py-1 text-xs text-amber-400">
          표시가 1 MiB에서 잘렸습니다.
        </p>
      )}
    </div>
  );
}

/** The selected commit: its trace lines, then one card per changed file. */
function CommitDetailPanel({ detail }: { detail: CommitDetail }) {
  const trace = parseCommitTrace(detail.body);
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-col gap-1">
        <h3 className="text-sm font-semibold break-words">{detail.subject}</h3>
        <p className="text-[11px] text-muted-foreground">
          <span className="font-mono">{shortSha(detail.sha)}</span>
          {" · "}
          {formatCommitTime(detail.timestamp)}
          {trace.session && (
            <>
              {" · 세션 "}
              <span className="font-mono">{trace.session}</span>
            </>
          )}
          {trace.request && (
            <>
              {" · 요청 "}
              <span className="font-mono">{trace.request}</span>
            </>
          )}
        </p>
      </div>

      {detail.files.length === 0 ? (
        <p className="text-sm text-muted-foreground">변경된 파일이 없습니다.</p>
      ) : (
        detail.files.map((file) => (
          <div
            key={file.path}
            data-testid={`commit-file-${file.path}`}
            className="overflow-hidden rounded border border-border"
          >
            <div className="flex items-center gap-1.5 bg-muted/60 px-2 py-1.5 text-xs">
              <FileDiffIcon
                aria-hidden
                className="size-3.5 shrink-0 text-muted-foreground"
              />
              <span className="min-w-0 break-all font-medium">{file.path}</span>
              <span className="ml-auto shrink-0 font-mono tabular-nums">
                <span className="text-emerald-400">+{file.insertions}</span>
                {" / "}
                <span className="text-destructive">-{file.deletions}</span>
              </span>
            </div>
            <FilePatch file={file} />
          </div>
        ))
      )}
    </div>
  );
}

export function GitHistoryView({
  revision,
  open,
  onOpenChange,
  loadLog,
  loadDetail,
  revert,
}: GitHistoryViewProps) {
  const [commits, setCommits] = useState<CommitSummary[]>([]);
  const [logError, setLogError] = useState<string | null>(null);
  const [logLoading, setLogLoading] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);
  const [detail, setDetail] = useState<CommitDetail | null>(null);
  const [detailError, setDetailError] = useState<string | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [confirming, setConfirming] = useState<CommitSummary | null>(null);
  const [reverting, setReverting] = useState(false);
  const [revertError, setRevertError] = useState<string | null>(null);
  const [reloadToken, setReloadToken] = useState(0);
  // Every load races the next one; only the newest result may land.
  const logRunRef = useRef(0);
  const detailRunRef = useRef(0);

  useEffect(() => {
    const run = ++logRunRef.current;
    let cancelled = false;
    setLogLoading(true);
    void loadLog()
      .then((rows) => {
        if (cancelled || run !== logRunRef.current) return;
        setCommits(rows);
        setLogError(null);
        // Keep the open commit selected across a refresh; otherwise follow the
        // newest one so the panel is never a list with nothing shown.
        setSelected((current) =>
          current !== null && rows.some((row) => row.sha === current)
            ? current
            : (rows[0]?.sha ?? null),
        );
      })
      .catch((error: unknown) => {
        if (cancelled || run !== logRunRef.current) return;
        setCommits([]);
        setSelected(null);
        setLogError(errorText(error));
      })
      .finally(() => {
        if (cancelled || run !== logRunRef.current) return;
        setLogLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [loadLog, revision, reloadToken]);

  useEffect(() => {
    if (selected === null) {
      setDetail(null);
      setDetailError(null);
      return;
    }
    const run = ++detailRunRef.current;
    let cancelled = false;
    setDetailLoading(true);
    void loadDetail(selected)
      .then((value) => {
        if (cancelled || run !== detailRunRef.current) return;
        setDetail(value);
        setDetailError(null);
      })
      .catch((error: unknown) => {
        if (cancelled || run !== detailRunRef.current) return;
        setDetail(null);
        setDetailError(errorText(error));
      })
      .finally(() => {
        if (cancelled || run !== detailRunRef.current) return;
        setDetailLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [loadDetail, selected, reloadToken]);

  const refresh = useCallback(() => setReloadToken((token) => token + 1), []);

  const confirmRevert = useCallback(async () => {
    if (confirming === null || reverting) return;
    setReverting(true);
    setRevertError(null);
    try {
      await revert(confirming.sha);
      setConfirming(null);
      refresh();
    } catch (error) {
      // The backend refuses a revert on a dirty work tree and says why, in
      // Korean; that sentence is the whole recovery instruction.
      setRevertError(errorText(error));
    } finally {
      setReverting(false);
    }
  }, [confirming, refresh, revert, reverting]);

  return (
    <section
      aria-label="변경 기록"
      className="flex max-h-[45vh] min-h-0 shrink-0 flex-col overflow-hidden border-t border-border bg-background"
    >
      <header className="flex min-h-12 shrink-0 items-center gap-2 border-b border-border px-4 py-2">
        <HistoryIcon aria-hidden className="size-4 shrink-0 text-muted-foreground" />
        <h2 className="text-sm font-semibold">변경 기록</h2>
        <Badge variant="outline" className="shrink-0 text-[11px]">
          {commits.length}건
        </Badge>
        {logLoading && (
          <Spinner
            aria-label="변경 기록을 읽는 중"
            className="size-3.5 shrink-0 motion-reduce:animate-none"
          />
        )}
        <div className="ml-auto flex shrink-0 items-center gap-1">
          <Button
            type="button"
            size="icon"
            variant="ghost"
            className="size-8"
            disabled={logLoading}
            aria-label="변경 기록 새로고침"
            onClick={refresh}
          >
            <RefreshCwIcon className="size-4" />
          </Button>
          <Button
            type="button"
            size="icon"
            variant="ghost"
            className="size-8"
            aria-expanded={open}
            aria-label={open ? "변경 기록 접기" : "변경 기록 펼치기"}
            onClick={() => onOpenChange(!open)}
          >
            <ChevronsUpDownIcon className="size-4" />
          </Button>
        </div>
      </header>

      {open && (
        <div className="grid min-h-0 flex-1 grid-cols-1 md:grid-cols-[minmax(0,16rem)_minmax(0,1fr)]">
          <div className="min-h-0 overflow-y-auto border-b border-border md:border-b-0 md:border-r">
            {logError ? (
              <p role="alert" className="px-4 py-3 text-sm text-destructive">
                {logError}
              </p>
            ) : commits.length === 0 ? (
              <p className="px-4 py-3 text-sm text-muted-foreground">
                {logLoading
                  ? "변경 기록을 읽는 중…"
                  : "아직 기록된 변경이 없습니다."}
              </p>
            ) : (
              <ul className="flex flex-col">
                {commits.map((commit) => {
                  const active = commit.sha === selected;
                  return (
                    <li
                      key={commit.sha}
                      data-testid={`commit-${shortSha(commit.sha)}`}
                      className={cn(
                        "flex items-center gap-1 border-b border-border/60 px-2 py-1.5",
                        active && "bg-muted/60",
                      )}
                    >
                      <button
                        type="button"
                        aria-current={active ? "true" : undefined}
                        className="min-w-0 flex-1 rounded px-1 py-0.5 text-left outline-none focus-visible:ring-2 focus-visible:ring-ring"
                        onClick={() => setSelected(commit.sha)}
                      >
                        <span className="block truncate text-sm">
                          {commit.subject}
                        </span>
                        <span className="mt-0.5 block text-[11px] text-muted-foreground">
                          <span className="font-mono">{shortSha(commit.sha)}</span>
                          {" · "}
                          {formatCommitTime(commit.timestamp)}
                        </span>
                      </button>
                      <Button
                        type="button"
                        size="xs"
                        variant="ghost"
                        className="shrink-0"
                        aria-label={`${commit.subject} 되돌리기`}
                        onClick={() => {
                          setRevertError(null);
                          setConfirming(commit);
                        }}
                      >
                        <Undo2Icon className="size-3.5" aria-hidden />
                        되돌리기
                      </Button>
                    </li>
                  );
                })}
              </ul>
            )}
          </div>

          <div className="min-h-0 overflow-y-auto px-4 py-3">
            {detailError ? (
              <p role="alert" className="text-sm text-destructive">
                {detailError}
              </p>
            ) : detailLoading && detail === null ? (
              <p className="flex items-center gap-2 text-sm text-muted-foreground">
                <Spinner
                  aria-label="커밋 내용을 읽는 중"
                  className="size-3.5 shrink-0 motion-reduce:animate-none"
                />
                커밋 내용을 읽는 중…
              </p>
            ) : detail === null ? (
              <p className="text-sm text-muted-foreground">
                왼쪽에서 커밋을 선택하면 바뀐 파일과 내용을 볼 수 있습니다.
              </p>
            ) : (
              <CommitDetailPanel detail={detail} />
            )}
          </div>
        </div>
      )}

      <Dialog
        open={confirming !== null}
        onOpenChange={(next) => {
          if (reverting) return;
          if (!next) {
            setConfirming(null);
            setRevertError(null);
          }
        }}
      >
        <DialogContent className="sm:max-w-lg">
          <DialogHeader>
            <DialogTitle>이 변경을 되돌릴까요?</DialogTitle>
            <DialogDescription>
              기록을 지우지 않고, 이 커밋을 취소하는 새 커밋을 남깁니다. 되돌린
              뒤에도 원래 커밋은 변경 기록에 그대로 남습니다.
            </DialogDescription>
          </DialogHeader>
          {confirming && (
            <p className="rounded border border-border bg-muted/40 px-3 py-2 text-sm break-words">
              <span className="font-mono text-xs text-muted-foreground">
                {shortSha(confirming.sha)}
              </span>
              {" · "}
              {confirming.subject}
            </p>
          )}
          {revertError && (
            <p role="alert" className="text-sm text-destructive">
              {revertError}
            </p>
          )}
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              disabled={reverting}
              onClick={() => {
                setConfirming(null);
                setRevertError(null);
              }}
            >
              취소
            </Button>
            <Button type="button" disabled={reverting} onClick={confirmRevert}>
              {reverting && (
                <Spinner
                  aria-label="되돌리는 중"
                  className="size-3.5 shrink-0 motion-reduce:animate-none"
                />
              )}
              {reverting ? "되돌리는 중…" : "되돌리기"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  );
}
