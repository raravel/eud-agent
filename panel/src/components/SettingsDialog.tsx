import { useEffect, useRef, useState } from "react";
import {
  ArrowLeft,
  Bell,
  Bot,
  CheckCircle2,
  ChevronRight,
  Download,
  FileInput,
  FileOutput,
  FolderKanban,
  FolderOpen,
  Hammer,
  LoaderCircle,
  Plus,
  RefreshCw,
  Volume2,
  X,
} from "lucide-react";
import {
  ProviderCard,
  ProviderStatusBadge,
} from "@/components/ProviderCard";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Switch } from "@/components/ui/switch";
import type {
  AppSettings,
  EuddraftSettings,
  NotificationChannelSettings,
  NotificationEvent,
} from "@/lib/ipc";
import type { StarcraftAvailability } from "@/lib/mapNew";
import { cn, formatPathForDisplay } from "@/lib/utils";
import {
  AVAILABILITY_LABELS,
  PROVIDER_DESCRIPTIONS,
  PROVIDER_LABELS,
} from "@/providers/providerCopy";
import type {
  ProviderId,
  ProviderModel,
  ProviderStatus,
  ReasoningSelection,
} from "@/providers/types";

export interface SettingsDialogProps {
  open: boolean;
  /** Category to show when the dialog opens for a specific task (e.g. 컴파일 for SCMDraft 2). */
  category?: SettingsCategory;
  settings: AppSettings | null;
  busy?: boolean;
  providerBusy?: ProviderId;
  providers: ProviderStatus[];
  providerModels: Partial<Record<ProviderId, ProviderModel[]>>;
  selectedModels: Partial<Record<ProviderId, string>>;
  selectedReasoning: Partial<Record<ProviderId, ReasoningSelection>>;
  versions?: Partial<Record<ProviderId, string>>;
  channels?: Partial<Record<ProviderId, string>>;
  baseUrls?: Partial<Record<ProviderId, string>>;
  hasApiKeys?: Partial<Record<ProviderId, boolean>>;
  providerErrors: Partial<Record<ProviderId, string>>;
  projectBusy?: "open" | "create" | "import" | "export" | null;
  euddraft: EuddraftSettings | null;
  euddraftBusy?: "load" | "check" | "update" | null;
  euddraftError?: string;
  scmdraftBusy?: boolean;
  scmdraftError?: string;
  /** Resolved StarCraft data folder for map rendering; null while loading or unreadable. */
  starcraft?: StarcraftAvailability | null;
  starcraftBusy?: boolean;
  starcraftError?: string;
  onOpenChange(open: boolean): void;
  onSettingsChange(settings: AppSettings): void;
  onReload(): void;
  onPreviewSound(): void;
  onSelectProvider(provider: ProviderId): Promise<void> | void;
  onProviderInstall(provider: ProviderId): Promise<void> | void;
  loginPending?: Partial<Record<ProviderId, boolean>>;
  onProviderLogin(provider: ProviderId): Promise<void> | void;
  onProviderLoginCancel(provider: ProviderId): Promise<void> | void;
  onProviderImport(provider: ProviderId): Promise<void> | void;
  onProviderApiKey(provider: ProviderId, key: string): Promise<void> | void;
  onProviderBaseUrl(provider: ProviderId, baseUrl: string): Promise<void> | void;
  onProviderLogout(provider: ProviderId): Promise<void> | void;
  onProviderRefresh(provider: ProviderId): Promise<void> | void;
  onProviderModelChange(
    provider: ProviderId,
    model: string,
    reasoning: ReasoningSelection | undefined,
  ): Promise<void> | void;
  onProjectOpen(): void;
  onProjectCreate(): void;
  onProjectImport(): void;
  onProjectExport(): void;
  onEuddraftCheck(): void;
  onEuddraftUpdate(): void;
  onScmdraftPick(): Promise<void> | void;
  onStarcraftPick?(): Promise<void> | void;
}

export type SettingsCategory = "project" | "compile" | "providers" | "notifications";

const EVENT_COPY: Readonly<
  Record<NotificationEvent, { title: string; description: string }>
> = {
  agentTurnComplete: {
    title: "에이전트 턴 종료",
    description: "계획 검토를 제외한 에이전트 턴이 종료됐을 때",
  },
  askResponseRequired: {
    title: "ASK 응답 필요",
    description: "에이전트가 질문에 대한 사용자 응답을 기다릴 때",
  },
  planApproval: {
    title: "계획 승인 필요",
    description: "새 계획안이나 수정된 계획안이 도착했을 때",
  },
  reviewRequired: {
    title: "검토할 변경 도착",
    description: "하네스가 정리한 문서 변경이나 맵 후보가 검토를 기다릴 때",
  },
};

