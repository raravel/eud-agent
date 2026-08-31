import { useEffect, useState } from "react";
import { BotIcon, CheckIcon, CircleAlertIcon, FolderOpenIcon, Loader2Icon } from "lucide-react";

import { ProviderCard } from "@/components/ProviderCard";
import { Button } from "@/components/ui/button";
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
  invalid_editor_folder:
    "선택한 폴더에서 EUD Editor 3을 찾지 못했습니다. Data\\Lua\\TriggerEditor 폴더가 있는 설치 폴더를 선택해 주세요.",
};

export interface SetupScreenProps {
  editorValid: boolean;
  pickError: string | null;
  onPick(): void;
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

const STEPS = ["에디터 폴더", "에셋 다운로드", "AI 제공자 선택", "선택 제공자 연결"] as const;

export function SetupScreen({
  editorValid,
  pickError,
  onPick,
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
  const currentStep = !editorValid
    ? 0
    : !assetsReady
      ? 1
      : !selectedProvider
        ? 2
        : selectedConnected
          ? 4
          : 3;

  return (
    <main className="min-h-dvh overflow-y-auto bg-background px-4 py-8 text-foreground sm:px-8">
      <div className="mx-auto w-full max-w-5xl">
        <header className="text-center">
          <p className="text-sm font-medium text-primary">eud-agent 시작 설정</p>
          <h1 className="mt-2 text-2xl font-semibold tracking-tight sm:text-3xl">
            작업 환경과 AI 제공자를 연결합니다
          </h1>
          <p className="mx-auto mt-3 max-w-2xl text-sm leading-6 text-muted-foreground">
            선택한 기본 제공자만 시작 조건입니다. 다른 제공자는 지금 연결하지 않아도 되며,
            나중에 설정의 AI 제공자 화면에서 관리할 수 있습니다.
          </p>
        </header>

        <ol className="mt-8 grid gap-2 rounded-xl border border-border bg-card/60 p-3 sm:grid-cols-4">
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

        {!editorValid && (
          <section className="mx-auto mt-6 max-w-xl rounded-xl border border-border bg-card p-6 shadow-sm">
            <h2 className="text-lg font-semibold">EUD Editor 3 설치 폴더</h2>
            <p className="mt-2 text-sm leading-6 text-muted-foreground">
              Data\\Lua\\TriggerEditor를 포함한 EUD Editor 3 루트 폴더를 선택해 주세요.
            </p>
            {pickError && (
              <p role="alert" className="mt-4 flex gap-2 rounded-md border border-destructive/40 bg-destructive/10 p-3 text-sm text-destructive">
                <CircleAlertIcon aria-hidden className="mt-0.5 size-4 shrink-0" />
                {PICK_ERROR_TEXT[pickError] ?? "설치 폴더를 확인하지 못했습니다."}
              </p>
            )}
            <Button className="mt-5 min-h-11" onClick={onPick}>
              <FolderOpenIcon aria-hidden className="size-4" />
              폴더 선택
            </Button>
          </section>
        )}

        {editorValid && !assetsReady && (
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

        {editorValid && assetsReady && (
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
