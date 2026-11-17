/**
 * The paired phone's screens: an open or recent project of the PC, then one
 * of its conversations or Map Agent sessions, then that conversation or Map
 * session itself.
 */
import { useTranslation } from "react-i18next";
import { intlLocale } from "@/lib/i18n";
import { useCallback, useEffect, useState } from "react";
import {
  ChevronLeft,
  FolderOpen,
  LoaderCircle,
  Map as MapIcon,
  MessageSquare,
  MoreVertical,
  Plus,
  RefreshCw,
  Unplug,
  X,
} from "lucide-react";
import { Toaster } from "sonner";

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
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import type { SessionMeta } from "@/lib/ipc";
import {
  DETACHED_EVENT,
  RemoteCommandError,
  remoteTransport,
  type RemoteProject,
  type RemoteProjectEntry,
  type RemoteStatus,
} from "@/lib/remoteTransport";
import { invoke, listen } from "@/lib/transport";
import { mapSessionList } from "@/map/mapProtocol";
import { RemoteChat } from "./RemoteChat";
import { RemoteMap } from "./RemoteMap";
import { mapSessionRows, type MapSessionRow } from "./remoteMapModel";
import { canCloseProject, canForceClose, openEntry, projectBadge } from "./projectList";

type SessionTab = "eps" | "map";

type Screen =
  | { kind: "projects" }
  | { kind: "sessions"; project: RemoteProject; tab: SessionTab }
  | { kind: "chat"; project: RemoteProject; sessionId: string | null; key: number }
  | { kind: "map"; project: RemoteProject; sessionId: string | null; key: number };

function messageOf(error: unknown): string {
  return typeof error === "string" ? error : error instanceof Error ? error.message : String(error);
}

function formatWhen(epochMs: number): string {
  if (!Number.isFinite(epochMs) || epochMs <= 0) return "";
  const date = new Date(epochMs);
  const now = new Date();
  return date.toDateString() === now.toDateString()
    ? date.toLocaleTimeString(intlLocale(), { hour: "2-digit", minute: "2-digit" })
    : date.toLocaleDateString(intlLocale(), { month: "long", day: "numeric" });
}

function ScreenHeader({
  title,
  subtitle,
  onBack,
  backLabel,
  children,
}: {
  title: string;
  subtitle?: string;
  onBack?: () => void;
  backLabel?: string;
  children?: React.ReactNode;
}) {
  return (
    <header className="flex min-h-14 shrink-0 items-center gap-1 border-b border-border pl-2 pr-4">
      {onBack ? (
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="size-11 shrink-0"
          aria-label={backLabel}
          onClick={onBack}
        >
          <ChevronLeft aria-hidden className="size-5" />
        </Button>
      ) : (
        <span className="w-2 shrink-0" />
      )}
      <div className="min-w-0 flex-1">
        <h1 className="truncate text-base font-semibold">{title}</h1>
        {subtitle && <p className="truncate text-xs text-muted-foreground">{subtitle}</p>}
      </div>
      {children}
    </header>
  );
}

function ErrorText({ children }: { children: React.ReactNode }) {
  return (
    <p
      role="alert"
      className="rounded-lg border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm text-destructive"
    >
      {children}
    </p>
  );
}

const STATUS_KEYS: Partial<Record<RemoteStatus, string>> = {
  connecting: "remote.workspace.connecting",
  closed: "remote.workspace.closed",
};

function ConnectionBanner({ status }: { status: RemoteStatus }) {
  const { t } = useTranslation();
  const textKey = STATUS_KEYS[status];
  const text = textKey ? t(textKey) : undefined;
  if (!text) return null;
  return (
    <p
      role="status"
      className="flex shrink-0 items-center gap-2 border-b border-border bg-muted px-4 py-2 text-xs text-muted-foreground"
    >
      <LoaderCircle aria-hidden className="size-3.5 animate-spin motion-reduce:animate-none" />
      {text}
    </p>
  );
}