interface EuddraftSettingsPanelProps {
  status: EuddraftSettings | null;
  busy: "load" | "check" | "update" | null;
  error?: string;
  onCheck(): void;
  onUpdate(): void;
}

function EuddraftSettingsPanel({
  status,
  busy,
  error,
  onCheck,
  onUpdate,
}: EuddraftSettingsPanelProps) {
  return (
    <div className="animate-in fade-in duration-200 motion-reduce:animate-none">
      <div className="flex items-start justify-between gap-4">
        <div>
          <h2 className="text-base font-semibold">euddraft 컴파일 도구</h2>
          <p className="mt-1.5 text-sm leading-6 text-muted-foreground">
            설치 경로와 관리형 배포판 버전을 확인하고 최신 공식 릴리스로
            업데이트합니다.
          </p>
        </div>
        {busy !== null && (
          <span
            role="status"
            className="flex shrink-0 items-center gap-1.5 text-xs text-muted-foreground"
          >
            <LoaderCircle
              aria-hidden
              className="size-3.5 animate-spin motion-reduce:animate-none"
            />
            {busy === "update"
              ? "업데이트 중…"
              : busy === "check"
                ? "확인 중…"
                : "불러오는 중…"}
          </span>
        )}
      </div>

      {status ? (
        <>
          <dl className="mt-5 overflow-hidden rounded-xl border border-border bg-card/40">
            <div className="grid gap-1.5 border-b border-border px-4 py-3.5">
              <dt className="text-xs font-medium text-muted-foreground">
                설치 경로
              </dt>
              <dd
                className="break-all font-mono text-sm text-foreground"
                title={formatPathForDisplay(status.path)}
              >
                {formatPathForDisplay(status.path) || "설정되지 않음"}
              </dd>
            </div>
            <div className="grid grid-cols-2 gap-4 px-4 py-3.5">
              <div>
                <dt className="text-xs font-medium text-muted-foreground">
                  현재 버전
                </dt>
                <dd className="mt-1 text-sm font-semibold">
                  {status.installedVersion ?? "버전 정보 없음"}
                </dd>
              </div>
              <div>
                <dt className="text-xs font-medium text-muted-foreground">
                  최신 버전
                </dt>
                <dd className="mt-1 text-sm font-semibold">
                  {status.latestVersion ?? "확인 전"}
                </dd>
              </div>
            </div>
          </dl>

          {!status.valid && (
            <p
              role="alert"
              className="mt-3 rounded-lg border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm text-destructive"
            >
              설정된 euddraft를 찾을 수 없습니다. 부트스트랩 화면에서 경로를
              다시 선택해 주세요.
            </p>
          )}
          {status.valid && !status.managed && (
            <p className="mt-3 rounded-lg border border-border bg-muted/30 px-4 py-3 text-xs leading-5 text-muted-foreground">
              직접 선택한 배포판은 버전을 자동 판별할 수 없습니다. 최신 관리형
              배포판을 설치하면 이후 버전을 자동으로 비교할 수 있습니다.
            </p>
          )}
          {status.latestVersion &&
            status.managed &&
            !status.updateAvailable && (
              <p className="mt-3 flex items-center gap-2 rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-4 py-3 text-sm text-emerald-700 dark:text-emerald-300">
                <CheckCircle2 aria-hidden className="size-4 shrink-0" />
                최신 버전을 사용 중입니다.
              </p>
            )}
          {status.updateAvailable && (
            <p className="mt-3 rounded-lg border border-amber-500/30 bg-amber-500/10 px-4 py-3 text-sm text-amber-800 dark:text-amber-200">
              새 euddraft {status.latestVersion} 버전을 설치할 수 있습니다.
            </p>
          )}
          {error && (
            <p
              role="alert"
              className="mt-3 rounded-lg border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm text-destructive"
            >
              {error}
            </p>
          )}

          <div className="mt-5 flex flex-col gap-3 sm:flex-row sm:justify-end">
            <Button
              type="button"
              variant="outline"
              className="h-11"
              disabled={busy !== null}
              onClick={onCheck}
            >
              <RefreshCw aria-hidden className="size-4" />
              최신 버전 확인
            </Button>
            {status.latestVersion &&
              (status.updateAvailable || !status.managed) && (
                <Button
                  type="button"
                  className="h-11"
                  disabled={busy !== null}
                  onClick={onUpdate}
                >
                  {busy === "update" ? (
                    <LoaderCircle
                      aria-hidden
                      className="size-4 animate-spin motion-reduce:animate-none"
                    />
                  ) : (
                    <Download aria-hidden className="size-4" />
                  )}
                  {status.managed
                    ? "최신 버전으로 업데이트"
                    : "최신 관리형 버전 설치"}
                </Button>
              )}
          </div>
        </>
      ) : (
        <div className="mt-5 flex min-h-52 flex-col items-center justify-center rounded-xl border border-dashed border-border text-center">
          <p className="text-sm text-muted-foreground">
            euddraft 설정을 불러오지 못했습니다.
          </p>
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="mt-3 h-11 gap-1.5"
            disabled={busy !== null}
            onClick={onCheck}
          >
            <RefreshCw aria-hidden className="size-3.5" />
            다시 시도
          </Button>
        </div>
      )}
    </div>
  );
}

