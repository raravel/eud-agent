import { useEffect, useMemo, useState } from "react";
import {
  DownloadIcon,
  KeyRoundIcon,
  Loader2Icon,
  LogInIcon,
  LogOutIcon,
  RefreshCwIcon,
  SaveIcon,
  UploadIcon,
  XIcon,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  RadioGroup,
  RadioGroupItem,
} from "@/components/ui/radio-group";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { cn } from "@/lib/utils";
import {
  AVAILABILITY_LABELS,
  PROVIDER_DESCRIPTIONS,
  PROVIDER_LABELS,
  REASONING_LABELS,
  providerErrorCopy,
} from "@/providers/providerCopy";
import type {
  ProviderAvailability,
  ProviderId,
  ProviderModel,
  ProviderStatus,
  ReasoningLevel,
  ReasoningSelection,
} from "@/providers/types";

const MODEL_DEFAULT_REASONING = "__model-default__";

export interface ProviderCardProps {
  status: ProviderStatus;
  selected: boolean;
  models?: ProviderModel[];
  selectedModel?: string;
  selectedReasoning?: ReasoningSelection;
  version?: string | null;
  channel?: string | null;
  baseUrl?: string | null;
  hasApiKey?: boolean;
  busy?: boolean;
  loginInProgress?: boolean;
  error?: string;
  allowUnreadyDefault?: boolean;
  showDefaultControl?: boolean;
  onSelectDefault(provider: ProviderId): Promise<void> | void;
  onInstall(provider: ProviderId): Promise<void> | void;
  onLogin(provider: ProviderId): Promise<void> | void;
  onLoginCancel(provider: ProviderId): Promise<void> | void;
  onImport(provider: ProviderId): Promise<void> | void;
  onApiKey(provider: ProviderId, key: string): Promise<void> | void;
  onBaseUrl(provider: ProviderId, baseUrl: string): Promise<void> | void;
  onLogout(provider: ProviderId): Promise<void> | void;
  onRefresh(provider: ProviderId): Promise<void> | void;
  onModelChange(
    provider: ProviderId,
    model: string,
    reasoning: ReasoningSelection | undefined,
  ): Promise<void> | void;
}

export function ProviderStatusBadge({
  availability,
  className,
}: {
  availability: ProviderAvailability;
  className?: string;
}) {
  return (
    <span
      className={cn(
        "shrink-0 rounded-full border px-2 py-1 text-xs font-medium",
        availability === "ready"
          ? "border-emerald-500/40 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300"
          : availability === "unavailable"
            ? "border-destructive/40 bg-destructive/10 text-destructive"
            : "border-amber-500/40 bg-amber-500/10 text-amber-700 dark:text-amber-300",
        className,
      )}
    >
      {AVAILABILITY_LABELS[availability]}
    </span>
  );
}