function ProjectList({
  onOpen,
  onDisconnect,
  notice,
}: {
  onOpen(entry: RemoteProjectEntry): Promise<void>;
  onDisconnect(): void;
  notice: string | null;
}) {
  const { t } = useTranslation();
  const [projects, setProjects] = useState<RemoteProjectEntry[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [opening, setOpening] = useState<string | null>(null);
  const [closing, setClosing] = useState<{
    entry: RemoteProjectEntry;
    refusal: string | null;
  } | null>(null);
  const [closeBusy, setCloseBusy] = useState(false);

  const load = useCallback(async () => {
    setError(null);
    try {
      setProjects(await invoke<RemoteProjectEntry[]>("remote_projects"));
    } catch (caught) {
      setError(messageOf(caught));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const close = async (force: boolean) => {
    const target = closing?.entry;
    if (!target?.projectId) return;
    setCloseBusy(true);
    try {
      await remoteTransport().closeProject(target.projectId, force);
      setClosing(null);
      await load();
    } catch (caught) {
      const refusal = messageOf(caught);
      const code = caught instanceof RemoteCommandError ? caught.code : undefined;
      if (!force && canForceClose(code)) {
        setClosing({ entry: target, refusal });
      } else {
        setClosing(null);
        setError(refusal);
      }
    } finally {
      setCloseBusy(false);
    }
  };

  return (
    <>
      <ScreenHeader title={t("remote.workspace.projects")} subtitle={t("remote.workspace.projectsSub")}>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="size-11"
          aria-label={t("remote.common.refresh")}
          onClick={() => void load()}
        >
          <RefreshCw aria-hidden className="size-4" />
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="size-11"
          aria-label={t("remote.workspace.disconnect")}
          onClick={onDisconnect}
        >
          <Unplug aria-hidden className="size-4" />
        </Button>
      </ScreenHeader>
      <main className="grid min-h-0 flex-1 content-start gap-3 overflow-y-auto px-4 py-4">
        {notice && <ErrorText>{notice}</ErrorText>}
        {error && <ErrorText>{error}</ErrorText>}
        {projects === null && !error && (
          <p role="status" className="flex items-center gap-2 text-sm text-muted-foreground">
            <LoaderCircle aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
            {t("remote.workspace.loadingProjects")}
          </p>
        )}
        {projects?.length === 0 && (
          <p className="text-sm leading-6 text-muted-foreground">
            {t("remote.workspace.noProjects")}
          </p>
        )}
        {projects && projects.length > 0 && (
          <ul className="grid gap-2">
            {projects.map((entry) => {
              const key = entry.projectId ?? entry.path;
              const badge = projectBadge(entry);
              return (
                <li key={key} className="flex items-stretch gap-1">
                  <Button
                    type="button"
                    variant="outline"
                    className="h-auto min-h-14 min-w-0 flex-1 justify-start gap-3 px-4 py-3 text-left"
                    disabled={opening !== null || !entry.available}
                    aria-label={t("remote.common.openAria", { name: entry.name })}
                    onClick={() => {
                      setOpening(key);
                      setError(null);
                      void onOpen(entry)
                        .catch((caught) => setError(messageOf(caught)))
                        .finally(() => setOpening(null));
                    }}
                  >
                    {opening === key ? (
                      <LoaderCircle
                        aria-hidden
                        className="size-5 shrink-0 animate-spin motion-reduce:animate-none"
                      />
                    ) : (
                      <FolderOpen
                        aria-hidden
                        className={
                          entry.open
                            ? "size-5 shrink-0 text-primary"
                            : "size-5 shrink-0 text-muted-foreground"
                        }
                      />
                    )}
                    <span className="grid min-w-0 flex-1 gap-0.5">
                      <span className="flex min-w-0 items-center gap-2">
                        <span className="truncate font-medium">{entry.name}</span>
                        {badge && (
                          <Badge variant={entry.desktopWindow ? "secondary" : "outline"}>
                            {badge}
                          </Badge>
                        )}
                      </span>
                      <span className="truncate text-xs font-normal text-muted-foreground">
                        {entry.available ? entry.path : t("remote.workspace.unavailable", { path: entry.path })}
                      </span>
                    </span>
                  </Button>
                  {canCloseProject(entry) && (
                    <Button
                      type="button"
                      variant="ghost"
                      className="h-auto min-h-14 shrink-0 px-3"
                      aria-label={t("remote.workspace.closeAria", { name: entry.name })}
                      disabled={opening !== null}
                      onClick={() => setClosing({ entry, refusal: null })}
                    >
                      <X aria-hidden className="size-4" />
                      {t("remote.workspace.close")}
                    </Button>
                  )}
                </li>
              );
            })}
          </ul>
        )}
      </main>

      <Dialog open={closing !== null} onOpenChange={(open) => !open && !closeBusy && setClosing(null)}>
        <DialogContent className="w-[calc(100vw-2rem)] max-w-sm">
          <DialogHeader>
            <DialogTitle>{t("remote.workspace.closeTitle")}</DialogTitle>
            <DialogDescription>
              {closing?.refusal
                ? t("remote.workspace.closeForceDesc", { refusal: closing.refusal })
                : t("remote.workspace.closeDesc", { name: closing?.entry.name ?? "" })}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              className="min-h-11"
              disabled={closeBusy}
              onClick={() => setClosing(null)}
            >
              {t("remote.common.cancel")}
            </Button>
            <Button
              type="button"
              variant="destructive"
              className="min-h-11"
              disabled={closeBusy}
              onClick={() => void close(closing?.refusal != null)}
            >
              {closing?.refusal ? t("remote.workspace.stopAndClose") : t("remote.workspace.close")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}

function MapSessionList({ onOpen }: { onOpen(sessionId: string | null): void }) {
  const { t } = useTranslation();
  const [rows, setRows] = useState<MapSessionRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setError(null);
    try {
      setRows(mapSessionRows(await mapSessionList()));
    } catch (caught) {
      setError(messageOf(caught));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <main className="grid min-h-0 flex-1 content-start gap-3 overflow-y-auto px-4 py-4">
      <div className="flex items-center gap-2">
        <p className="min-w-0 flex-1 text-xs text-muted-foreground">
          {t("remote.workspace.mapSub")}
        </p>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="size-11"
          aria-label={t("remote.workspace.refreshMap")}
          onClick={() => void load()}
        >
          <RefreshCw aria-hidden className="size-4" />
        </Button>
        <Button type="button" className="min-h-11 gap-1.5 px-3" onClick={() => onOpen(null)}>
          <Plus aria-hidden className="size-4" />
          {t("remote.workspace.newMap")}
        </Button>
      </div>
      {error && <ErrorText>{error}</ErrorText>}
      {rows === null && !error && (
        <p role="status" className="flex items-center gap-2 text-sm text-muted-foreground">
          <LoaderCircle aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
          {t("remote.workspace.loadingMap")}
        </p>
      )}
      {rows?.length === 0 && (
        <p className="text-sm text-muted-foreground">{t("remote.workspace.noMap")}</p>
      )}
      {rows && rows.length > 0 && (
        <ul className="grid gap-2">
          {rows.map((row) => (
            <li key={row.id}>
              <Button
                type="button"
                variant="outline"
                className="h-auto min-h-14 w-full min-w-0 justify-start gap-3 px-4 py-3 text-left"
                aria-label={t("remote.common.openAria", { name: `${row.name}${row.teamBadge ? ` (${row.teamBadge})` : ""}` })}
                onClick={() => onOpen(row.id)}
              >
                <MapIcon aria-hidden className="size-5 shrink-0 text-muted-foreground" />
                <span className="grid min-w-0 flex-1">
                  <span className="flex min-w-0 items-center gap-2">
                    <span className="truncate font-medium">{row.name}</span>
                    {row.teamBadge && <Badge variant="secondary">{row.teamBadge}</Badge>}
                  </span>
                  <span className="truncate text-xs font-normal text-muted-foreground">
                    {formatWhen(row.lastConversationAt)}
                  </span>
                </span>
              </Button>
            </li>
          ))}
        </ul>
      )}
    </main>
  );
}

function SessionList({
  project,
  tab,
  onTab,
  onBack,
  onOpen,
  onOpenMap,
}: {
  project: RemoteProject;
  tab: SessionTab;
  onTab(tab: SessionTab): void;
  onBack(): void;
  onOpen(sessionId: string | null): void;
  onOpenMap(sessionId: string | null): void;
}) {
  const { t } = useTranslation();
  const [sessions, setSessions] = useState<SessionMeta[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [renaming, setRenaming] = useState<SessionMeta | null>(null);
  const [newName, setNewName] = useState("");
  const [deleting, setDeleting] = useState<SessionMeta | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    setError(null);
    try {
      const rows = await invoke<SessionMeta[]>("session_list");
      setSessions(
        rows
          .filter((row) => row.workerParent === undefined && (row.kind ?? "eps") === "eps")
          .sort((left, right) => right.lastConversationAt - left.lastConversationAt),
      );
    } catch (caught) {
      setError(messageOf(caught));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const run = async (action: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await action();
      await load();
      return true;
    } catch (caught) {
      setError(messageOf(caught));
      return false;
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <ScreenHeader
        title={project.name}
        subtitle={tab === "eps" ? t("remote.common.conversation") : t("remote.common.mapAgent")}
        onBack={onBack}
        backLabel={t("remote.workspace.backToProjects")}
      >
        {tab === "eps" && (
          <Button
            type="button"
            className="min-h-11 gap-1.5 px-3"
            onClick={() => onOpen(null)}
          >
            <Plus aria-hidden className="size-4" />
            {t("remote.chat.newConversation")}
          </Button>
        )}
      </ScreenHeader>
      <Tabs value={tab} onValueChange={(value) => onTab(value as SessionTab)} className="shrink-0">
        <TabsList className="mx-4 mt-3 grid w-auto grid-cols-2">
          <TabsTrigger value="eps" className="min-h-9 gap-1.5">
            <MessageSquare aria-hidden className="size-4" />
            {t("remote.common.conversation")}
          </TabsTrigger>
          <TabsTrigger value="map" className="min-h-9 gap-1.5">
            <MapIcon aria-hidden className="size-4" />
            {t("remote.common.mapAgent")}
          </TabsTrigger>
        </TabsList>
      </Tabs>
      {tab === "map" ? (
        <MapSessionList onOpen={onOpenMap} />
      ) : (
        <main className="grid min-h-0 flex-1 content-start gap-3 overflow-y-auto px-4 py-4">
          {error && <ErrorText>{error}</ErrorText>}
          {sessions === null && !error && (
            <p role="status" className="flex items-center gap-2 text-sm text-muted-foreground">
              <LoaderCircle aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
              {t("remote.workspace.loadingConv")}
            </p>
          )}
          {sessions?.length === 0 && (
            <p className="text-sm text-muted-foreground">{t("remote.workspace.noConv")}</p>
          )}
          {sessions && sessions.length > 0 && (
            <ul className="grid gap-2">
              {sessions.map((session) => (
                <li key={session.id} className="flex items-stretch gap-1">
                  <Button
                    type="button"
                    variant="outline"
                    className="h-auto min-h-14 min-w-0 flex-1 justify-start gap-3 px-4 py-3 text-left"
                    onClick={() => onOpen(session.id)}
                  >
                    <MessageSquare aria-hidden className="size-5 shrink-0 text-muted-foreground" />
                    <span className="grid min-w-0 flex-1">
                      <span className="truncate font-medium">{session.name}</span>
                      <span className="truncate text-xs font-normal text-muted-foreground">
                        {formatWhen(session.lastConversationAt)}
                      </span>
                    </span>
                  </Button>
                  <DropdownMenu>
                    <DropdownMenuTrigger asChild>
                      <Button
                        type="button"
                        variant="ghost"
                        size="icon"
                        className="h-auto min-h-14 w-11 shrink-0"
                        aria-label={t("remote.workspace.menuAria", { name: session.name })}
                      >
                        <MoreVertical aria-hidden className="size-4" />
                      </Button>
                    </DropdownMenuTrigger>
                    <DropdownMenuContent align="end">
                      <DropdownMenuItem
                        onSelect={() => {
                          setNewName(session.name);
                          setRenaming(session);
                        }}
                      >
                        {t("remote.workspace.rename")}
                      </DropdownMenuItem>
                      <DropdownMenuItem variant="destructive" onSelect={() => setDeleting(session)}>
                        {t("remote.workspace.delete")}
                      </DropdownMenuItem>
                    </DropdownMenuContent>
                  </DropdownMenu>
                </li>
              ))}
            </ul>
          )}
        </main>
      )}

      <Dialog open={renaming !== null} onOpenChange={(open) => !open && setRenaming(null)}>
        <DialogContent className="w-[calc(100vw-2rem)] max-w-sm">
          <form
            className="grid gap-4"
            onSubmit={(event) => {
              event.preventDefault();
              const target = renaming;
              const name = newName.trim();
              if (!target || !name) return;
              void run(() => invoke("session_rename", { id: target.id, name })).then((ok) => {
                if (ok) setRenaming(null);
              });
            }}
          >
            <DialogHeader>
              <DialogTitle>{t("remote.workspace.renameTitle")}</DialogTitle>
              <DialogDescription>{t("remote.workspace.renameDesc")}</DialogDescription>
            </DialogHeader>
            <Input
              aria-label={t("remote.workspace.renameAria")}
              value={newName}
              maxLength={120}
              disabled={busy}
              onChange={(event) => setNewName(event.target.value)}
              className="h-11"
            />
            <DialogFooter>
              <Button type="button" variant="outline" className="min-h-11" onClick={() => setRenaming(null)}>
                {t("remote.common.cancel")}
              </Button>
              <Button type="submit" className="min-h-11" disabled={busy || newName.trim().length === 0}>
                {t("remote.workspace.save")}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>

      <Dialog open={deleting !== null} onOpenChange={(open) => !open && setDeleting(null)}>
        <DialogContent className="w-[calc(100vw-2rem)] max-w-sm">
          <DialogHeader>
            <DialogTitle>{t("remote.workspace.deleteTitle")}</DialogTitle>
            <DialogDescription>
              {t("remote.workspace.deleteDesc", { name: deleting?.name })}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button type="button" variant="outline" className="min-h-11" onClick={() => setDeleting(null)}>
              {t("remote.common.cancel")}
            </Button>
            <Button
              type="button"
              variant="destructive"
              className="min-h-11"
              disabled={busy}
              onClick={() => {
                const target = deleting;
                if (!target) return;
                void run(() => invoke("session_delete", { id: target.id })).then((ok) => {
                  if (ok) setDeleting(null);
                });
              }}
            >
              {t("remote.workspace.delete")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}

export function RemoteWorkspace({ onDisconnect }: { onDisconnect(): void }) {
  const { t } = useTranslation();
  const [screen, setScreen] = useState<Screen>({ kind: "projects" });
  const [status, setStatus] = useState<RemoteStatus>(() => remoteTransport().currentStatus());
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    const transport = remoteTransport();
    const off = transport.onStatus(setStatus);
    transport.start();
    setStatus(transport.currentStatus());
    return off;
  }, []);

  // The PC closed the project (or it could not be re-attached): back to the list.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void listen<{ error?: string }>(DETACHED_EVENT, ({ payload }) => {
      setNotice(payload.error ?? t("remote.workspace.projectLost"));
      setScreen({ kind: "projects" });
    }).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    if (status === "unauthorized") onDisconnect();
  }, [status, onDisconnect]);

  const openProject = useCallback(async (entry: RemoteProjectEntry) => {
    const transport = remoteTransport();
    const attached = await openEntry(
      {
        attach: (projectId) => transport.attach(projectId),
        openProject: (path) => transport.openProject(path),
      },
      entry,
    );
    setNotice(null);
    setScreen({ kind: "sessions", project: attached, tab: "eps" });
  }, []);

  const leaveProject = useCallback(() => {
    setScreen({ kind: "projects" });
    void remoteTransport()
      .detach()
      .catch(() => {});
  }, []);

  return (
    <div className="flex h-dvh min-w-0 flex-col overflow-hidden bg-background text-foreground">
      <Toaster position="top-center" richColors closeButton />
      <ConnectionBanner status={status} />
      {screen.kind === "projects" && (
        <ProjectList onOpen={openProject} onDisconnect={onDisconnect} notice={notice} />
      )}
      {screen.kind === "sessions" && (
        <SessionList
          project={screen.project}
          tab={screen.tab}
          onTab={(tab) => setScreen({ kind: "sessions", project: screen.project, tab })}
          onBack={leaveProject}
          onOpen={(sessionId) =>
            setScreen({ kind: "chat", project: screen.project, sessionId, key: Date.now() })
          }
          onOpenMap={(sessionId) =>
            setScreen({ kind: "map", project: screen.project, sessionId, key: Date.now() })
          }
        />
      )}
      {screen.kind === "chat" && (
        <div className="min-h-0 flex-1">
          <RemoteChat
            key={screen.key}
            sessionId={screen.sessionId}
            projectName={screen.project.name}
            onBack={() => setScreen({ kind: "sessions", project: screen.project, tab: "eps" })}
          />
        </div>
      )}
      {screen.kind === "map" && (
        <div className="min-h-0 flex-1">
          <RemoteMap
            key={screen.key}
            sessionId={screen.sessionId}
            projectName={screen.project.name}
            onBack={() => setScreen({ kind: "sessions", project: screen.project, tab: "map" })}
          />
        </div>
      )}
    </div>
  );
}