interface ScmdraftSettingsPanelProps {
  path: string;
  busy: boolean;
  error?: string;
  onPick(): Promise<void> | void;
}

function ScmdraftSettingsPanel({ path, busy, error, onPick }: ScmdraftSettingsPanelProps) {
  return (
    <section aria-labelledby="settings-scmdraft-heading" className="mt-8">
      <h2 id="settings-scmdraft-heading" className="text-base font-semibold">SCMDraft 2</h2>
      <p className="mt-1.5 text-sm leading-6 text-muted-foreground">
        상단의 &quot;SCMDraft 2로 열기&quot;가 이 실행 파일로 원본 맵을 엽니다.
      </p>
      <dl className="mt-5 overflow-hidden rounded-xl border border-border bg-card/40">
        <div className="grid gap-1.5 px-4 py-3.5">
          <dt className="text-xs font-medium text-muted-foreground">실행 파일</dt>
          <dd
            className="break-all font-mono text-sm text-foreground"
            title={formatPathForDisplay(path)}
          >
            {formatPathForDisplay(path) || "지정되지 않음"}
          </dd>
        </div>
      </dl>
      {error && (
        <p
          role="alert"
          className="mt-3 rounded-lg border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm text-destructive"
        >
          {error}
        </p>
      )}
      <div className="mt-5 flex flex-col gap-3 sm:flex-row sm:justify-end">
        <Button
          type="button"
          variant="outline"
          className="h-11"
          disabled={busy}
          aria-busy={busy || undefined}
          onClick={() => void onPick()}
        >
          {busy ? (
            <LoaderCircle aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
          ) : (
            <FolderOpen aria-hidden className="size-4" />
          )}
          {busy ? "선택 중…" : "실행 파일 선택"}
        </Button>
      </div>
    </section>
  );
}

interface StarcraftSettingsPanelProps {
  availability: StarcraftAvailability | null;
  busy: boolean;
  error?: string;
  onPick(): Promise<void> | void;
}

function StarcraftSettingsPanel({ availability, busy, error, onPick }: StarcraftSettingsPanelProps) {
  const path = availability?.available ? availability.path : "";
  return (
    <section aria-labelledby="settings-starcraft-heading" className="mt-8">
      <h2 id="settings-starcraft-heading" className="text-base font-semibold">StarCraft</h2>
      <p className="mt-1.5 text-sm leading-6 text-muted-foreground">
        맵 렌더링, 미니맵, 빈 맵 만들기가 이 설치 폴더의 타일셋 데이터를 읽습니다.
      </p>
      <dl className="mt-5 overflow-hidden rounded-xl border border-border bg-card/40">
        <div className="grid gap-1.5 px-4 py-3.5">
          <dt className="text-xs font-medium text-muted-foreground">설치 폴더</dt>
          <dd
            className="break-all font-mono text-sm text-foreground"
            title={formatPathForDisplay(path)}
          >
            {formatPathForDisplay(path) || (availability ? "찾지 못함" : "확인 중…")}
          </dd>
        </div>
      </dl>
      {availability && !availability.available && availability.reason && !error && (
        <p className="mt-3 text-sm leading-6 text-muted-foreground">{availability.reason}</p>
      )}
      {error && (
        <p
          role="alert"
          className="mt-3 rounded-lg border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm text-destructive"
        >
          {error}
        </p>
      )}
      <div className="mt-5 flex flex-col gap-3 sm:flex-row sm:justify-end">
        <Button
          type="button"
          variant="outline"
          className="h-11"
          disabled={busy}
          aria-busy={busy || undefined}
          onClick={() => void onPick()}
        >
          {busy ? (
            <LoaderCircle aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
          ) : (
            <FolderOpen aria-hidden className="size-4" />
          )}
          {busy ? "선택 중…" : "폴더 선택"}
        </Button>
      </div>
    </section>
  );
}

