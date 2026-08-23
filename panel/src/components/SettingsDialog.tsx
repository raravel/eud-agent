import { useState } from "react";
import { Bell, Bot, LoaderCircle, RefreshCw, Volume2, X } from "lucide-react";

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
  CodexModelSettings,
  NotificationChannelSettings,
  NotificationEvent,
} from "@/lib/ipc";

export interface SettingsDialogProps {
  open: boolean;
  settings: AppSettings | null;
  busy?: boolean;
  codexSettings: CodexModelSettings | null;
  codexBusy?: boolean;
  onOpenChange(open: boolean): void;
  onSettingsChange(settings: AppSettings): void;
  onReload(): void;
  onCodexReload(): void;
  onPreviewSound(): void;
}
type SettingsCategory = "notifications" | "codex";


interface EventRowProps {
  title: string;
  description: string;
  value: NotificationChannelSettings;
  disabled: boolean;
  onChange(field: keyof NotificationChannelSettings, checked: boolean): void;
}

function EventRow({
  title,
  description,
  value,
  disabled,
  onChange,
}: EventRowProps) {
  return (
    <div className="grid min-h-20 grid-cols-[minmax(0,1fr)_5.5rem_5.5rem] items-center gap-2 border-t border-border px-4 py-3 first:border-t-0">
      <div className="min-w-0 pr-3">
        <p className="text-sm font-medium text-foreground">{title}</p>
        <p className="mt-1 text-xs leading-relaxed text-muted-foreground">
          {description}
        </p>
      </div>
      <Switch
        checked={value.sound}
        disabled={disabled}
        aria-label={`${title} 알림음`}
        className="justify-self-center"
        onCheckedChange={(checked) => onChange("sound", checked)}
      />
      <Switch
        checked={value.osNotification}
        disabled={disabled}
        aria-label={`${title} OS 알림`}
        className="justify-self-center"
        onCheckedChange={(checked) => onChange("osNotification", checked)}
      />
    </div>
  );
}

const EVENT_COPY: Record<
  NotificationEvent,
  { title: string; description: string }
> = {
  agentTurnComplete: {
    title: "에이전트 턴 종료",
    description: "계획·변경사항 검토를 제외한 에이전트 턴이 종료됐을 때",
  },
  askResponseRequired: {
    title: "ASK 응답 필요",
    description: "에이전트가 질문에 대한 사용자 응답을 기다릴 때",
  },
  planApproval: {
    title: "계획 승인 필요",
    description: "새 계획안이나 수정된 계획안이 도착했을 때",
  },
  changesetReview: {
    title: "변경사항 검토 필요",
    description: "적용 또는 되돌리기를 결정할 변경사항이 도착했을 때",
  },
};