export function ProviderCard({
  status,
  selected,
  models = [],
  selectedModel,
  selectedReasoning,
  version,
  channel,
  busy = false,
  baseUrl,
  hasApiKey = false,
  loginInProgress = false,
  error,
  allowUnreadyDefault = false,
  showDefaultControl = true,
  onSelectDefault,
  onInstall,
  onLogin,
  onLoginCancel,
  onImport,
  onApiKey,
  onBaseUrl,
  onLogout,
  onRefresh,
  onModelChange,
}: ProviderCardProps) {
  const [key, setKey] = useState("");
  const [model, setModel] = useState(selectedModel ?? "");
  const [reasoning, setReasoning] = useState(selectedReasoning?.level ?? "");
  const [endpoint, setEndpoint] = useState(baseUrl ?? "");

  useEffect(() => setModel(selectedModel ?? ""), [selectedModel]);
  useEffect(() => setEndpoint(baseUrl ?? ""), [baseUrl]);
  useEffect(
    () => setReasoning(selectedReasoning?.level ?? ""),
    [selectedReasoning],
  );

  const currentModel = useMemo(
    () => models.find((candidate) => candidate.model === model),
    [model, models],
  );
  const reasoningLevels: ReasoningLevel[] =
    status.provider === "ollama"
      ? ["none", "low", "medium", "high", "max"]
      : currentModel?.capabilities.reasoningLevels ?? [];
  const visibleError = providerErrorCopy(error ?? status.detailCode, status.provider);
  const canSelectDefault =
    allowUnreadyDefault || status.availability === "ready";
  const operationBusy = busy || loginInProgress;
  const apiKeyLabel =
    status.provider === "ollama"
      ? hasApiKey
        ? "새 선택적 API 키"
        : "선택적 API 키"
      : status.availability === "ready"
        ? "새 API 키"
        : "API 키";
  const controlIdPrefix = `provider-${status.provider}`;

  const saveModel = (nextModel: string, nextReasoning?: string) => {
    const descriptor = models.find((candidate) => candidate.model === nextModel);
    const levels = descriptor?.capabilities.reasoningLevels ?? [];
    const normalizedReasoning = levels.includes(
      nextReasoning as (typeof levels)[number],
    )
      ? nextReasoning
      : levels[0];
    setModel(nextModel);
    setReasoning(normalizedReasoning ?? "");
    void onModelChange(
      status.provider,
      nextModel,
      normalizedReasoning ? { level: normalizedReasoning } : undefined,
    );
  };

  return (
    <section
      aria-busy={operationBusy}
      className={cn(
        "min-w-0 rounded-xl border bg-card/70 p-4 shadow-sm transition-colors",
        selected ? "border-primary/60 ring-1 ring-primary/30" : "border-border",
      )}
    >
      <div className="flex min-w-0 items-start justify-between gap-3">
        <div className="min-w-0">
          <h3 className="text-base font-semibold text-foreground">
            {PROVIDER_LABELS[status.provider]}
          </h3>
          <p className="mt-1 text-sm leading-5 text-muted-foreground">
            {PROVIDER_DESCRIPTIONS[status.provider]}
          </p>
          {status.provider === "claude-code" && (version || channel) && (
            <p className="mt-1 text-xs text-muted-foreground">
              CLI {version ?? "버전 확인 필요"}
              {channel ? ` · ${channel}` : ""}
            </p>
          )}
        </div>
        <ProviderStatusBadge availability={status.availability} />
      </div>


      {showDefaultControl && (
        <RadioGroup
          name="default-provider"
          value={selected ? status.provider : ""}
          disabled={operationBusy || !canSelectDefault}
          onValueChange={(value) => {
            if (value === status.provider) {
              void onSelectDefault(status.provider);
            }
          }}
        >
          <label
            htmlFor={`${controlIdPrefix}-default`}
            className="mt-4 flex min-h-11 cursor-pointer items-center gap-3 rounded-md border border-border px-3 py-2 text-sm"
          >
            <RadioGroupItem
              id={`${controlIdPrefix}-default`}
              value={status.provider}
            />
            <span>
              <span className="font-medium text-foreground">기본 제공자</span>
              <span className="ml-2 text-xs text-muted-foreground">
                새 세션에만 적용
              </span>
            </span>
          </label>
        </RadioGroup>
      )}

      {status.provider === "ollama" && (
        <form
          className="mt-4 grid gap-2 sm:grid-cols-[minmax(0,1fr)_auto]"
          onSubmit={(event) => {
            event.preventDefault();
            void onBaseUrl(status.provider, endpoint);
          }}
        >
          <label
            htmlFor={`${controlIdPrefix}-base-url`}
            className="grid min-w-0 gap-1.5 text-sm"
          >
            <span className="font-medium text-foreground">
              OpenAI 호환 Base URL
            </span>
            <Input
              id={`${controlIdPrefix}-base-url`}
              type="url"
              value={endpoint}
              disabled={operationBusy}
              spellCheck={false}
              onChange={(event) => setEndpoint(event.target.value)}
              placeholder="http://localhost:11434/v1"
              className="h-11"
            />
          </label>
          <Button
            type="submit"
            className="min-h-11 self-end"
            disabled={operationBusy || endpoint.trim().length === 0}
          >
            <SaveIcon aria-hidden className="size-4" />
            URL 저장
          </Button>
        </form>
      )}

      {status.availability === "ready" && status.provider === "ollama" && (
        <form
          className="mt-4 grid gap-3 sm:grid-cols-[minmax(0,1fr)_minmax(8rem,0.55fr)_auto]"
          onSubmit={(event) => {
            event.preventDefault();
            const nextModel = model.trim();
            if (!nextModel) return;
            void onModelChange(
              status.provider,
              nextModel,
              reasoning ? { level: reasoning } : undefined,
            );
          }}
        >
          <label
            htmlFor={`${controlIdPrefix}-model`}
            className="grid min-w-0 gap-1.5 text-sm"
          >
            <span className="font-medium text-foreground">기본 모델</span>
            <Input
              id={`${controlIdPrefix}-model`}
              value={model}
              disabled={operationBusy}
              spellCheck={false}
              onChange={(event) => setModel(event.target.value)}
              placeholder="예: qwen3:8b"
              className="h-11"
            />
          </label>
          <div className="grid min-w-0 gap-1.5 text-sm">
            <label
              htmlFor={`${controlIdPrefix}-reasoning`}
              className="font-medium text-foreground"
            >
              추론 강도
            </label>
            <Select
              value={reasoning || MODEL_DEFAULT_REASONING}
              disabled={operationBusy}
              onValueChange={(value) =>
                setReasoning(
                  value === MODEL_DEFAULT_REASONING ? "" : value,
                )
              }
            >
              <SelectTrigger
                id={`${controlIdPrefix}-reasoning`}
                className="h-11 w-full"
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={MODEL_DEFAULT_REASONING}>
                  모델 기본값
                </SelectItem>
                {reasoningLevels.map((level) => (
                  <SelectItem key={level} value={level}>
                    {REASONING_LABELS[level]}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <Button
            type="submit"
            className="min-h-11 self-end"
            disabled={operationBusy || model.trim().length === 0}
          >
            <SaveIcon aria-hidden className="size-4" />
            모델 저장
          </Button>
        </form>
      )}

      {status.availability === "ready" &&
        status.provider !== "ollama" &&
        models.length > 0 && (
        <div className="mt-4 grid gap-3 sm:grid-cols-2">
          <div className="grid gap-1.5 text-sm">
            <label
              htmlFor={`${controlIdPrefix}-model`}
              className="font-medium text-foreground"
            >
              기본 모델
            </label>
            <Select
              value={model || undefined}
              disabled={operationBusy}
              onValueChange={(value) => saveModel(value, reasoning)}
            >
              <SelectTrigger
                id={`${controlIdPrefix}-model`}
                className="h-11 w-full"
              >
                <SelectValue placeholder="모델 선택" />
              </SelectTrigger>
              <SelectContent>
                {models.map((candidate) => (
                  <SelectItem key={candidate.model} value={candidate.model}>
                    {candidate.displayName}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          {reasoningLevels.length > 0 && (
            <div className="grid gap-1.5 text-sm">
              <label
                htmlFor={`${controlIdPrefix}-reasoning`}
                className="font-medium text-foreground"
              >
                추론 강도
              </label>
              <Select
                value={reasoning || undefined}
                disabled={operationBusy || !model}
                onValueChange={(value) => saveModel(model, value)}
              >
                <SelectTrigger
                  id={`${controlIdPrefix}-reasoning`}
                  className="h-11 w-full"
                >
                  <SelectValue placeholder="추론 강도 선택" />
                </SelectTrigger>
                <SelectContent>
                  {reasoningLevels.map((level) => (
                    <SelectItem key={level} value={level}>
                      {REASONING_LABELS[level]}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          )}
        </div>
      )}

      {currentModel?.privacy && (
        <p className="mt-3 rounded-md bg-muted/60 px-3 py-2 text-xs leading-5 text-muted-foreground">
          데이터 학습: {currentModel.privacy.training === "used" ? "사용됨" : currentModel.privacy.training === "not-used" ? "사용 안 함" : "정책 확인 필요"}
          {currentModel.privacy.retentionDays !== undefined &&
            ` · 보존 ${currentModel.privacy.retentionDays}일`}
          <br />
          {currentModel.privacy.detail}
        </p>
      )}

      {(status.provider === "opencode-go" ||
        status.provider === "ollama" ||
        (status.provider === "codex" && status.availability !== "ready")) && (
          <form
            className="mt-4 grid min-w-0 gap-2 sm:grid-cols-[minmax(0,1fr)_auto]"
            onSubmit={async (event) => {
              event.preventDefault();
              const submitted = key;
              try {
                await onApiKey(status.provider, submitted);
              } finally {
                setKey("");
              }
            }}
          >
            <label
              htmlFor={`${controlIdPrefix}-api-key`}
              className="grid min-w-0 gap-1.5 text-sm"
            >
              <span className="font-medium text-foreground">{apiKeyLabel}</span>
              <Input
                id={`${controlIdPrefix}-api-key`}
                type="password"
                autoComplete="off"
                spellCheck={false}
                value={key}
                disabled={operationBusy}
                onChange={(event) => setKey(event.target.value)}
                placeholder={apiKeyLabel}
                className="h-11"
              />
            </label>
            <Button
              type="submit"
              className="min-h-11 self-end"
              disabled={operationBusy || key.trim().length === 0}
            >
              <KeyRoundIcon aria-hidden className="size-4" />
              {status.provider === "ollama"
                ? hasApiKey
                  ? "교체"
                  : "키 저장"
                : status.availability === "ready"
                  ? "교체"
                  : "연결"}
            </Button>
          </form>
        )}

      <div className="mt-4 flex flex-wrap gap-2">
        {status.availability === "needs-install" && status.canInstall && (
          <Button
            className="min-h-11"
            disabled={operationBusy}
            onClick={() => void onInstall(status.provider)}
          >
            <DownloadIcon aria-hidden className="size-4" />
            설치
          </Button>
        )}
        {status.availability === "needs-authentication" &&
          status.provider !== "opencode-go" &&
          !loginInProgress && (
            <Button
              className="min-h-11"
              disabled={operationBusy}
              onClick={() => void onLogin(status.provider)}
            >
              <LogInIcon aria-hidden className="size-4" />
              {status.provider === "antigravity" ? "Google 로그인" : status.provider === "codex" ? "ChatGPT 로그인" : "Claude 로그인"}
            </Button>
          )}
        {loginInProgress && (
          <Button
            className="min-h-11"
            variant="outline"
            onClick={() => void onLoginCancel(status.provider)}
          >
            <XIcon aria-hidden className="size-4" />
            로그인 취소
          </Button>
        )}
        {status.canImport && status.availability !== "ready" && (
          <Button
            className="min-h-11"
            variant="outline"
            disabled={operationBusy}
            onClick={() => void onImport(status.provider)}
          >
            <UploadIcon aria-hidden className="size-4" />
            기존 로그인 가져오기
          </Button>
        )}
        {status.availability === "ready" && (
          <Button
            className="min-h-11"
            variant="outline"
            disabled={operationBusy}
            onClick={() => void onRefresh(status.provider)}
          >
            <RefreshCwIcon aria-hidden className="size-4" />
            새로고침
          </Button>
        )}
        {status.availability === "ready" &&
          (status.provider !== "ollama" || hasApiKey) && (
          <Button
            className="min-h-11"
            variant="destructive"
            disabled={operationBusy}
            onClick={() => void onLogout(status.provider)}
          >
            <LogOutIcon aria-hidden className="size-4" />
            {status.provider === "ollama" ? "API 키 제거" : "로그아웃"}
          </Button>
        )}
        {operationBusy && (
          <span className="flex min-h-11 items-center gap-2 px-2 text-sm text-muted-foreground">
            <Loader2Icon aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
            {loginInProgress ? "로그인 대기 중…" : "처리 중…"}
          </span>
        )}
      </div>

      {visibleError && status.availability !== "ready" && (
        <p role="alert" className="mt-3 text-sm text-destructive">
          {visibleError}
        </p>
      )}
    </section>
  );
}
