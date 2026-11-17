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
  Globe,
  Hammer,
  KeyRound,
  Languages,
  LoaderCircle,
  Plus,
  RefreshCw,
  Smartphone,
  Volume2,
  X,
} from "lucide-react";
import {
  ProviderCard,
  ProviderStatusBadge,
} from "@/components/ProviderCard";
import {
  BrowserSettingsPanel,
  type BrowserSettingsActions,
} from "@/components/BrowserSettingsPanel";
import {
  RemoteSettingsPanel,
  type RemoteSettingsActions,
} from "@/components/RemoteSettingsPanel";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { useTranslation } from "react-i18next";
import { LanguageSettingsPanel } from "@/components/LanguageSettingsPanel";
import {
  MapTestSettingsPanel,
  type MapTestSettingsActions,
} from "@/components/MapTestSettingsPanel";
import type {
  AppSettings,
  EuddraftSettings,
  JevErrorCode,
  JevSettings,
  NotificationChannelSettings,
  NotificationEvent,
} from "@/lib/ipc";
import i18n from "@/lib/i18n";
import { isMacOS } from "@/lib/platform";
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
  onProjectImportE2(): void;
  onProjectExport(): void;
  onEuddraftCheck(): void;
  onEuddraftUpdate(): void;
  onScmdraftPick(): Promise<void> | void;
  onStarcraftPick?(): Promise<void> | void;
  /** Jev search-rerank settings; the section is shown only when provided. */
  jev?: JevSettingsActions;
  /** Map runtime tests (Settings → 컴파일); the section is shown only when provided. */
  mapTest?: MapTestSettingsActions;
  /** Remote access (phone over Tailscale); the 원격 접속 category is shown only when provided. */
  remote?: RemoteSettingsActions;
  /** The built-in browser; the 브라우저 category is shown only when provided. */
  browser?: BrowserSettingsActions;
}

/** Backend calls behind the Jev section (see `jevSettingsGet` & co. in ipc). */
export interface JevSettingsActions {
  load(): Promise<JevSettings>;
  save(enabled: boolean, apiKey?: string): Promise<JevSettings>;
  clearKey(): Promise<JevSettings>;
}

export type SettingsCategory =
  | "project"
  | "compile"
  | "providers"
  | "notifications"
  | "remote"
  | "browser"
  | "language";

const NOTIFICATION_EVENTS: readonly NotificationEvent[] = [
  "agentTurnComplete",
  "askResponseRequired",
  "planApproval",
  "reviewRequired",
];