export function SettingsDialog({
  open,
  settings,
  codexSettings,
  busy = false,
  codexBusy = false,
  onOpenChange,
  onSettingsChange,
  onReload,
  onCodexReload,
  onPreviewSound,
}: SettingsDialogProps) {
  const [category, setCategory] =
    useState<SettingsCategory>("notifications");
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
        className="grid h-[calc(100vh-4rem)] max-h-[42rem] grid-rows-[auto_minmax(0,1fr)] gap-0 overflow-hidden p-0 sm:max-w-3xl"
      >
        <DialogHeader className="relative border-b border-border px-6 py-5 pr-16 text-left">
          <DialogTitle>설정</DialogTitle>
          <DialogDescription>앱 동작과 알림을 관리합니다.</DialogDescription>
          <DialogClose asChild>
            <Button
              type="button"
              size="icon"
              variant="ghost"
              className="absolute right-4 top-4"
              aria-label="설정 닫기"
            >
              <X className="size-4" aria-hidden="true" />
            </Button>
          </DialogClose>
        </DialogHeader>

        <div className="grid min-h-0 grid-cols-[10.5rem_minmax(0,1fr)] overflow-hidden">
          <nav
            aria-label="설정 범주"
            className="border-r border-border bg-card/40 p-3"
          >
            <Button
              type="button"
              variant={category === "notifications" ? "secondary" : "ghost"}
              className="h-11 w-full justify-start gap-2"
              aria-current={category === "notifications" ? "page" : undefined}
              onClick={() => setCategory("notifications")}
            >
              <Bell className="size-4" aria-hidden="true" />
              알림
            </Button>
            <Button
              type="button"
              variant={category === "codex" ? "secondary" : "ghost"}
              className="mt-1 h-11 w-full justify-start gap-2"
              aria-current={category === "codex" ? "page" : undefined}
              onClick={() => setCategory("codex")}
            >
              <Bot className="size-4" aria-hidden="true" />
              Codex
            </Button>
          </nav>

          <section
            aria-labelledby={
              category === "notifications"
                ? "notification-settings-title"
                : "codex-settings-title"
            }
            className="min-h-0 min-w-0 overflow-y-auto overscroll-contain px-6 py-5"
          >
            {category === "notifications" ? (
              <>
            <div className="flex items-start justify-between gap-4">
              <div>
                <h2
                  id="notification-settings-title"
                  className="text-base font-semibold"
                >
                  알림
                </h2>
                <p className="mt-1.5 text-sm text-muted-foreground">
                  사용자 확인이 필요한 순간을 놓치지 않도록 알려드립니다.
                </p>
              </div>
              {busy && (
                <span
                  role="status"
                  className="flex shrink-0 items-center gap-1.5 text-xs text-muted-foreground"
                >
                  <LoaderCircle
                    className="size-3.5 animate-spin motion-reduce:animate-none"
                    aria-hidden="true"
                  />
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
                  {(Object.keys(EVENT_COPY) as NotificationEvent[]).map(
                    (event) => (
                      <EventRow
                        key={event}
                        {...EVENT_COPY[event]}
                        value={settings.notifications[event]}
                        disabled={busy}
                        onChange={(field, checked) =>
                          updateEvent(event, field, checked)
                        }
                      />
                    ),
                  )}
                </div>

                <div className="mt-4 flex items-center justify-between gap-4 rounded-xl border border-border bg-card/40 px-4 py-3.5">
                  <div className="flex min-w-0 items-center gap-3">
                    <span
                      aria-hidden="true"
                      className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary"
                    >
                      <Volume2 className="size-4" />
                    </span>
                    <div className="min-w-0">
                      <p className="text-sm font-medium">기본 알림음</p>
                      <p className="mt-0.5 text-xs text-muted-foreground">
                        Windows 기본 알림음을 사용합니다.
                      </p>
                    </div>
                  </div>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    className="shrink-0"
                    onClick={onPreviewSound}
                  >
                    소리 미리듣기
                  </Button>
                </div>

                <p className="mt-4 text-xs leading-relaxed text-muted-foreground">
                  OS 알림은 EUD 에이전트 창이 포커스되어 있지 않을 때만
                  표시됩니다. 설정은 변경 즉시 저장됩니다.
                </p>
              </>
            ) : (
              <div className="mt-5 flex min-h-52 flex-col items-center justify-center rounded-xl border border-dashed border-border text-center">
                <p className="text-sm text-muted-foreground">
                  알림 설정을 불러오지 못했습니다.
                </p>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  className="mt-3 gap-1.5"
                  disabled={busy}
                  onClick={onReload}
                >
                  <RefreshCw className="size-3.5" aria-hidden="true" />
                  다시 시도
                </Button>
              </div>
            )}
              </>
            ) : (
              <>
                <div className="flex items-start justify-between gap-4">
                  <div>
                    <h2 id="codex-settings-title" className="text-base font-semibold">
                      Codex 컨텍스트
                    </h2>
                    <p className="mt-1.5 text-sm text-muted-foreground">
                      모델별로 1M 컨텍스트와 네이티브 자동 압축을 설정합니다.
                    </p>
                  </div>
                  {(busy || codexBusy) && (
                    <span
                      role="status"
                      className="flex shrink-0 items-center gap-1.5 text-xs text-muted-foreground"
                    >
                      <LoaderCircle
                        className="size-3.5 animate-spin motion-reduce:animate-none"
                        aria-hidden="true"
                      />
                      {busy ? "저장 중…" : "불러오는 중…"}
                    </span>
                  )}
                </div>

                {settings && codexSettings ? (
                  <>
                    <div className="mt-5 overflow-hidden rounded-xl border border-border bg-card/40">
                      {codexSettings.models.map((model, index) => {
                        const enabled =
                          settings.codexLargeContextModels.includes(model.model);
                        const selected =
                          codexSettings.selectedModel === model.model;
                        return (
                          <div
                            key={model.model}
                            className={`grid min-h-20 grid-cols-[minmax(0,1fr)_5.5rem] items-center gap-3 px-4 py-3 ${
                              index === 0 ? "" : "border-t border-border"
                            }`}
                          >
                            <div className="min-w-0 pr-3">
                              <div className="flex flex-wrap items-center gap-2">
                                <p className="text-sm font-medium text-foreground">
                                  {model.displayName}
                                </p>
                                {selected && (
                                  <span className="rounded-full bg-primary/10 px-2 py-0.5 text-[11px] font-medium text-primary">
                                    현재 모델
                                  </span>
                                )}
                              </div>
                              <p className="mt-1 break-all font-mono text-xs text-muted-foreground">
                                {model.model}
                              </p>
                            </div>
                            <div className="flex flex-col items-center gap-1.5">
                              <Switch
                                checked={enabled}
                                disabled={busy}
                                aria-label={`${model.displayName} 1M 컨텍스트`}
                                onCheckedChange={(checked) =>
                                  updateLargeContext(model.model, checked)
                                }
                              />
                              <span className="text-[11px] text-muted-foreground">
                                1M
                              </span>
                            </div>
                          </div>
                        );
                      })}
                    </div>
                    <p className="mt-4 text-xs leading-relaxed text-muted-foreground">
                      켜면 해당 모델에 1,000,000 토큰 컨텍스트와 900,000 토큰
                      자동 압축 임계치를 적용합니다. Codex가 더 작은 한도로 제한하면
                      보고된 컨텍스트로 동작하고 대화에 한 번 안내합니다.
                    </p>
                  </>
                ) : (
                  <div className="mt-5 flex min-h-52 flex-col items-center justify-center rounded-xl border border-dashed border-border text-center">
                    <p className="text-sm text-muted-foreground">
                      Codex 모델 설정을 불러오지 못했습니다.
                    </p>
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      className="mt-3 h-11 gap-1.5"
                      disabled={busy || codexBusy}
                      onClick={onCodexReload}
                    >
                      <RefreshCw className="size-3.5" aria-hidden="true" />
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
