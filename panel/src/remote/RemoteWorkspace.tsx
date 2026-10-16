/**
 * The paired phone's screens: a project open on the PC, then one of its
 * conversations, then the conversation itself.
 */
import { useCallback, useEffect, useState } from "react";
import {
  ChevronLeft,
  FolderOpen,
  LoaderCircle,
  MessageSquare,
  MoreVertical,
  Plus,
  RefreshCw,
  Unplug,
} from "lucide-react";
import { Toaster } from "sonner";

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
import type { SessionMeta } from "@/lib/ipc";
import {
  DETACHED_EVENT,
  remoteTransport,
  type RemoteProject,
  type RemoteStatus,
} from "@/lib/remoteTransport";
import { invoke, listen } from "@/lib/transport";
import { RemoteChat } from "./RemoteChat";

type Screen =
  | { kind: "projects" }
  | { kind: "sessions"; project: RemoteProject }
  | { kind: "chat"; project: RemoteProject; sessionId: string | null; key: number };

function messageOf(error: unknown): string {
  return typeof error === "string" ? error : error instanceof Error ? error.message : String(error);
}

function formatWhen(epochMs: number): string {
  if (!Number.isFinite(epochMs) || epochMs <= 0) return "";
  const date = new Date(epochMs);
  const now = new Date();
  return date.toDateString() === now.toDateString()
    ? date.toLocaleTimeString("ko-KR", { hour: "2-digit", minute: "2-digit" })
    : date.toLocaleDateString("ko-KR", { month: "long", day: "numeric" });
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

const STATUS_TEXT: Partial<Record<RemoteStatus, string>> = {
  connecting: "PC에 연결하는 중…",
  closed: "PC와 연결이 끊어졌습니다. 다시 연결하는 중…",
};

function ConnectionBanner({ status }: { status: RemoteStatus }) {
  const text = STATUS_TEXT[status];
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
  onOpen(project: RemoteProject): Promise<void>;
  onDisconnect(): void;
  notice: string | null;
}) {
  const [projects, setProjects] = useState<RemoteProject[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [opening, setOpening] = useState<string | null>(null);

  const load = useCallback(async () => {
    setError(null);
    try {
      setProjects(await invoke<RemoteProject[]>("remote_projects"));
    } catch (caught) {
      setError(messageOf(caught));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <>
      <ScreenHeader title="프로젝트" subtitle="PC에서 열려 있는 프로젝트">
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="size-11"
          aria-label="새로 고침"
          onClick={() => void load()}
        >
          <RefreshCw aria-hidden className="size-4" />
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="size-11"
          aria-label="연결 해제"
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
            프로젝트를 불러오는 중…
          </p>
        )}
        {projects?.length === 0 && (
          <p className="text-sm leading-6 text-muted-foreground">
            PC에서 열려 있는 프로젝트가 없습니다. PC에서 프로젝트를 연 뒤 새로 고침을 눌러 주세요.
          </p>
        )}
        {projects && projects.length > 0 && (
          <ul className="grid gap-2">
            {projects.map((project) => (
              <li key={project.projectId}>
                <Button
                  type="button"
                  variant="outline"
                  className="h-auto min-h-14 w-full justify-start gap-3 px-4 py-3 text-left"
                  disabled={opening !== null}
                  onClick={() => {
                    setOpening(project.projectId);
                    setError(null);
                    void onOpen(project)
                      .catch((caught) => setError(messageOf(caught)))
                      .finally(() => setOpening(null));
                  }}
                >
                  {opening === project.projectId ? (
                    <LoaderCircle aria-hidden className="size-5 shrink-0 animate-spin motion-reduce:animate-none" />
                  ) : (
                    <FolderOpen aria-hidden className="size-5 shrink-0 text-primary" />
                  )}
                  <span className="grid min-w-0">
                    <span className="truncate font-medium">{project.name}</span>
                    <span className="truncate text-xs font-normal text-muted-foreground">
                      {project.root}
                    </span>
                  </span>
                </Button>
              </li>
            ))}
          </ul>
        )}
      </main>
    </>
  );
}

function SessionList({
  project,
  onBack,
  onOpen,
}: {
  project: RemoteProject;
  onBack(): void;
  onOpen(sessionId: string | null): void;
}) {
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
        subtitle="대화"
        onBack={onBack}
        backLabel="프로젝트 목록으로"
      >
        <Button
          type="button"
          className="min-h-11 gap-1.5 px-3"
          onClick={() => onOpen(null)}
        >
          <Plus aria-hidden className="size-4" />
          새 대화
        </Button>
      </ScreenHeader>
      <main className="grid min-h-0 flex-1 content-start gap-3 overflow-y-auto px-4 py-4">
        {error && <ErrorText>{error}</ErrorText>}
        {sessions === null && !error && (
          <p role="status" className="flex items-center gap-2 text-sm text-muted-foreground">
            <LoaderCircle aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
            대화를 불러오는 중…
          </p>
        )}
        {sessions?.length === 0 && (
          <p className="text-sm text-muted-foreground">아직 대화가 없습니다. 새 대화를 시작해 보세요.</p>
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
                      aria-label={`${session.name} 메뉴`}
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
                      이름 바꾸기
                    </DropdownMenuItem>
                    <DropdownMenuItem variant="destructive" onSelect={() => setDeleting(session)}>
                      삭제
                    </DropdownMenuItem>
                  </DropdownMenuContent>
                </DropdownMenu>
              </li>
            ))}
          </ul>
        )}
      </main>

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
              <DialogTitle>대화 이름 바꾸기</DialogTitle>
              <DialogDescription>목록에 표시할 이름을 입력하세요.</DialogDescription>
            </DialogHeader>
            <Input
              aria-label="대화 이름"
              value={newName}
              maxLength={120}
              disabled={busy}
              onChange={(event) => setNewName(event.target.value)}
              className="h-11"
            />
            <DialogFooter>
              <Button type="button" variant="outline" className="min-h-11" onClick={() => setRenaming(null)}>
                취소
              </Button>
              <Button type="submit" className="min-h-11" disabled={busy || newName.trim().length === 0}>
                저장
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>

      <Dialog open={deleting !== null} onOpenChange={(open) => !open && setDeleting(null)}>
        <DialogContent className="w-[calc(100vw-2rem)] max-w-sm">
          <DialogHeader>
            <DialogTitle>대화를 삭제할까요?</DialogTitle>
            <DialogDescription>
              {deleting?.name} 대화가 PC에서도 삭제되며 되돌릴 수 없습니다.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button type="button" variant="outline" className="min-h-11" onClick={() => setDeleting(null)}>
              취소
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
              삭제
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}

export function RemoteWorkspace({ onDisconnect }: { onDisconnect(): void }) {
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
      setNotice(payload.error ?? "프로젝트 연결이 끊어졌습니다. 다시 선택해 주세요.");
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

  const openProject = useCallback(async (project: RemoteProject) => {
    const attached = await remoteTransport().attach(project.projectId);
    setNotice(null);
    setScreen({ kind: "sessions", project: attached });
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
          onBack={leaveProject}
          onOpen={(sessionId) =>
            setScreen({ kind: "chat", project: screen.project, sessionId, key: Date.now() })
          }
        />
      )}
      {screen.kind === "chat" && (
        <div className="min-h-0 flex-1">
          <RemoteChat
            key={screen.key}
            sessionId={screen.sessionId}
            projectName={screen.project.name}
            onBack={() => setScreen({ kind: "sessions", project: screen.project })}
          />
        </div>
      )}
    </div>
  );
}
