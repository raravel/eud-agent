import { useTranslation } from "react-i18next";
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
  const { t } = useTranslation();
  const rows = [
    [t("providers.prompt.input"), usage.total.inputTokens],
    [t("providers.prompt.cachedInput"), usage.total.cachedInputTokens],
    [t("providers.prompt.output"), usage.total.outputTokens],
    [t("providers.prompt.reasoning"), usage.total.reasoningOutputTokens],
  ] as const;
  return (
    <div className="space-y-2 text-xs">
      <div className="flex items-center justify-between gap-3 font-medium">
        <span>{t("providers.prompt.sessionTotal")}</span>
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
  const { t } = useTranslation();
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
              aria-label={t("providers.prompt.sessionModel")}
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
              <SelectTrigger aria-label={t("providers.prompt.sessionModel")} className="max-w-44">
                <SelectValue placeholder={t("providers.prompt.selectModel")} />
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
              <SelectTrigger aria-label={t("providers.prompt.reasoningStep")} className="max-w-32">
                <SelectValue />
              </SelectTrigger>
              <SelectContent position="popper">
                {settings.provider === "ollama" && (
                  <SelectItem value="__default__">{t("providers.prompt.modelDefault")}</SelectItem>
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
          aria-label={t("providers.prompt.reloadLabel")}
          disabled={busy}
          onClick={onReload}
        >
          <RefreshCwIcon
            aria-hidden
            className={`size-4 ${busy ? "animate-spin motion-reduce:animate-none" : ""}`}
          />
          {busy ? t("providers.prompt.reloading") : t("providers.prompt.reload")}
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
