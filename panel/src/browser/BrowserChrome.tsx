import { useCallback, useEffect, useState, type FormEvent } from "react";
import {
  ArrowLeft,
  ArrowRight,
  Bot,
  CircleAlert,
  Download,
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
        className="size-3 text-destructive"
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
        "size-3",
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
    <div className="flex h-dvh flex-col gap-1 overflow-hidden border-b border-border bg-background px-2 py-1 text-foreground">
      <div
        role="tablist"
        aria-label="브라우저 탭"
        className="flex h-9 shrink-0 items-center gap-1 overflow-x-auto overflow-y-hidden"
      >
        {state.tabs.length === 0 ? (
          <span className="px-2 text-xs text-muted-foreground">
            열린 탭이 없습니다.
          </span>
        ) : null}
        {state.tabs.map((tab) => {
          const label = tabLabel(tab.title, tab.url);
          const download = downloads.find(
            (candidate) => candidate.tabId === tab.tabId,
          );
          return (
            <div
              key={tab.tabId}
              className={cn(
                "flex max-w-64 shrink-0 items-center rounded-md border",
                tab.active
                  ? "border-primary/60 bg-accent"
                  : "border-transparent",
              )}
            >
              <Button
                role="tab"
                aria-selected={tab.active}
                variant="ghost"
                size="sm"
                className="min-w-0 justify-start gap-1.5 px-2"
                title={`${label} — ${tab.sessionTitle}`}
                onClick={() => void run(() => browserActivate(tab.tabId))}
              >
                {tab.awaitingConfirmation ? (
                  <ShieldAlert
                    aria-label="확인 대기"
                    className="size-3 text-destructive"
                  />
                ) : tab.agentBusy ? (
                  <Spinner aria-label="에이전트 조작 중" className="size-3" />
                ) : null}
                {download ? <TabDownloadMark download={download} /> : null}
                <span className="truncate">{label}</span>
                <Badge variant="secondary" className="max-w-24 justify-start">
                  <span className="truncate">{tab.sessionTitle}</span>
                </Badge>
              </Button>
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label={`${label} 탭 닫기`}
                onClick={() => void run(() => browserTabClose(tab.tabId))}
              >
                <X />
              </Button>
            </div>
          );
        })}
      </div>
      <form className="flex min-h-8 items-center gap-1" onSubmit={submit}>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
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
          aria-label="새로고침"
          disabled={!active}
          onClick={() => navigate("reload")}
        >
          <RotateCw />
        </Button>
        <Input
          aria-label="주소"
          className="h-8 flex-1"
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
              className="gap-1 px-2"
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
                      <p className="truncate font-medium" title={download.fileName}>
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