function eventCopy(event: NotificationEvent): { title: string; description: string } {
  return {
    title: i18n.t(`settings.events.${event}.title`),
    description: i18n.t(`settings.events.${event}.description`),
  };
}

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
  const { t } = useTranslation();
  return (
    <div className="animate-in fade-in duration-200 motion-reduce:animate-none">
      <div className="flex items-start justify-between gap-4">
        <div>
          <h2 className="text-base font-semibold">{t("settings.euddraft.title")}</h2>
          <p className="mt-1.5 text-sm leading-6 text-muted-foreground">
            {t("settings.euddraft.description")}
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
              ? t("settings.euddraft.updating")
              : busy === "check"
                ? t("settings.euddraft.checking")
                : t("settings.euddraft.loading")}
          </span>
        )}
      </div>

      {status ? (
        <>
          <dl className="mt-5 overflow-hidden rounded-xl border border-border bg-card/40">
            <div className="grid gap-1.5 border-b border-border px-4 py-3.5">
              <dt className="text-xs font-medium text-muted-foreground">
                {t("settings.euddraft.path")}
              </dt>
              <dd
                className="break-all font-mono text-sm text-foreground"
                title={formatPathForDisplay(status.path)}
              >
                {formatPathForDisplay(status.path) || t("settings.euddraft.notSet")}
              </dd>
            </div>
            <div className="grid grid-cols-2 gap-4 px-4 py-3.5">
              <div>
                <dt className="text-xs font-medium text-muted-foreground">
                  {t("settings.euddraft.currentVersion")}
                </dt>
                <dd className="mt-1 text-sm font-semibold">
                  {status.installedVersion ?? t("settings.euddraft.noVersion")}
                </dd>
              </div>
              <div>
                <dt className="text-xs font-medium text-muted-foreground">
                  {t("settings.euddraft.latestVersion")}
                </dt>
                <dd className="mt-1 text-sm font-semibold">
                  {status.latestVersion ?? t("settings.euddraft.notChecked")}
                </dd>
              </div>
            </div>
          </dl>

          {!status.valid && (
            <p
              role="alert"
              className="mt-3 rounded-lg border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm text-destructive"
            >
              {t("settings.euddraft.missing")}
            </p>
          )}
          {status.valid && !status.managed && (
            <p className="mt-3 rounded-lg border border-border bg-muted/30 px-4 py-3 text-xs leading-5 text-muted-foreground">
              {t("settings.euddraft.manual")}
            </p>
          )}
          {status.latestVersion &&
            status.managed &&
            !status.updateAvailable && (
              <p className="mt-3 flex items-center gap-2 rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-4 py-3 text-sm text-emerald-700 dark:text-emerald-300">
                <CheckCircle2 aria-hidden className="size-4 shrink-0" />
                {t("settings.euddraft.upToDate")}
              </p>
            )}
          {status.updateAvailable && (
            <p className="mt-3 rounded-lg border border-amber-500/30 bg-amber-500/10 px-4 py-3 text-sm text-amber-800 dark:text-amber-200">
              {t("settings.euddraft.available", { version: status.latestVersion })}
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
              {t("settings.euddraft.check")}
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
                    ? t("settings.euddraft.update")
                    : t("settings.euddraft.install")}
                </Button>
              )}
          </div>
        </>
      ) : (
        <div className="mt-5 flex min-h-52 flex-col items-center justify-center rounded-xl border border-dashed border-border text-center">
          <p className="text-sm text-muted-foreground">
            {t("settings.euddraft.loadFailed")}
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
            {t("settings.retry")}
          </Button>
        </div>
      )}
    </div>
  );
}

const JEV_ERROR_CODES: readonly JevErrorCode[] = [
  "jev_key_invalid",
  "jev_unreachable",
  "jev_unavailable",
  "jev_key_missing",
  "jev_key_store",
];

function jevErrorMessage(error: unknown): string {
  const code =
    typeof error === "string" ? error : error instanceof Error ? error.message : "";
  return (JEV_ERROR_CODES as readonly string[]).includes(code)
    ? i18n.t(`settings.jev.error.${code}`)
    : i18n.t("settings.jev.saveFailed");
}

