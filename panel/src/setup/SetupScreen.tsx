import { useEffect, useState } from "react";
import { BotIcon, CheckIcon, CircleAlertIcon, FolderOpenIcon, Loader2Icon } from "lucide-react";

import { ProviderCard } from "@/components/ProviderCard";
import { Button } from "@/components/ui/button";
import { euddraftExecutableName } from "@/lib/platform";
import { cn } from "@/lib/utils";
import type { BootstrapView } from "@/setup/bootstrap";
import {
  AVAILABILITY_LABELS,
  PROVIDER_LABELS,
} from "@/providers/providerCopy";
import type {
  ProviderId,
  ProviderModel,
  ProviderStatus,
  ReasoningSelection,
} from "@/providers/types";

const PICK_ERROR_TEXT: Readonly<Record<string, string>> = {
  invalid_project_folder:
    ".eap 파일이 있는 Native EUD 프로젝트 폴더를 선택해 주세요.",
  invalid_euddraft_path:
    `${euddraftExecutableName()} 또는 euddraft.py 파일이나 해당 파일이 있는 폴더를 선택해 주세요.`,
};

function projectErrorText(error: string): string {
  const separator = error.indexOf(":");
  if (separator > 0) {
    const detail = error.slice(separator + 1).trim();
    if (detail) return detail;
  }
  if (error.startsWith("project_create_failed:")) {
    return "프로젝트를 만들지 못했습니다. 원본 맵과 비어 있는 대상 폴더를 확인해 주세요.";
  }
  if (error.startsWith("e3s_import_failed:")) {
    return "E3S 프로젝트를 가져오지 못했습니다. 참조 맵과 비어 있는 대상 폴더를 확인해 주세요.";
  }
  return PICK_ERROR_TEXT[error] ?? "설정 경로를 확인하지 못했습니다.";
}

export interface SetupScreenProps {
  projectValid: boolean;
  euddraftPath?: string;
  euddraftValid: boolean;
  pickError: string | null;
  onPickProject(): void;
  onCreateProject(): void;
  onImportE3s(): void;
  projectAction?: "open" | "create" | "import" | null;
  onPickEuddraft(directory?: boolean): void;
  onInstallEuddraft(): void;
  euddraftAction?: "file" | "folder" | "install" | null;
  bootstrapActive?: boolean;
  view: BootstrapView;
  error: string | null;
  onRetry(): void;
  assetsReady: boolean;
  defaultProvider?: ProviderId;
  providers: ProviderStatus[];
  models: Partial<Record<ProviderId, ProviderModel[]>>;
  selectedModels: Partial<Record<ProviderId, string>>;
  selectedReasoning: Partial<Record<ProviderId, ReasoningSelection>>;
  versions?: Partial<Record<ProviderId, string>>;
  channels?: Partial<Record<ProviderId, string>>;
  baseUrls?: Partial<Record<ProviderId, string>>;
  hasApiKeys?: Partial<Record<ProviderId, boolean>>;
  busyProvider?: ProviderId;
  loginPending?: Partial<Record<ProviderId, boolean>>;
  providerErrors: Partial<Record<ProviderId, string>>;
  onSelectProvider(provider: ProviderId): Promise<void> | void;
  onProviderInstall(provider: ProviderId): Promise<void> | void;
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
}

const STEPS = [
  "프로젝트",
  "euddraft",
  "에셋 다운로드",
  "AI 제공자 선택",
  "선택 제공자 연결",
] as const;

