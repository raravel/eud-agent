import { useEffect, useMemo, useRef, useState } from "react";
import {
  AlertCircle,
  ArrowLeft,
  ArrowRight,
  FileInput,
  FolderOpen,
  FolderX,
  History,
  Loader2,
  Plus,
  Search,
  X,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import type { RecentProject } from "@/lib/protocol";
import { cn, formatPathForDisplay } from "@/lib/utils";

export interface ProjectLauncherProps {
  recent: RecentProject[];
  loading?: boolean;
  busy?: boolean;
  error?: string | null;
  busyLabel?: string;
  pendingPath?: string | null;
  onRetryPending?: () => void;
  onDismissPending?: () => void;
  onOpenRecent(path: string): void | Promise<void>;
  onOpenPicker(directory?: boolean): void | Promise<void>;
  onCreate(): void | Promise<void>;
  onImport(): void | Promise<void>;
  onRemoveRecent(path: string): void | Promise<void>;
  onCancel?: () => void;
}

const openedDate = new Intl.DateTimeFormat("ko-KR", {
  year: "numeric",
  month: "short",
  day: "numeric",
});

function openedLabel(timestamp: number): string {
  const elapsed = Date.now() - timestamp;
  if (elapsed < 60_000) return "방금 열림";
  if (elapsed < 3_600_000) return `${Math.floor(elapsed / 60_000)}분 전`;
  if (elapsed < 86_400_000) return `${Math.floor(elapsed / 3_600_000)}시간 전`;
  return openedDate.format(timestamp);
}

const actionClassName = "group h-auto min-h-20 w-full justify-start gap-4 whitespace-normal rounded-xl px-4 py-4 text-left";

export function ProjectLauncher({
  recent,
  loading = false,
  busy = false,
  error = null,
  busyLabel = "프로젝트 작업을 처리하는 중…",
  pendingPath = null,
  onRetryPending,
  onDismissPending,
  onOpenRecent,
  onOpenPicker,
  onCreate,
  onImport,
  onRemoveRecent,
  onCancel,
}: ProjectLauncherProps) {
  const [query, setQuery] = useState("");
  const searchRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    searchRef.current?.focus();
  }, []);

  const visibleRecent = useMemo(() => {
    const normalized = formatPathForDisplay(query.trim()).replaceAll("\\", "/").toLocaleLowerCase();
    if (!normalized) return recent;
    return recent.filter(
      (project) =>
        project.name.toLocaleLowerCase().includes(normalized) ||
        formatPathForDisplay(project.path).replaceAll("\\", "/").toLocaleLowerCase().includes(normalized),
    );
  }, [query, recent]);

  const clearSearch = () => {
    setQuery("");
    searchRef.current?.focus();
  };

  return (
    <main className="h-dvh overflow-y-auto bg-background text-foreground">
      <div className="mx-auto flex min-h-full max-w-[1440px] flex-col px-6 py-6 sm:px-10 sm:py-8 md:h-full lg:px-14 lg:py-10">
        <header className="flex shrink-0 flex-wrap items-center justify-between gap-4">
          <div className="flex items-center gap-4">
            <img src="/eud-agent.png" alt="" className="size-12 shrink-0 rounded-xl" />
            <div>
              <p className="text-xs font-medium tracking-wide text-muted-foreground">EUD 에이전트</p>
              <h1 className="mt-1 text-2xl font-semibold tracking-tight">프로젝트 시작</h1>
            </div>
          </div>
          {onCancel && (
            <Button type="button" variant="ghost" className="min-h-10 gap-2 text-muted-foreground" disabled={busy} onClick={onCancel}>
              <ArrowLeft aria-hidden className="size-4" /> 현재 프로젝트로 돌아가기
            </Button>
          )}
        </header>

        <section aria-labelledby="e3s-welcome" className="mt-6 flex shrink-0 flex-col gap-5 rounded-2xl border border-primary/30 bg-primary/10 p-5 sm:flex-row sm:items-center sm:justify-between lg:px-6">
          <div className="flex min-w-0 items-start gap-4">
            <span className="hidden size-12 shrink-0 items-center justify-center rounded-xl bg-primary/15 text-primary lg:flex"><FileInput aria-hidden className="size-6" /></span>
            <div className="min-w-0">
              <h2 id="e3s-welcome" className="text-lg font-semibold tracking-tight">EUD Editor 3로 작업하셨나요?</h2>
              <p className="mt-2 break-keep text-sm leading-6">기존 .e3s 프로젝트를 가져와 편집과 빌드를 이어가세요.</p>
              <p className="mt-1 break-keep text-xs leading-5 text-muted-foreground">가져온 뒤에는 EUD Editor 3를 따로 실행할 필요가 없습니다.</p>
            </div>
          </div>
          <Button type="button" className="min-h-12 shrink-0 gap-3 px-5 font-semibold" disabled={busy} onClick={() => void onImport()}>
            <FileInput aria-hidden className="size-4" /> E3S 프로젝트 가져오기 <ArrowRight aria-hidden className="size-4" />
          </Button>
        </section>

        {(pendingPath || error || busy) && (
          <section
            aria-label={pendingPath ? "대기 중인 프로젝트 열기" : "프로젝트 작업 상태"}
            className={cn("mt-6 shrink-0 rounded-xl border p-4", error ? "border-destructive/30 bg-destructive/5" : "border-border bg-card")}
          >
            <div className="flex flex-wrap items-center gap-x-6 gap-y-3">
              <div className="min-w-0 flex-1 basis-64">
                {busy ? (
                  <p role="status" className="flex items-center gap-2 text-sm">
                    <Loader2 aria-hidden className="size-4 shrink-0 animate-spin text-primary motion-reduce:animate-none" /> {busyLabel}
                  </p>
                ) : pendingPath ? (
                  <p className="text-sm font-medium">대기 중인 프로젝트 열기</p>
                ) : null}
                {pendingPath && (
                  <p className="mt-1 truncate text-sm text-muted-foreground" title={formatPathForDisplay(pendingPath)}>{formatPathForDisplay(pendingPath)}</p>
                )}
                {error && (
                  <p role="alert" className={cn("flex items-start gap-2 break-keep text-sm leading-6 text-destructive", (busy || pendingPath) && "mt-2")}>
                    <AlertCircle aria-hidden className="mt-1 size-4 shrink-0" /> {error}
                  </p>
                )}
              </div>
              {pendingPath && (
                <div className="flex shrink-0 gap-2">
                  <Button type="button" variant="outline" disabled={busy} onClick={onRetryPending}>다시 열기</Button>
                  <Button type="button" variant="ghost" disabled={busy} onClick={onDismissPending}>열기 요청 취소</Button>
                </div>
              )}
            </div>
          </section>
        )}

        <div className="grid min-h-0 flex-1 gap-8 pt-8 md:grid-cols-[minmax(0,1fr)_19rem] lg:gap-10 lg:grid-cols-[minmax(0,1fr)_21rem] xl:grid-cols-[minmax(0,1fr)_23rem]">
          <section aria-labelledby="recent-projects" className="flex min-h-0 min-w-0 flex-col">
            <div className="shrink-0">
              <div className="flex items-baseline justify-between gap-3">
                <h2 id="recent-projects" className="text-lg font-semibold">최근 프로젝트</h2>
                <span className="text-xs text-muted-foreground">최근에 연 순서</span>
              </div>
              <label className="relative mt-4 block">
                <span className="sr-only">최근 프로젝트 검색</span>
                <Search aria-hidden className="pointer-events-none absolute left-3.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
                <input
                  ref={searchRef}
                  value={query}
                  onChange={(event) => setQuery(event.target.value)}
                  placeholder="이름 또는 경로로 검색"
                  className="h-11 w-full rounded-lg border border-input bg-card/60 pl-10 pr-11 text-sm outline-none transition focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring/30"
                />
                {query && (
                  <button type="button" aria-label="검색어 지우기" onClick={clearSearch} className="absolute right-1.5 top-1/2 flex size-8 -translate-y-1/2 items-center justify-center rounded-md text-muted-foreground outline-none hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring">
                    <X aria-hidden className="size-4" />
                  </button>
                )}
              </label>
            </div>

            <div className="-mr-2 mt-4 min-h-0 flex-1 pr-2 md:overflow-y-auto md:overscroll-contain" aria-busy={loading}>
              {loading ? (
                <div role="status" className="flex min-h-52 items-center justify-center gap-2 text-sm text-muted-foreground">
                  <Loader2 aria-hidden className="size-4 animate-spin motion-reduce:animate-none" /> 최근 프로젝트를 불러오는 중…
                </div>
              ) : visibleRecent.length > 0 ? (
                <ul className="space-y-1" aria-label="최근 프로젝트 목록">
                  {visibleRecent.map((project) => {
                    const name = project.name || "이름 없는 프로젝트";
                    const displayPath = formatPathForDisplay(project.path);
                    return (
                      <li key={project.path} className="group flex items-center rounded-xl border border-transparent transition-colors hover:border-border hover:bg-card focus-within:border-border focus-within:bg-card">
                        <button
                          type="button"
                          className="flex min-w-0 flex-1 items-center gap-3 rounded-xl px-3 py-4 text-left outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50"
                          aria-label={`${name} ${displayPath}${project.available ? "" : " 사용할 수 없음"}`}
                          title={displayPath}
                          disabled={busy}
                          onClick={() => void onOpenRecent(project.path)}
                        >
                          <span className="flex size-10 shrink-0 items-center justify-center rounded-lg bg-muted/60 text-muted-foreground">
                            {project.available ? <FolderOpen aria-hidden className="size-5" /> : <FolderX aria-hidden className="size-5 text-destructive" />}
                          </span>
                          <span className="min-w-0 flex-1">
                            <span className="block truncate text-sm font-medium">{name}</span>
                            <span className="mt-1 block truncate text-xs text-muted-foreground">{displayPath}</span>
                            {!project.available && <span className="mt-1 block text-xs text-destructive">사용할 수 없음 · 위치를 확인해 주세요</span>}
                          </span>
                          <span className="hidden shrink-0 pl-2 text-xs tabular-nums text-muted-foreground md:block">{openedLabel(project.lastOpenedAt)}</span>
                        </button>
                        <Button
                          type="button"
                          variant="ghost"
                          size="icon-sm"
                          className="mr-2 shrink-0 text-muted-foreground opacity-60 transition-opacity hover:text-foreground md:opacity-0 md:group-hover:opacity-100 md:group-focus-within:opacity-100 focus-visible:opacity-100"
                          aria-label={`${name} 최근 목록에서 제거`}
                          title="최근 목록에서만 제거"
                          disabled={busy}
                          onClick={() => void onRemoveRecent(project.path)}
                        >
                          <X aria-hidden className="size-4" />
                        </Button>
                      </li>
                    );
                  })}
                </ul>
              ) : (
                <div className="flex min-h-64 flex-col items-center justify-center rounded-xl border border-dashed border-border px-6 py-10 text-center md:min-h-full">
                  <span className="mb-4 flex size-12 items-center justify-center rounded-full bg-muted/60 text-muted-foreground">
                    {query.trim() ? <Search aria-hidden className="size-5" /> : <History aria-hidden className="size-5" />}
                  </span>
                  <p className="text-sm font-medium">{query.trim() ? "검색 결과가 없습니다." : "최근 프로젝트가 없습니다."}</p>
                  <p className="mt-2 max-w-64 break-keep text-sm leading-6 text-muted-foreground">
                    {query.trim() ? "다른 이름이나 경로로 검색해 보세요." : "E3S를 가져오거나 프로젝트를 열면 이곳에서 이어서 작업할 수 있습니다."}
                  </p>
                  {query.trim() && <Button type="button" variant="ghost" className="mt-3" onClick={clearSearch}>전체 프로젝트 보기</Button>}
                </div>
              )}
            </div>
            <p className="mt-4 shrink-0 border-t border-border pt-3 text-xs leading-5 text-muted-foreground">목록에서 제거해도 실제 프로젝트 파일은 삭제되지 않습니다.</p>
          </section>

          <aside aria-labelledby="launcher-actions" className="min-h-0 border-t border-border pt-6 md:overflow-y-auto md:overscroll-contain md:border-l md:border-t-0 md:pl-8 md:pt-0 lg:pl-10">
            <h2 id="launcher-actions" className="text-lg font-semibold">새로 만들기 및 열기</h2>
            <p className="mt-2 break-keep text-sm leading-6 text-muted-foreground">새 맵에서 시작하거나 가져온 프로젝트를 여세요.</p>
            <div className="mt-5 space-y-3">
              <Button type="button" variant="outline" className={actionClassName} disabled={busy} onClick={() => void onCreate()}>
                <Plus aria-hidden className="size-5 shrink-0 text-muted-foreground" />
                <span className="min-w-0 flex-1"><span className="block font-medium">새 프로젝트 만들기</span><span className="mt-1 block text-xs font-normal leading-5 text-muted-foreground">SCX·SCM 맵에서 시작</span></span>
                <ArrowRight aria-hidden className="size-4 shrink-0 text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100 group-focus-visible:opacity-100" />
              </Button>
              <Button type="button" variant="outline" className={actionClassName} disabled={busy} onClick={() => void onOpenPicker(false)}>
                <FileInput aria-hidden className="size-5 shrink-0 text-muted-foreground" />
                <span className="min-w-0 flex-1"><span className="block font-medium">프로젝트 파일 열기</span><span className="mt-1 block text-xs font-normal leading-5 text-muted-foreground">.eap · EUD Agent Project</span></span>
                <ArrowRight aria-hidden className="size-4 shrink-0 text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100 group-focus-visible:opacity-100" />
              </Button>
              <Button type="button" variant="outline" className={actionClassName} disabled={busy} onClick={() => void onOpenPicker(true)}>
                <FolderOpen aria-hidden className="size-5 shrink-0 text-muted-foreground" />
                <span className="min-w-0 flex-1"><span className="block font-medium">프로젝트 폴더 열기</span><span className="mt-1 block text-xs font-normal leading-5 text-muted-foreground">기존 작업 폴더 선택</span></span>
                <ArrowRight aria-hidden className="size-4 shrink-0 text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100 group-focus-visible:opacity-100" />
              </Button>
            </div>
            <p className="mt-6 break-keep rounded-lg bg-muted/30 px-4 py-3 text-xs leading-6 text-muted-foreground">
              <span className="font-medium text-foreground">파일로 바로 시작</span><br />
              프로젝트 폴더의 <span className="font-medium">.eap 파일</span>을 더블클릭하면 바로 열 수 있습니다.
            </p>
          </aside>
        </div>
      </div>
    </main>
  );
}