function JevSettingsPanel({
  actions,
  disabled,
}: {
  actions: JevSettingsActions;
  disabled: boolean;
}) {
  const { t } = useTranslation();
  const [saved, setSaved] = useState<JevSettings | null>(null);
  const [enabled, setEnabled] = useState(false);
  const [apiKey, setApiKey] = useState("");
  const [pending, setPending] = useState<"load" | "save" | "clear" | null>(
    "load",
  );
  const [error, setError] = useState<string>();

  useEffect(() => {
    let active = true;
    actions
      .load()
      .then((value) => {
        if (!active) return;
        setSaved(value);
        setEnabled(value.enabled);
      })
      .catch(() => {
        if (active) setError(i18n.t("settings.jev.loadFailed"));
      })
      .finally(() => {
        if (active) setPending(null);
      });
    return () => {
      active = false;
    };
  }, [actions]);

  /** `revertTo`: switch state to restore when the call fails. */
  const run = async (
    kind: "save" | "clear",
    call: () => Promise<JevSettings>,
    revertTo?: boolean,
  ) => {
    setPending(kind);
    setError(undefined);
    try {
      const value = await call();
      setSaved(value);
      setEnabled(value.enabled);
      setApiKey("");
    } catch (caught) {
      setError(jevErrorMessage(caught));
      if (revertTo !== undefined) setEnabled(revertTo);
    } finally {
      setPending(null);
    }
  };

  const toggle = (checked: boolean) => {
    if (!saved) return;
    setEnabled(checked);
    setError(undefined);
    // Off saves at once and keeps the stored key; on saves at once when a key
    // is stored, otherwise waits for the key form so the key is validated
    // first. A failed toggle shows the backend's actual state again.
    if (checked !== saved.enabled && (!checked || saved.hasKey)) {
      void run("save", () => actions.save(checked), saved.enabled);
    }
  };

  const busy = disabled || pending !== null;
  const trimmedKey = apiKey.trim();

  return (
    <section
      aria-labelledby="settings-jev-heading"
      className="mt-4 rounded-xl border border-border bg-card/40 px-4 py-3.5"
    >
      <div className="flex min-h-11 items-center justify-between gap-4">
        <span className="min-w-0">
          <span
            id="settings-jev-heading"
            className="block text-sm font-medium text-foreground"
          >
            {t("settings.jev.title")}
          </span>
          <span className="mt-0.5 block text-xs leading-5 text-muted-foreground">
            {t("settings.jev.description")}
          </span>
        </span>
        <Switch
          checked={enabled}
          disabled={busy || saved === null}
          aria-label={t("settings.jev.title")}
          onCheckedChange={toggle}
        />
      </div>
      {enabled && saved && (
        <form
          className="mt-3 grid min-w-0 gap-2 sm:grid-cols-[minmax(0,1fr)_auto]"
          onSubmit={(event) => {
            event.preventDefault();
            void run("save", () =>
              actions.save(true, trimmedKey.length > 0 ? trimmedKey : undefined),
            );
          }}
        >
          <label htmlFor="settings-jev-api-key" className="grid min-w-0 gap-1.5 text-sm">
            <span className="font-medium text-foreground">{t("settings.jev.apiKey")}</span>
            <Input
              id="settings-jev-api-key"
              type="password"
              autoComplete="off"
              spellCheck={false}
              value={apiKey}
              disabled={busy}
              onChange={(event) => setApiKey(event.target.value)}
              placeholder={
                saved.hasKey ? t("settings.jev.keySaved") : t("settings.jev.keyEnter")
              }
              className="h-11"
            />
          </label>
          <Button
            type="submit"
            className="min-h-11 self-end"
            disabled={busy || (!saved.hasKey && trimmedKey.length === 0)}
          >
            {pending === "save" ? (
              <LoaderCircle
                aria-hidden
                className="size-4 animate-spin motion-reduce:animate-none"
              />
            ) : (
              <KeyRound aria-hidden className="size-4" />
            )}
            {pending === "save" ? t("settings.euddraft.checking") : t("settings.jev.save")}
          </Button>
        </form>
      )}
      {saved?.hasKey && (
        <div className="mt-3 flex justify-end">
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-11 text-muted-foreground"
            disabled={busy}
            onClick={() => void run("clear", actions.clearKey)}
          >
            {t("settings.jev.deleteKey")}
          </Button>
        </div>
      )}
      {error && (
        <p
          role="alert"
          className="mt-3 rounded-lg border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm text-destructive"
        >
          {error}
        </p>
      )}
    </section>
  );
}

interface ScmdraftSettingsPanelProps {
  path: string;
  busy: boolean;
  error?: string;
  onPick(): Promise<void> | void;
}