export function SetupScreen({
  projectValid,
  euddraftPath = "",
  euddraftValid,
  pickError,
  onPickProject,
  onCreateProject,
  onImportE3s,
  projectAction = null,
  onPickEuddraft,
  onInstallEuddraft,
  euddraftAction = null,
  bootstrapActive = false,
  view,
  error,
  onRetry,
  assetsReady,
  defaultProvider,
  providers,
  models,
  selectedModels,
  selectedReasoning,
  versions = {},
  channels = {},
  baseUrls = {},
  hasApiKeys = {},
  busyProvider,
  loginPending = {},
  providerErrors,
  onSelectProvider,
  onProviderInstall,
  onProviderLogin,
  onProviderLoginCancel,
  onProviderImport,
  onProviderApiKey,
  onProviderBaseUrl,
  onProviderLogout,
  onProviderRefresh,
  onProviderModelChange,
}: SetupScreenProps) {
  const [selectedProvider, setSelectedProvider] = useState<ProviderId | "">(
    defaultProvider ?? "",
  );
  useEffect(() => {
    setSelectedProvider(defaultProvider ?? "");
  }, [defaultProvider]);
  const selectedStatus = providers.find(
    (status) => status.provider === selectedProvider,
  );
  const selectedConnected =
    selectedStatus?.availability === "ready" &&
    selectedProvider !== "" &&
    !!selectedModels[selectedProvider];
  const currentStep = !projectValid
    ? 0
    : !euddraftValid
      ? 1
      : !assetsReady
        ? 2
        : !selectedProvider
          ? 3
          : selectedConnected
            ? 5
            : 4;
  const hasConfiguredEuddraft = euddraftPath.trim().length > 0;

  return (
    <main className="min-h-dvh overflow-y-auto bg-background px-4 py-8 text-foreground sm:px-8">
      <div className="mx-auto w-full max-w-5xl">
        <header className="text-center">
          <p className="text-sm font-medium text-primary">eud-agent 시작 설정</p>
          <h1 className="mt-2 text-2xl font-semibold tracking-tight sm:text-3xl">
            작업 환경과 AI 제공자를 연결합니다
          </h1>
          <p className="mx-auto mt-3 max-w-2xl break-keep text-sm leading-6 text-muted-foreground">
            선택한 기본 제공자만 시작 조건입니다. 다른 제공자는 지금 연결하지 않아도 되며,
            나중에 설정의 <span className="whitespace-nowrap">AI 제공자 화면에서</span>{" "}
            관리할 수 있습니다.
          </p>
        </header>

        <ol className="mt-8 grid gap-2 rounded-xl border border-border bg-card/60 p-3 sm:grid-cols-5">
          {STEPS.map((label, index) => {
            const state = index < currentStep ? "done" : index === currentStep ? "current" : "pending";
            return (
              <li
                key={label}
                aria-current={state === "current" ? "step" : undefined}
                className={cn(
                  "flex min-h-11 items-center gap-2 rounded-lg px-3 text-sm",
                  state === "current" && "bg-primary/10 text-foreground",
                  state !== "current" && "text-muted-foreground",
                )}
              >
                <span
                  className={cn(
                    "flex size-7 shrink-0 items-center justify-center rounded-full border text-xs font-semibold",
                    state === "done" && "border-emerald-500/40 bg-emerald-500/15 text-emerald-300",
                    state === "current" && "border-primary bg-primary text-primary-foreground",
                    state === "pending" && "border-border bg-muted",
                  )}
                >
                  {state === "done" ? <CheckIcon aria-hidden className="size-4" /> : index + 1}
                </span>
                {label}
              </li>
            );
          })}
        </ol>

        {!projectValid && (
          <section className="mx-auto mt-6 max-w-xl rounded-xl border border-border bg-card p-6 shadow-sm">
            <h2 className="text-lg font-semibold">Native EUD 프로젝트</h2>
            <p className="mt-2 text-sm leading-6 text-muted-foreground">
              기존 프로젝트를 열거나 SCX/SCM에서 새 프로젝트를 만들고, 기존 E3S를
              <span className="whitespace-nowrap">가져올 수 있습니다.</span>
            </p>
            {pickError && (
              <p role="alert" className="mt-4 flex gap-2 rounded-md border border-destructive/40 bg-destructive/10 p-3 text-sm text-destructive">
                <CircleAlertIcon aria-hidden className="mt-0.5 size-4 shrink-0" />
                {projectErrorText(pickError)}
              </p>
            )}
            <div className="mt-5 grid gap-2">
              <Button
                className="min-h-11"
                onClick={onPickProject}
                disabled={projectAction !== null}
              >
                {projectAction === "open" ? (
                  <Loader2Icon
                    aria-hidden
                    className="size-4 animate-spin motion-reduce:animate-none"
                  />
                ) : (
                  <FolderOpenIcon aria-hidden className="size-4" />
                )}
                {projectAction === "open"
                  ? "프로젝트 여는 중…"
                  : "기존 Native 프로젝트 열기"}
              </Button>
              <div className="grid grid-cols-2 gap-2">
                <Button
                  variant="outline"
                  className="min-h-11"
                  onClick={onCreateProject}
                  disabled={projectAction !== null}
                >
                  {projectAction === "create" ? "만드는 중…" : "새 프로젝트 만들기"}
                </Button>
                <Button
                  variant="outline"
                  className="min-h-11"
                  onClick={onImportE3s}
                  disabled={projectAction !== null}
                >
                  {projectAction === "import" ? "가져오는 중…" : "E3S 가져오기"}
                </Button>
              </div>
            </div>
          </section>
        )}
        {projectValid && !euddraftValid && (
          <section className="mx-auto mt-6 max-w-xl rounded-xl border border-border bg-card p-6 shadow-sm">
            <h2 className="text-lg font-semibold">euddraft 준비</h2>
            <p className="mt-2 text-sm leading-6 text-muted-foreground">
              {hasConfiguredEuddraft
                ? "설정된 euddraft 경로를 찾을 수 없습니다. 최신 버전을 설치하거나 기존 실행 파일을 다시 선택해 주세요."
                : "euddraft를 직접 선택하지 않으면 최신 버전을 자동으로 내려받아 설치합니다."}
            </p>
            {pickError && (
              <p role="alert" className="mt-4 flex gap-2 rounded-md border border-destructive/40 bg-destructive/10 p-3 text-sm text-destructive">
                <CircleAlertIcon aria-hidden className="mt-0.5 size-4 shrink-0" />
                {projectErrorText(pickError)}
              </p>
            )}
            {(bootstrapActive || (!hasConfiguredEuddraft && view.phase === "error")) && (
              <div className="mt-5 rounded-md border border-border bg-muted/40 p-4">
                <div className="flex items-center gap-2 text-sm">
                  <Loader2Icon
                    aria-hidden
                    className={cn(
                      "size-4",
                      bootstrapActive && "animate-spin motion-reduce:animate-none",
                    )}
                  />
                  {view.label}
                </div>
                {view.pct !== null && (
                  <div
                    role="progressbar"
                    aria-label="euddraft 다운로드"
                    aria-valuemin={0}
                    aria-valuemax={100}
                    aria-valuenow={view.pct}
                    className="mt-3 h-2 overflow-hidden rounded-full bg-muted"
                  >
                    <div
                      className="h-full bg-primary transition-transform"
                      style={{ transform: `translateX(${view.pct - 100}%)` }}
                    />
                  </div>
                )}
              </div>
            )}
            {view.phase === "error" && (
              <p role="alert" className="mt-4 text-sm text-destructive">
                euddraft를 준비하지 못했습니다. 네트워크 연결과 설치 경로를 확인해 주세요.
              </p>
            )}
            <div className="mt-5 grid gap-2 sm:grid-cols-2">
              <Button
                className="min-h-11"
                onClick={() => onPickEuddraft(false)}
                disabled={euddraftAction !== null}
              >
                {euddraftAction === "file" ? (
                  <Loader2Icon aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
                ) : (
                  <FolderOpenIcon aria-hidden className="size-4" />
                )}
                euddraft 파일 선택
              </Button>
              <Button
                variant="outline"
                className="min-h-11"
                onClick={() => onPickEuddraft(true)}
                disabled={euddraftAction !== null}
              >
                {euddraftAction === "folder" ? (
                  <Loader2Icon aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
                ) : (
                  <FolderOpenIcon aria-hidden className="size-4" />
                )}
                euddraft 폴더 선택
              </Button>
            </div>
            {hasConfiguredEuddraft && (
              <Button
                variant="outline"
                className="mt-2 min-h-11 w-full"
                onClick={onInstallEuddraft}
                disabled={euddraftAction !== null || bootstrapActive}
              >
                {euddraftAction === "install" ? (
                  <Loader2Icon aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
                ) : (
                  <Loader2Icon aria-hidden className="size-4" />
                )}
                최신 euddraft 설치
              </Button>
            )}
            {view.phase === "error" && (
              <Button
                variant="ghost"
                className="mt-2 min-h-11 w-full"
                onClick={hasConfiguredEuddraft ? onInstallEuddraft : onRetry}
                disabled={euddraftAction !== null || bootstrapActive}
              >
                다시 시도
              </Button>
            )}
          </section>
        )}

        {projectValid && euddraftValid && !assetsReady && (
          <section className="mx-auto mt-6 max-w-xl rounded-xl border border-border bg-card p-6 shadow-sm">
            <h2 className="text-lg font-semibold">검색 에셋 준비</h2>
            <p className="mt-2 text-sm text-muted-foreground">
              bge-m3 모델과 EUD 문서 인덱스를 내려받아 검증합니다. AI provider 실행 파일과는 분리됩니다.
            </p>
            <div className="mt-5 rounded-md border border-border bg-muted/40 p-4">
              <div className="flex items-center gap-2 text-sm">
                <Loader2Icon aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
                {view.label}
              </div>
              {view.pct !== null && (
                <div
                  role="progressbar"
                  aria-label="에셋 다운로드"
                  aria-valuemin={0}
                  aria-valuemax={100}
                  aria-valuenow={view.pct}
                  className="mt-3 h-2 overflow-hidden rounded-full bg-muted"
                >
                  <div
                    className="h-full bg-primary transition-transform"
                    style={{ transform: `translateX(${view.pct - 100}%)` }}
                  />
                </div>
              )}
            </div>
            {(error || view.phase === "error") && (
              <div className="mt-4">
                <p role="alert" className="text-sm text-destructive">
                  에셋 준비를 완료하지 못했습니다. 네트워크 연결을 확인해 주세요.
                </p>
                <Button variant="outline" className="mt-3 min-h-11" onClick={onRetry}>
                  다시 시도
                </Button>
              </div>
            )}
          </section>
        )}

        {projectValid && euddraftValid && assetsReady && (
          <section className="mx-auto mt-6 max-w-3xl">
            <div className="rounded-2xl border border-border bg-card/70 p-5 shadow-sm sm:p-6">
              <div className="flex items-start gap-3">
                <span className="flex size-11 shrink-0 items-center justify-center rounded-xl bg-primary/10 text-primary">
                  <BotIcon aria-hidden className="size-5" />
                </span>
                <div className="min-w-0">
                  <h2 className="text-lg font-semibold">사용할 AI 제공자 선택</h2>
                  <p className="mt-1 text-sm leading-6 text-muted-foreground">
                    먼저 기본 제공자 하나를 고르세요. 선택한 제공자의 설치·로그인만 다음에 표시됩니다.
                  </p>
                </div>
              </div>

              <label className="mt-5 grid gap-2">
                <span className="text-sm font-medium text-foreground">
                  기본 AI 제공자
                </span>
                <select
                  aria-label="기본 AI 제공자 선택"
                  className="min-h-16 w-full rounded-xl border border-input bg-background px-4 text-base font-semibold shadow-sm outline-none transition-colors hover:border-primary/50 focus:border-primary focus:ring-2 focus:ring-primary/25 disabled:cursor-not-allowed disabled:opacity-50"
                  value={selectedProvider}
                  disabled={busyProvider !== undefined}
                  onChange={(event) => {
                    const provider = event.target.value as ProviderId;
                    setSelectedProvider(provider);
                    void onSelectProvider(provider);
                  }}
                >
                  <option value="" disabled>
                    AI 제공자를 선택하세요
                  </option>
                  {providers.map((status) => (
                    <option key={status.provider} value={status.provider}>
                      {PROVIDER_LABELS[status.provider]} ·{" "}
                      {AVAILABILITY_LABELS[status.availability]}
                    </option>
                  ))}
                </select>
              </label>

              {!selectedStatus && (
                <p className="mt-4 rounded-xl border border-dashed border-border px-4 py-5 text-center text-sm text-muted-foreground">
                  선택 전에는 로그인이나 API 키 입력을 표시하지 않습니다.
                </p>
              )}
            </div>

            {selectedStatus && selectedProvider && (
              <div className="mt-5">
                <div className="mb-3 flex items-center justify-between gap-3">
                  <div>
                    <h2 className="text-base font-semibold">선택 제공자 연결</h2>
                    <p className="mt-1 text-sm text-muted-foreground">
                      {PROVIDER_LABELS[selectedStatus.provider]}만 연결하면 시작할 수 있습니다.
                    </p>
                  </div>
                  {selectedConnected && (
                    <span className="rounded-full border border-emerald-500/40 bg-emerald-500/10 px-3 py-1.5 text-sm text-emerald-300">
                      시작 준비 완료
                    </span>
                  )}
                </div>
                <ProviderCard
                  key={selectedStatus.provider}
                  status={selectedStatus}
                  selected
                  models={models[selectedStatus.provider]}
                  selectedModel={selectedModels[selectedStatus.provider]}
                  selectedReasoning={selectedReasoning[selectedStatus.provider]}
                  version={versions[selectedStatus.provider]}
                  channel={channels[selectedStatus.provider]}
                  baseUrl={baseUrls[selectedStatus.provider]}
                  hasApiKey={hasApiKeys[selectedStatus.provider] === true}
                  busy={busyProvider === selectedStatus.provider}
                  loginInProgress={loginPending[selectedStatus.provider] === true}
                  error={providerErrors[selectedStatus.provider]}
                  showDefaultControl={false}
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
              </div>
            )}
          </section>
        )}
      </div>
    </main>
  );
}
