import { useCallback, useEffect, useState, type FormEvent } from "react";
import {
  ArrowLeft,
  ArrowRight,
  Bot,
  CircleAlert,
  Download,
  Globe,
  RotateCw,
  ShieldAlert,
  X,
} from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { Spinner } from "@/components/ui/spinner";
import { cn } from "@/lib/utils";
import { listenToCurrentWindow } from "@/lib/windowEvents";

import {
  BROWSER_STATE_EVENT,
  browserActivate,
  browserChromeExpand,
  browserState,
  browserTabClose,
  browserUserNavigate,
  type BrowserAction,
  type BrowserDownload,
  type BrowserState,
} from "./browserProtocol";

const EMPTY: BrowserState = { tabs: [], activeTabId: null, downloads: [] };

/** A byte count as the downloads list shows it. */
export function formatBytes(bytes: number): string {
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  if (bytes >= 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${bytes} B`;
}

function downloadStatus(download: BrowserDownload): string {
  switch (download.state) {
    case "inProgress":
      return "받는 중";
    case "completed":
      return formatBytes(download.bytes);
    default:
      return `실패: ${download.error ?? "알 수 없는 오류"}`;
  }
}

/** The small mark a tab carries for its newest download. */
function TabDownloadMark({ download }: { download: BrowserDownload }) {
  if (download.state === "failed") {
    return (
      <CircleAlert
        role="img"
        aria-label={`다운로드 실패: ${download.fileName}`}
        className="size-3.5 shrink-0 text-destructive"
      />
    );
  }
  const label =
    download.state === "inProgress"
      ? `다운로드 중: ${download.fileName}`
      : `다운로드됨: ${download.fileName}`;
  return (
    <Download
      role="img"
      aria-label={label}
      className={cn(
        "size-3.5 shrink-0",
        download.state === "inProgress"
          ? "text-muted-foreground motion-safe:animate-pulse"
          : "text-primary",
      )}
    />
  );
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** Session marks, one per session, as Chrome colours a tab group. */
const SESSION_COLORS = [
  "bg-chart-1",
  "bg-chart-2",
  "bg-chart-3",
  "bg-chart-4",
  "bg-chart-5",
];

/** The same session always gets the same colour. */
export function sessionColor(sessionId: string): string {
  let hash = 0;
  for (const char of sessionId) hash = (hash * 31 + char.charCodeAt(0)) | 0;
  return SESSION_COLORS[Math.abs(hash) % SESSION_COLORS.length];
}

/** The one mark in a tab's icon slot: confirmation, agent, download, page. */
function TabStatus({
  tab,
  download,
}: {
  tab: BrowserState["tabs"][number];
  download: BrowserDownload | undefined;
}) {
  if (tab.awaitingConfirmation) {
    return (
      <ShieldAlert
        role="img"
        aria-label="확인 대기"
        className="size-3.5 shrink-0 text-destructive"
      />
    );
  }
  if (tab.agentBusy) {
    return (
      <Spinner aria-label="에이전트 조작 중" className="size-3.5 shrink-0" />
    );
  }
  if (download) return <TabDownloadMark download={download} />;
  return <Globe aria-hidden className="size-3.5 shrink-0 opacity-70" />;
}

function tabLabel(title: string, url: string): string {
  if (title.trim()) return title;
  if (!url || url === "about:blank") return "새 탭";
  return url;
}

/**
 * The browser window's top strip: one button per tab (page title and the
 * owning session's name, a spinner while an agent call runs on it), and the
 * active tab's address bar. The page itself is a separate webview below.
 */
export default function BrowserChrome() {
  const [state, setState] = useState<BrowserState>(EMPTY);
  const [address, setAddress] = useState("");
  const [editing, setEditing] = useState(false);
  const [error, setError] = useState("");
  const [downloadsOpen, setDownloadsOpen] = useState(false);
  const downloads = state.downloads ?? [];

  const active =
    state.tabs.find((tab) => tab.tabId === state.activeTabId) ?? null;

  useEffect(() => {
    let disposed = false;
    const unlisten = listenToCurrentWindow<BrowserState>(
      BROWSER_STATE_EVENT,
      (event) => {
        if (!disposed) setState(event.payload);
      },
    );
    browserState()
      .then((next) => {
        if (!disposed) setState(next);
      })
      .catch((reason: unknown) => {
        if (!disposed) setError(errorText(reason));
      });
    return () => {
      disposed = true;
      void unlisten.then((stop) => stop());
    };
  }, []);

  useEffect(() => {
    if (!editing) setAddress(active?.url ?? "");
  }, [active?.tabId, active?.url, editing]);

  const run = useCallback(async (operation: () => Promise<void>) => {
    setError("");
    try {
      await operation();
    } catch (reason) {
      setError(errorText(reason));
    }
  }, []);

  const navigate = (action: BrowserAction) => {
    if (active) void run(() => browserUserNavigate(active.tabId, { action }));
  };

  const toggleDownloads = (open: boolean) => {
    setDownloadsOpen(open);
    void run(() => browserChromeExpand(open));
  };

  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (!active || !address.trim()) return;
    setEditing(false);
    void run(() => browserUserNavigate(active.tabId, { url: address.trim() }));
  };

  return (
    <div className="flex h-dvh flex-col overflow-hidden bg-background text-foreground">
      <div
        role="tablist"
        aria-label="브라우저 탭"
        className="flex h-9 shrink-0 items-end overflow-x-auto overflow-y-hidden px-2 pt-1 [scrollbar-width:none]"
      >
        {state.tabs.length === 0 ? (
          <span className="self-center px-2 text-xs text-muted-foreground">
            열린 탭이 없습니다.
          </span>
        ) : null}
        {state.tabs.map((tab, index) => {
          const label = tabLabel(tab.title, tab.url);
          const download = downloads.find(
            (candidate) => candidate.tabId === tab.tabId,
          );
          const next = state.tabs[index + 1];
          return (
            <div
              key={tab.tabId}
              className={cn(
                "group relative flex h-8 min-w-[100px] max-w-60 flex-1 basis-60 items-center rounded-t-lg pr-1",
                tab.active
                  ? "bg-card text-foreground"
                  : "text-muted-foreground hover:bg-muted/60 hover:text-foreground",
              )}
            >
              <span
                aria-hidden
                className={cn(
                  "absolute inset-x-3 top-0 h-0.5 rounded-b-full",
                  sessionColor(tab.sessionId),
                )}
              />
              <Button
                role="tab"
                aria-selected={tab.active}
                variant="ghost"
                size="sm"
                className="h-full min-w-0 flex-1 justify-start gap-2 rounded-t-lg rounded-b-none px-2.5 text-xs font-normal hover:bg-transparent"
                title={`${label}
세션: ${tab.sessionTitle}`}
                onClick={() => void run(() => browserActivate(tab.tabId))}
              >
                <TabStatus tab={tab} download={download} />
                <span className="min-w-0 flex-1 truncate text-left">
                  {label}
                </span>
                <span className="sr-only">, 세션 {tab.sessionTitle}</span>
              </Button>
              <Button
                variant="ghost"
                size="icon-xs"
                className="shrink-0 rounded-full"
                aria-label={`${label} 탭 닫기`}
                onClick={() => void run(() => browserTabClose(tab.tabId))}
              >
                <X />
              </Button>
              {!tab.active && !next?.active && next ? (
                <span
                  aria-hidden
                  className="absolute right-0 h-4 w-px bg-border group-hover:opacity-0"
                />
              ) : null}
            </div>
          );
        })}
      </div>
      <form
        className="flex h-10 shrink-0 items-center gap-1 border-b border-border bg-card px-2"
        onSubmit={submit}
      >
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          className="rounded-full"
          aria-label="뒤로"
          disabled={!active}
          onClick={() => navigate("back")}
        >
          <ArrowLeft />
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          className="rounded-full"
          aria-label="앞으로"
          disabled={!active}
          onClick={() => navigate("forward")}
        >
          <ArrowRight />
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          className="rounded-full"
          aria-label="새로고침"
          disabled={!active}
          onClick={() => navigate("reload")}
        >
          <RotateCw />
        </Button>
        {active ? (
          <span
            className="flex max-w-40 shrink-0 items-center gap-1.5 px-1.5 text-xs text-muted-foreground"
            title={`이 탭을 쓰는 대화: ${active.sessionTitle}`}
          >
            <span
              aria-hidden
              className={cn(
                "size-2 shrink-0 rounded-full",
                sessionColor(active.sessionId),
              )}
            />
            <span className="truncate">{active.sessionTitle}</span>
          </span>
        ) : null}
        <Input
          aria-label="주소"
          className="h-7 flex-1 rounded-full bg-background px-3.5 text-sm"
          disabled={!active}
          value={address}
          placeholder="https://"
          onFocus={() => setEditing(true)}
          onBlur={() => setEditing(false)}
          onChange={(event) => setAddress(event.target.value)}
        />
        <Popover open={downloadsOpen} onOpenChange={toggleDownloads}>
          <PopoverTrigger asChild>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="gap-1 rounded-full px-2"
              aria-label={`다운로드 목록 (${downloads.length}개)`}
            >
              <Download />
              {downloads.length > 0 ? (
                <span className="text-xs tabular-nums">{downloads.length}</span>
              ) : null}
            </Button>
          </PopoverTrigger>
          <PopoverContent align="end" className="w-80 p-2">
            <p className="px-1 pb-1 text-sm font-medium">다운로드</p>
            {downloads.length === 0 ? (
              <p className="px-1 py-2 text-xs text-muted-foreground">
                받은 파일이 없습니다.
              </p>
            ) : (
              <ul
                aria-label="받은 파일"
                className="flex max-h-80 flex-col gap-1 overflow-y-auto"
              >
                {downloads.map((download) => (
                  <li
                    key={download.downloadId}
                    className="flex items-center gap-2 rounded-md px-1 py-1 text-xs"
                  >
                    {download.state === "inProgress" ? (
                      <Spinner aria-label="받는 중" className="size-3" />
                    ) : download.state === "failed" ? (
                      <CircleAlert
                        aria-hidden
                        className="size-3 shrink-0 text-destructive"
                      />
                    ) : (
                      <Download
                        aria-hidden
                        className="size-3 shrink-0 text-primary"
                      />
                    )}
                    <div className="min-w-0 flex-1">
                      <p
                        className="truncate font-medium"
                        title={download.fileName}
                      >
                        {download.fileName}
                      </p>
                      <p
                        className={cn(
                          "truncate text-muted-foreground",
                          download.state === "failed" && "text-destructive",
                        )}
                        title={downloadStatus(download)}
                      >
                        {downloadStatus(download)} · {download.sessionTitle}
                      </p>
                    </div>
                  </li>
                ))}
              </ul>
            )}
          </PopoverContent>
        </Popover>
        {active?.awaitingConfirmation ? (
          <Badge
            variant="destructive"
            className="gap-1"
            role="status"
            title="에이전트 대화의 확인 카드에서 허용 또는 거부를 고르세요."
          >
            <ShieldAlert />
            확인 대기
          </Badge>
        ) : null}
        {active?.agentBusy ? (
          <Badge variant="outline" className="gap-1" role="status">
            <Bot />
            에이전트 조작 중
          </Badge>
        ) : null}
        {error ? (
          <span
            role="alert"
            className="max-w-80 truncate text-xs text-destructive"
            title={error}
          >
            {error}
          </span>
        ) : null}
      </form>
    </div>
  );
}