function ScmdraftSettingsPanel({ path, busy, error, onPick }: ScmdraftSettingsPanelProps) {
  const { t } = useTranslation();
  return (
    <section aria-labelledby="settings-scmdraft-heading" className="mt-8">
      <h2 id="settings-scmdraft-heading" className="text-base font-semibold">SCMDraft 2</h2>
      <p className="mt-1.5 text-sm leading-6 text-muted-foreground">
        {t("settings.scmdraft.description")}
      </p>
      <dl className="mt-5 overflow-hidden rounded-xl border border-border bg-card/40">
        <div className="grid gap-1.5 px-4 py-3.5">
          <dt className="text-xs font-medium text-muted-foreground">{t("settings.scmdraft.executable")}</dt>
          <dd
            className="break-all font-mono text-sm text-foreground"
            title={formatPathForDisplay(path)}
          >
            {formatPathForDisplay(path) || t("settings.scmdraft.notSet")}
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
          {busy ? t("settings.picking") : t("settings.scmdraft.pick")}
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
  const { t } = useTranslation();
  const path = availability?.available ? availability.path : "";
  return (
    <section aria-labelledby="settings-starcraft-heading" className="mt-8">
      <h2 id="settings-starcraft-heading" className="text-base font-semibold">StarCraft</h2>
      <p className="mt-1.5 text-sm leading-6 text-muted-foreground">
        {t("settings.starcraft.description")}
      </p>
      <dl className="mt-5 overflow-hidden rounded-xl border border-border bg-card/40">
        <div className="grid gap-1.5 px-4 py-3.5">
          <dt className="text-xs font-medium text-muted-foreground">{t("settings.starcraft.folder")}</dt>
          <dd
            className="break-all font-mono text-sm text-foreground"
            title={formatPathForDisplay(path)}
          >
            {formatPathForDisplay(path) || (availability ? t("settings.starcraft.notFound") : t("settings.starcraft.checking"))}
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
          {busy ? t("settings.picking") : t("settings.starcraft.pick")}
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
  onProviderApiKey,
  onProviderBaseUrl,
  onProviderLogout,
  onProviderRefresh,
  onProviderModelChange,
  onProjectOpen,
  onProjectCreate,
  onProjectImport,
  onProjectImportE2,
  onProjectExport,
  onEuddraftCheck,
  onEuddraftUpdate,
  onScmdraftPick,
  onStarcraftPick,
  jev,
  mapTest,
  remote,
  browser,
}: SettingsDialogProps) {
  const { t } = useTranslation();
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
          <DialogTitle>{t("settings.dialog.title")}</DialogTitle>
          <DialogDescription>
            {t("settings.dialog.description")}
          </DialogDescription>
          <DialogClose asChild>
            <Button
              type="button"
              size="icon"
              variant="ghost"
              className="absolute right-3 top-3 size-11 sm:right-4 sm:top-4"
              aria-label={t("settings.close")}
            >
              <X aria-hidden className="size-4" />
            </Button>
          </DialogClose>
        </DialogHeader>

        <div className="grid min-h-0 grid-rows-[auto_minmax(0,1fr)] overflow-hidden sm:grid-cols-[10.5rem_minmax(0,1fr)] sm:grid-rows-1">
          <nav
            aria-label={t("settings.categoriesLabel")}
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
              {t("settings.category.project")}
            </Button>
            <Button
              type="button"
              variant={category === "compile" ? "secondary" : "ghost"}
              className="h-11 flex-1 justify-start gap-2 sm:mt-1 sm:w-full"
              aria-current={category === "compile" ? "page" : undefined}
              onClick={() => selectCategory("compile")}
            >
              <Hammer aria-hidden className="size-4" />
              {t("settings.category.compile")}
            </Button>
            <Button
              type="button"
              variant={category === "providers" ? "secondary" : "ghost"}
              className="h-11 flex-1 justify-start gap-2 sm:mt-1 sm:w-full"
              aria-current={category === "providers" ? "page" : undefined}
              onClick={() => selectCategory("providers")}
            >
              <Bot aria-hidden className="size-4" />
              {t("settings.category.providers")}
            </Button>
            <Button
              type="button"
              variant={category === "notifications" ? "secondary" : "ghost"}
              className="h-11 flex-1 justify-start gap-2 sm:mt-1 sm:w-full"
              aria-current={category === "notifications" ? "page" : undefined}
              onClick={() => selectCategory("notifications")}
            >
              <Bell aria-hidden className="size-4" />
              {t("settings.category.notifications")}
            </Button>
            {remote && (
              <Button
                type="button"
                variant={category === "remote" ? "secondary" : "ghost"}
                className="h-11 flex-1 justify-start gap-2 sm:mt-1 sm:w-full"
                aria-current={category === "remote" ? "page" : undefined}
                onClick={() => selectCategory("remote")}
              >
                <Smartphone aria-hidden className="size-4" />
                {t("settings.category.remote")}
              </Button>
            )}
            <Button
              type="button"
              variant={category === "language" ? "secondary" : "ghost"}
              className="h-11 flex-1 justify-start gap-2 sm:mt-1 sm:w-full"
              aria-current={category === "language" ? "page" : undefined}
              onClick={() => selectCategory("language")}
            >
              <Languages aria-hidden className="size-4" />
              {t("settings.category.language")}
            </Button>
            {browser && (
              <Button
                type="button"
                variant={category === "browser" ? "secondary" : "ghost"}
                className="h-11 flex-1 justify-start gap-2 sm:mt-1 sm:w-full"
                aria-current={category === "browser" ? "page" : undefined}
                onClick={() => selectCategory("browser")}
              >
                <Globe aria-hidden className="size-4" />
                {t("settings.category.browser")}
              </Button>
            )}
          </nav>

          <section className="min-h-0 min-w-0 overflow-y-auto overscroll-contain px-4 py-4 sm:px-6 sm:py-5">
            {category === "language" ? (
              <LanguageSettingsPanel />
            ) : category === "remote" && remote ? (
              <RemoteSettingsPanel actions={remote} disabled={busy} />
            ) : category === "browser" && browser ? (
              <BrowserSettingsPanel actions={browser} supported={!isMacOS()} disabled={busy} />
            ) : category === "project" ? (
              <div className="animate-in fade-in duration-200 motion-reduce:animate-none">
                <div className="flex items-start justify-between gap-4">
                  <div>
                    <h2 className="text-base font-semibold">{t("settings.project.title")}</h2>
                    <p className="mt-1.5 text-sm leading-6 text-muted-foreground">
                      {t("settings.project.description")}
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
                      {t("settings.working")}
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
                    {t("settings.project.open")}
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
                      {t("settings.project.new")}
                    </Button>
                    <Button
                      type="button"
                      variant="outline"
                      className="min-h-11 justify-start"
                      disabled={projectBusy !== null}
                      onClick={onProjectImport}
                    >
                      <FileInput aria-hidden className="size-4" />
                      {t("settings.project.importE3s")}
                    </Button>
                    <Button
                      type="button"
                      variant="outline"
                      className="min-h-11 justify-start"
                      disabled={projectBusy !== null}
                      onClick={onProjectImportE2}
                    >
                      <FileInput aria-hidden className="size-4" />
                      {t("settings.project.importE2")}
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
                    {t("settings.project.exportE3s")}
                  </Button>
                </div>
                <p className="mt-4 text-xs leading-relaxed text-muted-foreground">
                  {t("settings.project.exportNote")}
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
                {mapTest && (
                  <MapTestSettingsPanel actions={mapTest} disabled={busy} />
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
                    {t("settings.providers.all")}
                  </Button>
                  <div className="mt-2 flex items-start justify-between gap-4">
                    <div>
                      <h2
                        ref={detailHeadingRef}
                        tabIndex={-1}
                        className="text-base font-semibold outline-none"
                      >
                        {t("settings.providers.settingsTitle", { name: PROVIDER_LABELS[selectedProviderStatus.provider] })}
                      </h2>
                      <p className="mt-1.5 text-sm leading-6 text-muted-foreground">
                        {t("settings.providers.detailDescription")}
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
                        {t("settings.working")}
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
                            {t("settings.providers.codexLargeContext")}
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
                                  aria-label={t("settings.providers.largeContextLabel", { name: model.displayName })}
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
                      {t("settings.providers.autosaved")}
                    </p>
                    <Button
                      type="button"
                      className="h-11 sm:min-w-28"
                      onClick={() => setSelectedProvider(undefined)}
                    >
                      <CheckCircle2 aria-hidden className="size-4" />
                      {t("settings.providers.done")}
                    </Button>
                  </div>
                </div>
              ) : (
                <div className="animate-in fade-in duration-200 motion-reduce:animate-none">
                  <div className="flex items-start justify-between gap-4">
                    <div>
                      <h2 className="text-base font-semibold">{t("settings.providers.title")}</h2>
                      <p className="mt-1.5 text-sm leading-6 text-muted-foreground">
                        {t("settings.providers.pick")}
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
                        {t("settings.working")}
                      </span>
                    )}
                  </div>
                  <ul
                    aria-label={t("settings.providers.listLabel")}
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
                          aria-label={t(
                            status.selectedAsDefault
                              ? "settings.providers.openLabelDefault"
                              : "settings.providers.openLabel",
                            {
                              name: PROVIDER_LABELS[status.provider],
                              status: AVAILABILITY_LABELS[status.availability],
                            },
                          )}
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
                                  {t("settings.providers.default")}
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
                      {t("settings.providers.defaultNote")}
                    </p>
                  </div>
                  {settings && (
                    <label className="mt-4 flex min-h-11 items-center justify-between gap-4 rounded-xl border border-border bg-card/40 px-4 py-3.5">
                      <span className="min-w-0">
                        <span className="block text-sm font-medium text-foreground">
                          {t("settings.providers.deepPlanning")}
                        </span>
                        <span className="mt-0.5 block text-xs leading-5 text-muted-foreground">
                          {t("settings.providers.deepPlanningNote")}
                        </span>
                      </span>
                      <Switch
                        checked={settings.deepPlanning}
                        disabled={busy}
                        aria-label={t("settings.providers.deepPlanning")}
                        onCheckedChange={updateDeepPlanning}
                      />
                    </label>
                  )}
                  {jev && <JevSettingsPanel actions={jev} disabled={busy} />}
                </div>
              )
            ) : (
              <>
                <div className="flex items-start justify-between gap-4">
                  <div>
                    <h2 className="text-base font-semibold">{t("settings.notifications.title")}</h2>
                    <p className="mt-1.5 text-sm text-muted-foreground">
                      {t("settings.notifications.description")}
                    </p>
                  </div>
                  {busy && (
                    <span role="status" className="flex shrink-0 items-center gap-1.5 text-xs text-muted-foreground">
                      <LoaderCircle aria-hidden className="size-3.5 animate-spin motion-reduce:animate-none" />
                      {t("settings.notifications.saving")}
                    </span>
                  )}
                </div>
                {settings ? (
                  <>
                    <div className="mt-5 overflow-hidden rounded-xl border border-border bg-card/40">
                      <div className="grid grid-cols-[minmax(0,1fr)_5.5rem_5.5rem] gap-2 border-b border-border px-4 py-2.5 text-xs font-medium text-muted-foreground">
                        <span>{t("settings.notifications.when")}</span>
                        <span className="text-center">{t("settings.notifications.sound")}</span>
                        <span className="text-center">{t("settings.notifications.os")}</span>
                      </div>
                      {NOTIFICATION_EVENTS.map((event) => {
                        const copy = eventCopy(event);
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
                              aria-label={t("settings.notifications.soundLabel", { title: copy.title })}
                              className="justify-self-center"
                              onCheckedChange={(checked) => updateEvent(event, "sound", checked)}
                            />
                            <Switch
                              checked={value.osNotification}
                              disabled={busy}
                              aria-label={t("settings.notifications.osLabel", { title: copy.title })}
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
                          <p className="text-sm font-medium">{t("settings.notifications.defaultSound")}</p>
                          <p className="mt-0.5 text-xs text-muted-foreground">{t("settings.notifications.defaultSoundNote", { platform: isMacOS() ? "macOS" : "Windows" })}</p>
                        </div>
                      </div>
                      <Button type="button" variant="outline" size="sm" onClick={onPreviewSound}>
                        {t("settings.notifications.preview")}
                      </Button>
                    </div>
                  </>
                ) : (
                  <div className="mt-5 flex min-h-52 flex-col items-center justify-center rounded-xl border border-dashed border-border text-center">
                    <p className="text-sm text-muted-foreground">{t("settings.notifications.loadFailed")}</p>
                    <Button type="button" variant="outline" size="sm" className="mt-3 h-11 gap-1.5" onClick={onReload}>
                      <RefreshCw aria-hidden className="size-3.5" />
                      {t("settings.retry")}
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