export function SettingsDialog({
  open,
  category: requestedCategory,
  settings,
  busy = false,
  providerBusy,
  providers,
  providerModels,
  selectedModels,
  selectedReasoning,
  versions = {},
  channels = {},
  baseUrls = {},
  hasApiKeys = {},
  providerErrors,
  projectBusy = null,
  euddraft,
  euddraftBusy = null,
  euddraftError,
  scmdraftBusy = false,
  scmdraftError,
  starcraft = null,
  starcraftBusy = false,
  starcraftError,
  onOpenChange,
  onSettingsChange,
  onReload,
  onPreviewSound,
  onSelectProvider,
  loginPending = {},
  onProviderInstall,
  onProviderLogin,
  onProviderLoginCancel,
  onProviderImport,
  onProviderApiKey,
  onProviderBaseUrl,
  onProviderLogout,
  onProviderRefresh,
  onProviderModelChange,
  onProjectOpen,
  onProjectCreate,
  onProjectImport,
  onProjectExport,
  onEuddraftCheck,
  onEuddraftUpdate,
  onScmdraftPick,
  onStarcraftPick,
}: SettingsDialogProps) {
  const [category, setCategory] = useState<SettingsCategory>("providers");
  const [selectedProvider, setSelectedProvider] = useState<ProviderId>();
  const detailHeadingRef = useRef<HTMLHeadingElement>(null);
  const providerButtonRefs = useRef<
    Partial<Record<ProviderId, HTMLButtonElement | null>>
  >({});
  const providerToRestoreRef = useRef<ProviderId | undefined>(undefined);
  const selectedProviderStatus = providers.find(
    (status) => status.provider === selectedProvider,
  );

  useEffect(() => {
    if (open) return;
    setSelectedProvider(undefined);
    providerToRestoreRef.current = undefined;
  }, [open]);

  useEffect(() => {
    if (!open || !requestedCategory) return;
    setCategory(requestedCategory);
    setSelectedProvider(undefined);
  }, [open, requestedCategory]);

  useEffect(() => {
    if (!open || category !== "providers") return;
    if (selectedProvider) {
      detailHeadingRef.current?.focus();
      return;
    }
    const provider = providerToRestoreRef.current;
    if (provider) providerButtonRefs.current[provider]?.focus();
  }, [category, open, selectedProvider]);

  const openProvider = (provider: ProviderId) => {
    providerToRestoreRef.current = provider;
    setSelectedProvider(provider);
  };


  const selectCategory = (nextCategory: SettingsCategory) => {
    setCategory(nextCategory);
    setSelectedProvider(undefined);
  };

  const updateEvent = (
    event: NotificationEvent,
    field: keyof NotificationChannelSettings,
    checked: boolean,
  ) => {
    if (!settings) return;
    onSettingsChange({
      ...settings,
      notifications: {
        ...settings.notifications,
        [event]: {
          ...settings.notifications[event],
          [field]: checked,
        },
      },
    });
  };

  const updateDeepPlanning = (checked: boolean) => {
    if (!settings) return;
    onSettingsChange({ ...settings, deepPlanning: checked });
  };

  const updateLargeContext = (model: string, checked: boolean) => {
    if (!settings) return;
    const enabled = new Set(settings.codexLargeContextModels);
    if (checked) enabled.add(model);
    else enabled.delete(model);
    onSettingsChange({
      ...settings,
      codexLargeContextModels: Array.from(enabled).sort(),
    });
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        showCloseButton={false}
        className="grid h-[calc(100dvh-1rem)] max-h-[50rem] grid-rows-[auto_minmax(0,1fr)] gap-0 overflow-hidden p-0 sm:h-[calc(100vh-3rem)] sm:max-w-5xl"
      >
        <DialogHeader className="relative border-b border-border px-4 py-4 pr-16 text-left sm:px-6 sm:py-5">
          <DialogTitle>설정</DialogTitle>
          <DialogDescription>
            Native 프로젝트, 컴파일 도구, AI 제공자와 사용자 확인 알림을
            관리합니다.
          </DialogDescription>
          <DialogClose asChild>
            <Button
              type="button"
              size="icon"
              variant="ghost"
              className="absolute right-3 top-3 size-11 sm:right-4 sm:top-4"
              aria-label="설정 닫기"
            >
              <X aria-hidden className="size-4" />
            </Button>
          </DialogClose>
        </DialogHeader>

        <div className="grid min-h-0 grid-rows-[auto_minmax(0,1fr)] overflow-hidden sm:grid-cols-[10.5rem_minmax(0,1fr)] sm:grid-rows-1">
          <nav
            aria-label="설정 범주"
            className="flex gap-1 border-b border-border bg-card/40 p-2 sm:block sm:border-b-0 sm:border-r sm:p-3"
          >
            <Button
              type="button"
              variant={category === "project" ? "secondary" : "ghost"}
              className="h-11 flex-1 justify-start gap-2 sm:w-full"
              aria-current={category === "project" ? "page" : undefined}
              onClick={() => selectCategory("project")}
            >
              <FolderKanban aria-hidden className="size-4" />
              프로젝트
            </Button>
            <Button
              type="button"
              variant={category === "compile" ? "secondary" : "ghost"}
              className="h-11 flex-1 justify-start gap-2 sm:mt-1 sm:w-full"
              aria-current={category === "compile" ? "page" : undefined}
              onClick={() => selectCategory("compile")}
            >
              <Hammer aria-hidden className="size-4" />
              컴파일
            </Button>
            <Button
              type="button"
              variant={category === "providers" ? "secondary" : "ghost"}
              className="h-11 flex-1 justify-start gap-2 sm:mt-1 sm:w-full"
              aria-current={category === "providers" ? "page" : undefined}
              onClick={() => selectCategory("providers")}
            >
              <Bot aria-hidden className="size-4" />
              AI 제공자
            </Button>
            <Button
              type="button"
              variant={category === "notifications" ? "secondary" : "ghost"}
              className="h-11 flex-1 justify-start gap-2 sm:mt-1 sm:w-full"
              aria-current={category === "notifications" ? "page" : undefined}
              onClick={() => selectCategory("notifications")}
            >
              <Bell aria-hidden className="size-4" />
              알림
            </Button>
          </nav>

          <section className="min-h-0 min-w-0 overflow-y-auto overscroll-contain px-4 py-4 sm:px-6 sm:py-5">
            {category === "project" ? (
              <div className="animate-in fade-in duration-200 motion-reduce:animate-none">
                <div className="flex items-start justify-between gap-4">
                  <div>
                    <h2 className="text-base font-semibold">Native 프로젝트</h2>
                    <p className="mt-1.5 text-sm leading-6 text-muted-foreground">
                      프로젝트를 전환하거나 E3S 호환 파일을 가져오고 내보냅니다.
                    </p>
                  </div>
                  {projectBusy !== null && (
                    <span
                      role="status"
                      className="flex shrink-0 items-center gap-1.5 text-xs text-muted-foreground"
                    >
                      <LoaderCircle
                        aria-hidden
                        className="size-3.5 animate-spin motion-reduce:animate-none"
                      />
                      처리 중…
                    </span>
                  )}
                </div>
                <div className="mt-5 grid gap-3 rounded-xl border border-border bg-card/40 p-4">
                  <Button
                    type="button"
                    className="min-h-11 justify-start"
                    disabled={projectBusy !== null}
                    onClick={onProjectOpen}
                  >
                    <FolderKanban aria-hidden className="size-4" />
                    기존 Native 프로젝트 열기
                  </Button>
                  <div className="grid grid-cols-2 gap-3">
                    <Button
                      type="button"
                      variant="outline"
                      className="min-h-11 justify-start"
                      disabled={projectBusy !== null}
                      onClick={onProjectCreate}
                    >
                      <Plus aria-hidden className="size-4" />
                      새 프로젝트
                    </Button>
                    <Button
                      type="button"
                      variant="outline"
                      className="min-h-11 justify-start"
                      disabled={projectBusy !== null}
                      onClick={onProjectImport}
                    >
                      <FileInput aria-hidden className="size-4" />
                      E3S 가져오기
                    </Button>
                  </div>
                  <Button
                    type="button"
                    variant="outline"
                    className="min-h-11 justify-start"
                    disabled={projectBusy !== null}
                    onClick={onProjectExport}
                  >
                    <FileOutput aria-hidden className="size-4" />
                    E3S 내보내기
                  </Button>
                </div>
                <p className="mt-4 text-xs leading-relaxed text-muted-foreground">
                  E3S 내보내기는 E3S에서 가져온 프로젝트만 지원합니다. Native
                  프로젝트가 원본이며, EUD Editor 실행 환경은 사용하지 않습니다.
                </p>
              </div>
            ) : category === "compile" ? (
              <>
                <EuddraftSettingsPanel
                  status={euddraft}
                  busy={euddraftBusy}
                  error={euddraftError}
                  onCheck={onEuddraftCheck}
                  onUpdate={onEuddraftUpdate}
                />
                <ScmdraftSettingsPanel
                  path={settings?.scmdraftPath ?? ""}
                  busy={busy || scmdraftBusy}
                  error={scmdraftError}
                  onPick={onScmdraftPick}
                />
                {onStarcraftPick && (
                  <StarcraftSettingsPanel
                    availability={starcraft}
                    busy={busy || starcraftBusy}
                    error={starcraftError}
                    onPick={onStarcraftPick}
                  />
                )}
              </>
            ) : category === "providers" ? (
              selectedProviderStatus ? (
                <div className="animate-in fade-in slide-in-from-right-2 duration-200 motion-reduce:animate-none">
                  <Button
                    type="button"
                    variant="ghost"
                    className="-ml-2 h-11 gap-2 px-2 text-muted-foreground"
                    onClick={() => setSelectedProvider(undefined)}
                  >
                    <ArrowLeft aria-hidden className="size-4" />
                    모든 제공자
                  </Button>
                  <div className="mt-2 flex items-start justify-between gap-4">
                    <div>
                      <h2
                        ref={detailHeadingRef}
                        tabIndex={-1}
                        className="text-base font-semibold outline-none"
                      >
                        {PROVIDER_LABELS[selectedProviderStatus.provider]} 설정
                      </h2>
                      <p className="mt-1.5 text-sm leading-6 text-muted-foreground">
                        연결 상태와 새 세션에 사용할 기본 모델을 관리합니다.
                      </p>
                    </div>
                    {(busy || providerBusy === selectedProviderStatus.provider) && (
                      <span
                        role="status"
                        className="flex shrink-0 items-center gap-1.5 text-xs text-muted-foreground"
                      >
                        <LoaderCircle
                          aria-hidden
                          className="size-3.5 animate-spin motion-reduce:animate-none"
                        />
                        처리 중…
                      </span>
                    )}
                  </div>
                  <div className="mt-5">
                    <ProviderCard
                      status={selectedProviderStatus}
                      selected={selectedProviderStatus.selectedAsDefault}
                      models={providerModels[selectedProviderStatus.provider]}
                      selectedModel={selectedModels[selectedProviderStatus.provider]}
                      selectedReasoning={
                        selectedReasoning[selectedProviderStatus.provider]
                      }
                      version={versions[selectedProviderStatus.provider]}
                      channel={channels[selectedProviderStatus.provider]}
                      baseUrl={baseUrls[selectedProviderStatus.provider]}
                      hasApiKey={
                        hasApiKeys[selectedProviderStatus.provider] === true
                      }
                      busy={providerBusy === selectedProviderStatus.provider}
                      loginInProgress={
                        loginPending[selectedProviderStatus.provider] === true
                      }
                      error={providerErrors[selectedProviderStatus.provider]}
                      onSelectDefault={onSelectProvider}
                      onInstall={onProviderInstall}
                      onLogin={onProviderLogin}
                      onLoginCancel={onProviderLoginCancel}
                      onImport={onProviderImport}
                      onApiKey={onProviderApiKey}
                      onBaseUrl={onProviderBaseUrl}
                      onLogout={onProviderLogout}
                      onRefresh={onProviderRefresh}
                      onModelChange={onProviderModelChange}
                    />
                    {selectedProviderStatus.provider === "codex" &&
                      settings &&
                      (providerModels.codex?.length ?? 0) > 0 && (
                        <div className="mt-3 rounded-xl border border-border bg-card/40 p-4">
                          <p className="text-xs font-medium text-foreground">
                            Codex 1M 컨텍스트 opt-in
                          </p>
                          <div className="mt-2 grid gap-2">
                            {providerModels.codex?.map((model) => (
                              <label
                                key={model.model}
                                className="flex min-h-11 items-center justify-between gap-3 text-xs text-muted-foreground"
                              >
                                <span className="min-w-0 truncate">
                                  {model.displayName}
                                </span>
                                <Switch
                                  checked={settings.codexLargeContextModels.includes(
                                    model.model,
                                  )}
                                  disabled={busy}
                                  aria-label={`${model.displayName} 1M 컨텍스트`}
                                  onCheckedChange={(checked) =>
                                    updateLargeContext(model.model, checked)
                                  }
                                />
                              </label>
                            ))}
                          </div>
                        </div>
                      )}
                  </div>
                  <div className="mt-5 flex flex-col gap-3 border-t border-border pt-4 sm:flex-row sm:items-center sm:justify-between">
                    <p className="text-xs leading-5 text-muted-foreground">
                      변경 내용은 각 항목에서 즉시 저장됩니다.
                    </p>
                    <Button
                      type="button"
                      className="h-11 sm:min-w-28"
                      onClick={() => setSelectedProvider(undefined)}
                    >
                      <CheckCircle2 aria-hidden className="size-4" />
                      설정 완료
                    </Button>
                  </div>
                </div>
              ) : (
                <div className="animate-in fade-in duration-200 motion-reduce:animate-none">
                  <div className="flex items-start justify-between gap-4">
                    <div>
                      <h2 className="text-base font-semibold">AI 제공자</h2>
                      <p className="mt-1.5 text-sm leading-6 text-muted-foreground">
                        설정할 제공자를 선택하세요. 선택한 제공자의 연결과
                        모델 옵션만 표시합니다.
                      </p>
                    </div>
                    {(busy || providerBusy) && (
                      <span
                        role="status"
                        className="flex shrink-0 items-center gap-1.5 text-xs text-muted-foreground"
                      >
                        <LoaderCircle
                          aria-hidden
                          className="size-3.5 animate-spin motion-reduce:animate-none"
                        />
                        처리 중…
                      </span>
                    )}
                  </div>
                  <ul
                    aria-label="AI 제공자 목록"
                    className="mt-5 grid gap-3"
                  >
                    {providers.map((status) => (
                      <li key={status.provider}>
                        <Button
                          ref={(node) => {
                            providerButtonRefs.current[status.provider] = node;
                          }}
                          type="button"
                          variant="outline"
                          className={cn(
                            "group h-auto min-h-[5.5rem] w-full justify-start gap-3 whitespace-normal rounded-xl px-4 py-3 text-left shadow-none",
                            status.selectedAsDefault &&
                              "border-primary/50 bg-primary/5",
                          )}
                          aria-label={`${PROVIDER_LABELS[status.provider]} 설정 열기. ${AVAILABILITY_LABELS[status.availability]}${status.selectedAsDefault ? ". 기본 제공자" : ""}`}
                          onClick={() => openProvider(status.provider)}
                        >
                          <span className="flex size-10 shrink-0 items-center justify-center rounded-lg border border-border bg-muted/60 text-muted-foreground">
                            <Bot aria-hidden className="size-5" />
                          </span>
                          <span className="min-w-0 flex-1">
                            <span className="flex flex-wrap items-center gap-2">
                              <span className="font-semibold text-foreground">
                                {PROVIDER_LABELS[status.provider]}
                              </span>
                              {status.selectedAsDefault && (
                                <span className="inline-flex items-center gap-1 text-xs font-medium text-primary">
                                  <CheckCircle2
                                    aria-hidden
                                    className="size-3.5"
                                  />
                                  기본 제공자
                                </span>
                              )}
                            </span>
                            <span className="mt-1 block text-xs leading-5 text-muted-foreground">
                              {PROVIDER_DESCRIPTIONS[status.provider]}
                            </span>
                          </span>
                          <ProviderStatusBadge
                            availability={status.availability}
                            className="self-start sm:self-center"
                          />
                          <ChevronRight
                            aria-hidden
                            className="size-4 shrink-0 text-muted-foreground transition-transform duration-200 group-hover:translate-x-0.5 motion-reduce:transform-none motion-reduce:transition-none"
                          />
                        </Button>
                      </li>
                    ))}
                  </ul>
                  <div className="mt-4 rounded-xl border border-border bg-card/40 px-4 py-3">
                    <p className="text-xs leading-5 text-muted-foreground">
                      기본 제공자는 새 세션에만 적용됩니다. 기존 EPS·Map
                      세션과 하네스 작업의 제공자는 바뀌지 않습니다.
                    </p>
                  </div>
                  {settings && (
                    <label className="mt-4 flex min-h-11 items-center justify-between gap-4 rounded-xl border border-border bg-card/40 px-4 py-3.5">
                      <span className="min-w-0">
                        <span className="block text-sm font-medium text-foreground">
                          더 똑똑한 계획
                        </span>
                        <span className="mt-0.5 block text-xs leading-5 text-muted-foreground">
                          계획마다 모델 호출이 여러 번 추가되어 토큰 사용량이 크게 늘어납니다.
                        </span>
                      </span>
                      <Switch
                        checked={settings.deepPlanning}
                        disabled={busy}
                        aria-label="더 똑똑한 계획"
                        onCheckedChange={updateDeepPlanning}
                      />
                    </label>
                  )}
                </div>
              )
            ) : (
              <>
                <div className="flex items-start justify-between gap-4">
                  <div>
                    <h2 className="text-base font-semibold">알림</h2>
                    <p className="mt-1.5 text-sm text-muted-foreground">
                      사용자 확인이 필요한 순간을 놓치지 않도록 알려드립니다.
                    </p>
                  </div>
                  {busy && (
                    <span role="status" className="flex shrink-0 items-center gap-1.5 text-xs text-muted-foreground">
                      <LoaderCircle aria-hidden className="size-3.5 animate-spin motion-reduce:animate-none" />
                      저장 중…
                    </span>
                  )}
                </div>
                {settings ? (
                  <>
                    <div className="mt-5 overflow-hidden rounded-xl border border-border bg-card/40">
                      <div className="grid grid-cols-[minmax(0,1fr)_5.5rem_5.5rem] gap-2 border-b border-border px-4 py-2.5 text-xs font-medium text-muted-foreground">
                        <span>알림 시점</span>
                        <span className="text-center">알림음</span>
                        <span className="text-center">OS 알림</span>
                      </div>
                      {(Object.keys(EVENT_COPY) as NotificationEvent[]).map((event) => {
                        const copy = EVENT_COPY[event];
                        const value = settings.notifications[event];
                        return (
                          <div key={event} className="grid min-h-20 grid-cols-[minmax(0,1fr)_5.5rem_5.5rem] items-center gap-2 border-t border-border px-4 py-3 first:border-t-0">
                            <div className="min-w-0 pr-3">
                              <p className="text-sm font-medium text-foreground">{copy.title}</p>
                              <p className="mt-1 text-xs leading-relaxed text-muted-foreground">{copy.description}</p>
                            </div>
                            <Switch
                              checked={value.sound}
                              disabled={busy}
                              aria-label={`${copy.title} 알림음`}
                              className="justify-self-center"
                              onCheckedChange={(checked) => updateEvent(event, "sound", checked)}
                            />
                            <Switch
                              checked={value.osNotification}
                              disabled={busy}
                              aria-label={`${copy.title} OS 알림`}
                              className="justify-self-center"
                              onCheckedChange={(checked) => updateEvent(event, "osNotification", checked)}
                            />
                          </div>
                        );
                      })}
                    </div>
                    <div className="mt-4 flex items-center justify-between gap-4 rounded-xl border border-border bg-card/40 px-4 py-3.5">
                      <div className="flex items-center gap-3">
                        <Volume2 aria-hidden className="size-4 text-primary" />
                        <div>
                          <p className="text-sm font-medium">기본 알림음</p>
                          <p className="mt-0.5 text-xs text-muted-foreground">Windows 기본 알림음을 사용합니다.</p>
                        </div>
                      </div>
                      <Button type="button" variant="outline" size="sm" onClick={onPreviewSound}>
                        소리 미리듣기
                      </Button>
                    </div>
                  </>
                ) : (
                  <div className="mt-5 flex min-h-52 flex-col items-center justify-center rounded-xl border border-dashed border-border text-center">
                    <p className="text-sm text-muted-foreground">알림 설정을 불러오지 못했습니다.</p>
                    <Button type="button" variant="outline" size="sm" className="mt-3 h-11 gap-1.5" onClick={onReload}>
                      <RefreshCw aria-hidden className="size-3.5" />
                      다시 시도
                    </Button>
                  </div>
                )}
              </>
            )}
          </section>
        </div>
      </DialogContent>
    </Dialog>
  );
}
