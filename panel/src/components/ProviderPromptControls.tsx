import { useEffect, useState } from "react";

import { RefreshCwIcon } from "lucide-react";

import {
  Context,
  ContextContent,
  ContextContentBody,
  ContextContentHeader,
  ContextTrigger,
} from "@/components/ai-elements/context";
import { PromptInputButton } from "@/components/ai-elements/prompt-input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { ContextUsage, SessionModelSettings } from "@/lib/ipc";
import { PROVIDER_LABELS, REASONING_LABELS } from "@/providers/providerCopy";
import type { ReasoningSelection } from "@/providers/types";

const CONTEXT_TOKEN_FORMATTER = new Intl.NumberFormat("ko-KR", {
  notation: "compact",
  maximumFractionDigits: 1,
});

function ContextUsageDetails({ usage }: { usage: ContextUsage }) {
  const rows = [
    ["입력", usage.total.inputTokens],
    ["캐시 입력", usage.total.cachedInputTokens],
    ["출력", usage.total.outputTokens],
    ["추론", usage.total.reasoningOutputTokens],
  ] as const;
  return (
    <div className="space-y-2 text-xs">
      <div className="flex items-center justify-between gap-3 font-medium">
        <span>세션 누적</span>
        <span className="font-mono tabular-nums">
          {CONTEXT_TOKEN_FORMATTER.format(usage.total.totalTokens)}
        </span>
      </div>
      <dl className="space-y-1.5">
        {rows.map(([label, tokens]) => (
          <div key={label} className="flex items-center justify-between gap-3 text-muted-foreground">
            <dt>{label}</dt>
            <dd className="font-mono tabular-nums">
              {CONTEXT_TOKEN_FORMATTER.format(tokens)}
            </dd>
          </div>
        ))}
      </dl>
    </div>
  );
}

export interface ProviderPromptControlsProps {
  settings?: SessionModelSettings | null;
  busy?: boolean;
  disabled?: boolean;
  contextUsage?: ContextUsage | null;
  onChange?(
    model: string,
    reasoning: ReasoningSelection | undefined,
  ): void;
  onReload?(): void;
}

export function ProviderPromptControls({
  settings,
  busy = false,
  disabled = false,
  contextUsage,
  onChange,
  onReload,
}: ProviderPromptControlsProps) {
  const [modelInput, setModelInput] = useState(settings?.selectedModel ?? "");
  useEffect(() => {
    setModelInput(settings?.selectedModel ?? "");
  }, [settings?.selectedModel]);
  const selectedModel = settings?.models.find(
    (model) => model.model === settings.selectedModel,
  );
  const settingsDisabled = disabled || busy || onChange === undefined;
  const reasoningLevels = selectedModel?.capabilities.reasoningLevels ?? [];

  return (
    <>
      {settings && settings.models.length > 0 ? (
        <>
          <span className="inline-flex min-h-9 items-center rounded-md border border-border bg-muted/50 px-2.5 text-xs font-medium text-muted-foreground">
            {PROVIDER_LABELS[settings.provider]}
          </span>
          {settings.provider === "ollama" ? (
            <input
              aria-label="세션 모델"
              value={modelInput}
              disabled={settingsDisabled}
              spellCheck={false}
              className="min-h-9 max-w-44 rounded-md border border-input bg-background px-2.5 text-xs"
              onChange={(event) => setModelInput(event.target.value)}
              onBlur={() => {
                const model = modelInput.trim();
                if (model && model !== settings.selectedModel) {
                  onChange?.(model, settings.selectedReasoning);
                }
              }}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  const model = modelInput.trim();
                  if (model && model !== settings.selectedModel) {
                    onChange?.(model, settings.selectedReasoning);
                  }
                } else if (event.key === "Escape") {
                  setModelInput(settings.selectedModel);
                }
              }}
            />
          ) : (
            <Select
              value={selectedModel?.model ?? ""}
              disabled={settingsDisabled}
              onValueChange={(modelId) => {
                const model = settings.models.find(
                  (candidate) => candidate.model === modelId,
                );
                if (!model) return;
                const current = settings.selectedReasoning?.level;
                const level = model.capabilities.reasoningLevels.includes(
                  current as (typeof model.capabilities.reasoningLevels)[number],
                )
                  ? current
                  : model.capabilities.reasoningLevels[0];
                onChange?.(model.model, level ? { level } : undefined);
              }}
            >
              <SelectTrigger aria-label="세션 모델" className="max-w-44">
                <SelectValue placeholder="모델 선택" />
              </SelectTrigger>
              <SelectContent position="popper">
                {settings.models.map((model) => (
                  <SelectItem key={model.model} value={model.model} title={model.description}>
                    {model.displayName}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}
          {reasoningLevels.length > 0 && (
            <Select
              value={
                settings.selectedReasoning?.level ??
                (settings.provider === "ollama" ? "__default__" : reasoningLevels[0])
              }
              disabled={settingsDisabled}
              onValueChange={(level) =>
                onChange?.(
                  settings.provider === "ollama"
                    ? modelInput.trim() || settings.selectedModel
                    : settings.selectedModel,
                  level === "__default__" ? undefined : { level },
                )
              }
            >
              <SelectTrigger aria-label="추론 단계" className="max-w-32">
                <SelectValue />
              </SelectTrigger>
              <SelectContent position="popper">
                {settings.provider === "ollama" && (
                  <SelectItem value="__default__">모델 기본값</SelectItem>
                )}
                {reasoningLevels.map((level) => (
                  <SelectItem key={level} value={level}>
                    {REASONING_LABELS[level]}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}
        </>
      ) : onReload ? (
        <PromptInputButton
          type="button"
          aria-label="제공자 모델 다시 불러오기"
          disabled={busy}
          onClick={onReload}
        >
          <RefreshCwIcon
            aria-hidden
            className={`size-4 ${busy ? "animate-spin motion-reduce:animate-none" : ""}`}
          />
          {busy ? "모델 불러오는 중…" : "모델 다시 불러오기"}
        </PromptInputButton>
      ) : null}
      {contextUsage?.modelContextWindow !== null &&
        contextUsage?.modelContextWindow !== undefined &&
        contextUsage.modelContextWindow > 0 && (
          <Context
            usedTokens={Math.max(0, contextUsage.last.totalTokens)}
            maxTokens={contextUsage.modelContextWindow}
          >
            <ContextTrigger />
            <ContextContent side="top" align="end">
              <ContextContentHeader />
              <ContextContentBody>
                <ContextUsageDetails usage={contextUsage} />
              </ContextContentBody>
            </ContextContent>
          </Context>
        )}
    </>
  );
}
