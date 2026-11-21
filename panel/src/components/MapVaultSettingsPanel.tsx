import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { CheckCircle2, KeyRound, LoaderCircle } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import i18n from "@/lib/i18n";
import type { MapVaultErrorCode, MapVaultSettings } from "@/lib/ipc";

/** The address MapVault uses until the user picks another. */
export const MAPVAULT_DEFAULT_ADDRESS = "https://sc.mi3.dev";

/** Backend calls behind Settings → MapVault (see `mapVaultSettingsGet` & co. in ipc). */
export interface MapVaultSettingsActions {
  load(): Promise<MapVaultSettings>;
  save(address: string, apiKey?: string): Promise<MapVaultSettings>;
  clearKey(): Promise<MapVaultSettings>;
}

const ERROR_CODES: readonly MapVaultErrorCode[] = [
  "mapvault_address_invalid",
  "mapvault_key_format",
  "mapvault_key_store",
  "mapvault_key_invalid",
  "mapvault_unreachable",
  "mapvault_unavailable",
];

function errorMessage(error: unknown): string {
  const code =
    typeof error === "string" ? error : error instanceof Error ? error.message : "";
  return (ERROR_CODES as readonly string[]).includes(code)
    ? i18n.t(`settings.mapvault.error.${code}`)
    : i18n.t("settings.mapvault.saveFailed");
}

