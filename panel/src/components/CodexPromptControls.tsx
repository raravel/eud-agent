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
import type { CodexModelSettings, ContextUsage } from "@/lib/ipc";

const REASONING_LABELS: Readonly<Record<string, string>> = {
  none: "추론 없음",
  minimal: "추론 최소",
  low: "추론 낮음",
  medium: "추론 보통",
  high: "추론 높음",
  xhigh: "추론 매우 높음",
  ultra: "추론 최고",
};

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
          <div
            key={label}
            className="flex items-center justify-between gap-3 text-muted-foreground"
          >
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

export interface CodexPromptControlsProps {
  settings?: CodexModelSettings | null;
  busy?: boolean;
  disabled?: boolean;
  contextUsage?: ContextUsage | null;
  onChange?(model: string, reasoningEffort: string): void;
  onReload?(): void;
}

export function CodexPromptControls({
  settings,
  busy = false,
  disabled = false,
  contextUsage,
  onChange,
  onReload,
}: CodexPromptControlsProps) {
  const selectedModel = settings?.models.find(
    (model) => model.model === settings.selectedModel,
  );
  const settingsDisabled = disabled || busy || onChange === undefined;

  return (
    <>
      {settings && selectedModel ? (
        <>
          <Select
            value={settings.selectedModel}
            disabled={settingsDisabled}
            onValueChange={(modelId) => {
              const model = settings.models.find(
                (candidate) => candidate.model === modelId,
              );
              if (model) onChange?.(model.model, model.defaultReasoningEffort);
            }}
          >
            <SelectTrigger
              size="sm"
              aria-label="Codex 모델"
              className="max-w-44"
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent position="popper">
              {settings.models.map((model) => (
                <SelectItem
                  key={model.model}
                  value={model.model}
                  title={model.description}
                >
                  {model.displayName}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Select
            value={settings.selectedReasoningEffort}
            disabled={settingsDisabled}
            onValueChange={(reasoningEffort) =>
              onChange?.(settings.selectedModel, reasoningEffort)
            }
          >
            <SelectTrigger
              size="sm"
              aria-label="추론 단계"
              className="max-w-32"
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent position="popper">
              {selectedModel.supportedReasoningEfforts.map((option) => (
                <SelectItem
                  key={option.reasoningEffort}
                  value={option.reasoningEffort}
                  title={option.description}
                >
                  {REASONING_LABELS[option.reasoningEffort] ??
                    option.reasoningEffort}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </>
      ) : onReload ? (
        <PromptInputButton
          type="button"
          aria-label="Codex 모델 다시 불러오기"
          disabled={busy}
          onClick={onReload}
        >
          <RefreshCwIcon
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