export function MapVaultSettingsPanel({
  actions,
  disabled = false,
}: {
  actions: MapVaultSettingsActions;
  disabled?: boolean;
}) {
  const { t } = useTranslation();
  const [saved, setSaved] = useState<MapVaultSettings | null>(null);
  const [address, setAddress] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [pending, setPending] = useState<"load" | "save" | "clear" | null>(
    "load",
  );
  const [error, setError] = useState<string>();
  const [done, setDone] = useState(false);

  useEffect(() => {
    let active = true;
    actions
      .load()
      .then((value) => {
        if (!active) return;
        setSaved(value);
        setAddress(value.address);
      })
      .catch(() => {
        if (active) setError(i18n.t("settings.mapvault.loadFailed"));
      })
      .finally(() => {
        if (active) setPending(null);
      });
    return () => {
      active = false;
    };
  }, [actions]);

  const run = async (
    kind: "save" | "clear",
    call: () => Promise<MapVaultSettings>,
  ) => {
    setPending(kind);
    setError(undefined);
    setDone(false);
    try {
      const value = await call();
      setSaved(value);
      setAddress(value.address);
      setApiKey("");
      setDone(kind === "save");
    } catch (caught) {
      setError(errorMessage(caught));
    } finally {
      setPending(null);
    }
  };

  const busy = disabled || pending !== null;
  const trimmedKey = apiKey.trim();
  const trimmedAddress = address.trim();

  return (
    <div className="animate-in fade-in duration-200 motion-reduce:animate-none">
      <h2 className="text-base font-semibold">{t("settings.mapvault.title")}</h2>
      <p className="mt-1.5 text-sm leading-6 text-muted-foreground break-keep">
        {t("settings.mapvault.description")}
      </p>

      {saved && (
        <form
          className="mt-5 grid min-w-0 gap-4 rounded-xl border border-border bg-card/40 px-4 py-3.5"
          onSubmit={(event) => {
            event.preventDefault();
            void run("save", () =>
              actions.save(
                trimmedAddress,
                trimmedKey.length > 0 ? trimmedKey : undefined,
              ),
            );
          }}
        >
          <div className="grid min-w-0 gap-1.5 text-sm">
            <label
              htmlFor="settings-mapvault-address"
              className="font-medium text-foreground"
            >
              {t("settings.mapvault.address")}
            </label>
            <div className="flex min-w-0 flex-col gap-2 sm:flex-row">
              <Input
                id="settings-mapvault-address"
                type="url"
                inputMode="url"
                autoComplete="off"
                spellCheck={false}
                value={address}
                disabled={busy}
                onChange={(event) => {
                  setAddress(event.target.value);
                  setDone(false);
                }}
                aria-describedby="settings-mapvault-address-hint"
                className="h-11 min-w-0 flex-1"
              />
              <Button
                type="button"
                variant="outline"
                className="min-h-11"
                disabled={busy || trimmedAddress === MAPVAULT_DEFAULT_ADDRESS}
                onClick={() => {
                  setAddress(MAPVAULT_DEFAULT_ADDRESS);
                  setDone(false);
                }}
              >
                {t("settings.mapvault.resetAddress")}
              </Button>
            </div>
            <span
              id="settings-mapvault-address-hint"
              className="text-xs leading-5 text-muted-foreground break-keep"
            >
              {t("settings.mapvault.addressHint")}
            </span>
          </div>

          <div className="grid min-w-0 gap-1.5 text-sm">
            <label
              htmlFor="settings-mapvault-api-key"
              className="font-medium text-foreground"
            >
              {t("settings.mapvault.apiKey")}
            </label>
            <Input
              id="settings-mapvault-api-key"
              type="password"
              autoComplete="off"
              spellCheck={false}
              value={apiKey}
              disabled={busy}
              onChange={(event) => {
                setApiKey(event.target.value);
                setDone(false);
              }}
              placeholder={
                saved.hasKey
                  ? t("settings.mapvault.keySaved")
                  : t("settings.mapvault.keyEnter")
              }
              aria-describedby="settings-mapvault-key-hint"
              className="h-11"
            />
            <span
              id="settings-mapvault-key-hint"
              className="text-xs leading-5 text-muted-foreground break-keep"
            >
              {t("settings.mapvault.apiKeyHint")}
            </span>
            {!saved.hasKey && (
              <span className="text-sm text-muted-foreground break-keep">
                {t("settings.mapvault.keyMissing")}
              </span>
            )}
          </div>

          <div className="flex flex-col-reverse gap-2 sm:flex-row sm:items-center sm:justify-end">
            {saved.hasKey && (
              <Button
                type="button"
                variant="ghost"
                className="min-h-11 text-muted-foreground"
                disabled={busy}
                onClick={() => void run("clear", actions.clearKey)}
              >
                {t("settings.mapvault.deleteKey")}
              </Button>
            )}
            <Button
              type="submit"
              className="min-h-11"
              disabled={busy || trimmedAddress.length === 0}
            >
              {pending === "save" ? (
                <LoaderCircle
                  aria-hidden
                  className="size-4 animate-spin motion-reduce:animate-none"
                />
              ) : (
                <KeyRound aria-hidden className="size-4" />
              )}
              {pending === "save"
                ? t("settings.mapvault.saving")
                : t("settings.mapvault.save")}
            </Button>
          </div>
          {done && (
            <p
              role="status"
              className="flex items-center gap-1.5 text-sm text-emerald-400"
            >
              <CheckCircle2 aria-hidden className="size-4" />
              {saved.owner
                ? t("settings.mapvault.savedOwner", {
                    nickname: saved.owner.nickname || saved.owner.username,
                    username: saved.owner.username,
                    keyName: saved.owner.keyName,
                  })
                : t("settings.mapvault.saved")}
            </p>
          )}
          {done && saved.owner?.readOnly && (
            <p
              role="status"
              className="rounded-lg border border-border bg-muted/50 px-4 py-3 text-sm text-foreground break-keep"
            >
              {t("settings.mapvault.readOnly")}
            </p>
          )}
        </form>
      )}

      {pending === "load" && (
        <p
          role="status"
          className="mt-5 flex items-center gap-1.5 text-sm text-muted-foreground"
        >
          <LoaderCircle
            aria-hidden
            className="size-4 animate-spin motion-reduce:animate-none"
          />
          {t("settings.euddraft.loading")}
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
    </div>
  );
}
