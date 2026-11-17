import { useCallback, useEffect, useState } from "react";
import QRCode from "qrcode";
import { useTranslation } from "react-i18next";
import {
  Copy,
  Download,
  ExternalLink,
  Globe,
  LoaderCircle,
  LogIn,
  RefreshCw,
  ShieldAlert,
  Smartphone,
  Unplug,
} from "lucide-react";

import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Progress } from "@/components/ui/progress";
import { Switch } from "@/components/ui/switch";
import i18n from "@/lib/i18n";
import type {
  RemoteDevice,
  RemotePairing,
  RemotePairingNotice,
  RemoteStatus,
  TailscaleInstallProgress,
} from "@/lib/ipc";
import {
  downloadPercent,
  formatCountdown,
  formatDownloadProgress,
  formatLastSeen,
  formatPairingCode,
  parseRemotePort,
  secondsUntil,
} from "@/lib/remoteAccess";

/** Backend calls behind Settings → 원격 접속 (see `remoteStatus` & co. in ipc). */
export interface RemoteSettingsActions {
  status(): Promise<RemoteStatus>;
  save(enabled: boolean, port: number): Promise<RemoteStatus>;
  pairBegin(): Promise<RemotePairing>;
  devices(): Promise<RemoteDevice[]>;
  revoke(id: string): Promise<RemoteDevice[]>;
  /** `tailscale serve`; `url` is the page that enables HTTPS on the tailnet when it is still off. */
  serve(): Promise<{ url: string | null; status: RemoteStatus }>;
  /** Download, verify and install Tailscale on this PC. */
  install(): Promise<{ rebootRequired: boolean; status: RemoteStatus }>;
  /** `tailscale up`; `url` is the browser sign-in page when one is needed. */
  login(): Promise<{ url: string | null; status: RemoteStatus }>;
  openUrl(url: string): Promise<void>;
  copy(text: string): Promise<void>;
  /** Pairing changes as they happen; returns the unsubscribe. */
  onPairing?(listener: (notice: RemotePairingNotice) => void): () => void;
  /** Installer progress while `install` runs; returns the unsubscribe. */
  onInstallProgress?(listener: (progress: TailscaleInstallProgress) => void): () => void;
}

type Pending =
  | "load"
  | "save"
  | "pair"
  | "serve"
  | "install"
  | "login"
  | `revoke:${string}`
  | null;

export const TAILSCALE_DOWNLOAD_URL = "https://tailscale.com/download";
/** How long the screen waits for a browser sign-in (or HTTPS approval) to finish. */
export const LOGIN_WATCH_MS = 10 * 60 * 1000;
/** How long it waits for a `tailscale up` that needed no sign-in to finish connecting. */
export const CONNECT_WATCH_MS = 30 * 1000;
/** How often a watch re-reads the status. */
export const WATCH_POLL_MS = 3000;

export function errorText(error: unknown, fallback: string): string {
  if (typeof error === "string" && error.trim()) return error;
  if (error instanceof Error && error.message.trim()) return error.message;
  return fallback;
}

function useNow(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [active]);
  return now;
}

/** The pairing link as a QR code; `className` sets its size. */
export function PairingQr({ url, className = "size-44" }: { url: string; className?: string }) {
  const [svg, setSvg] = useState<string>();
  useEffect(() => {
    let active = true;
    QRCode.toString(url, { type: "svg", margin: 1, errorCorrectionLevel: "M" })
      .then((value) => {
        if (active) setSvg(value);
      })
      .catch(() => {
        if (active) setSvg(undefined);
      });
    return () => {
      active = false;
    };
  }, [url]);
  if (!svg) return null;
  return (
    <img
      src={`data:image/svg+xml;utf8,${encodeURIComponent(svg)}`}
      alt={i18n.t("remoteAccess.qrAlt")}
      className={`${className} rounded-lg bg-white p-2`}
    />
  );
}

export function RemoteSettingsPanel({
  actions,
  disabled = false,
}: {
  actions: RemoteSettingsActions;
  disabled?: boolean;
}) {
  const { t } = useTranslation();
  const [status, setStatus] = useState<RemoteStatus | null>(null);
  const [devices, setDevices] = useState<RemoteDevice[]>([]);
  const [portText, setPortText] = useState("");
  const [pairing, setPairing] = useState<RemotePairing | null>(null);
  const [pending, setPending] = useState<Pending>("load");
  const [error, setError] = useState<string>();
  const [copied, setCopied] = useState(false);
  const [installProgress, setInstallProgress] = useState<TailscaleInstallProgress | null>(null);
  const [notice, setNotice] = useState<string>();
  // A sign-in (`login`) or HTTPS approval (`serve`) being waited for.
  const [watch, setWatch] = useState<{ kind: "login" | "serve"; url: string | null } | null>(
    null,
  );
  const now = useNow(pairing !== null);

  const applyStatus = useCallback((value: RemoteStatus) => {
    setStatus(value);
    setPortText(String(value.port));
  }, []);

  const refresh = useCallback(async () => {
    setPending("load");
    setError(undefined);
    try {
      const [nextStatus, nextDevices] = await Promise.all([
        actions.status(),
        actions.devices(),
      ]);
      applyStatus(nextStatus);
      setDevices(nextDevices);
    } catch (caught) {
      setError(errorText(caught, t("remoteAccess.loadFailed")));
    } finally {
      setPending(null);
    }
  }, [actions, applyStatus, t]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // A device that paired, or one removed elsewhere (logging out on the
  // phone), shows at once; a used or locked code disappears.
  useEffect(() => {
    if (!actions.onPairing) return;
    return actions.onPairing((notice) => {
      if (notice.kind !== "revoked") setPairing(null);
      if (notice.kind === "codeLocked") {
        setError(t("remoteAccess.codeLockedSettings"));
      }
      actions.devices().then(setDevices).catch(() => undefined);
    });
  }, [actions, t]);

  useEffect(() => {
    if (!actions.onInstallProgress) return;
    return actions.onInstallProgress(setInstallProgress);
  }, [actions]);

  // After a sign-in starts (or `tailscale up` returned while Tailscale was
  // still starting), watch it come up. Only the status is replaced, so a
  // port being typed is left alone.
  useEffect(() => {
    if (!watch) return;
    const started = Date.now();
    const limit = watch.url ? LOGIN_WATCH_MS : CONNECT_WATCH_MS;
    const timer = window.setInterval(() => {
      if (Date.now() - started > limit) {
        setWatch(null);
        setError(
          watch.kind === "serve"
            ? t("remoteAccess.serveWatchTimeout")
            : watch.url
              ? t("remoteAccess.loginWatchTimeout")
              : t("remoteAccess.connectWatchTimeout"),
        );
        return;
      }
      actions
        .status()
        .then((next) => {
          setStatus(next);
          if (watch.kind === "serve" ? next.tailscale.serveConfigured : next.tailscale.running) {
            setWatch(null);
            setNotice(
              watch.kind === "serve"
                ? t("remoteAccess.servedNotice")
                : watch.url
                  ? t("remoteAccess.signedInNotice")
                  : t("remoteAccess.connectedNotice"),
            );
          }
        })
        .catch(() => undefined);
    }, WATCH_POLL_MS);
    return () => window.clearInterval(timer);
  }, [actions, watch, t]);

  const run = async <T,>(
    kind: Exclude<Pending, null>,
    call: () => Promise<T>,
    apply: (value: T) => void,
    fallback: string,
  ) => {
    setPending(kind);
    setError(undefined);
    setNotice(undefined);
    try {
      apply(await call());
    } catch (caught) {
      setError(errorText(caught, fallback));
    } finally {
      setPending(null);
    }
  };

  const port = parseRemotePort(portText);
  const busy = disabled || pending !== null;
  const remaining = pairing ? secondsUntil(pairing.expiresAt, now) : 0;
  const pairingLive = pairing !== null && remaining > 0;

  // While a code is live, watch for the phone to finish pairing so the new
  // device appears and the used code disappears without a manual refresh.
  useEffect(() => {
    if (!pairingLive) return;
    const known = devices.length;
    const timer = window.setInterval(() => {
      actions
        .devices()
        .then((next) => {
          if (next.length > known) {
            setDevices(next);
            setPairing(null);
          }
        })
        .catch(() => undefined);
    }, 3000);
    return () => window.clearInterval(timer);
  }, [actions, devices.length, pairingLive]);

  const toggle = (checked: boolean) => {
    if (!status) return;
    if (!checked) setPairing(null);
    void run(
      "save",
      () => actions.save(checked, port ?? status.port),
      applyStatus,
      t("remoteAccess.saveFailed"),
    );
  };

  const copyCommand = async () => {
    if (!status) return;
    try {
      await actions.copy(status.tailscale.serveCommand);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
    } catch {
      setError(t("remoteAccess.copyFailed"));
    }
  };

  const install = () =>
    void run(
      "install",
      async () => {
        setInstallProgress(null);
        try {
          return await actions.install();
        } finally {
          setInstallProgress(null);
        }
      },
      (result) => {
        applyStatus(result.status);
        setNotice(
          result.rebootRequired
            ? t("remoteAccess.installedReboot")
            : t("remoteAccess.installed"),
        );
      },
      t("remoteAccess.installFailed"),
    );

  const login = () =>
    void run(
      "login",
      () => actions.login(),
      (result) => {
        applyStatus(result.status);
        if (result.url) {
          setWatch({ kind: "login", url: result.url });
          void actions.openUrl(result.url).catch(() => undefined);
        } else if (result.status.tailscale.running) {
          setNotice(t("remoteAccess.connectedNotice"));
        } else {
          setWatch({ kind: "login", url: null });
        }
      },
      t("remoteAccess.loginFailed"),
    );

  const serveUrl = watch?.kind === "serve" ? watch.url : null;
  const signInUrl = watch?.kind === "login" ? watch.url : null;
  const tailscale = status?.tailscale;
  const tailscaleLine = !tailscale
    ? ""
    : !tailscale.installed
      ? tailscale.installSupported
        ? t("remoteAccess.line.notInstalled")
        : t("remoteAccess.line.notInstalledManual")
      : !tailscale.running
        ? tailscale.backendState === "NeedsLogin"
          ? t("remoteAccess.line.needsLogin")
          : tailscale.backendState === "Stopped"
            ? t("remoteAccess.line.stopped")
            : t("remoteAccess.line.notRunning")
        : tailscale.serveConfigured && tailscale.dnsName
          ? t("remoteAccess.line.served", { host: tailscale.dnsName })
          : tailscale.dnsName
            ? t("remoteAccess.line.connectedHost", { host: tailscale.dnsName })
            : t("remoteAccess.line.connected");

  return (
    <div className="animate-in fade-in duration-200 motion-reduce:animate-none">
      <div className="flex items-start justify-between gap-4">
        <div>
          <h2 className="text-base font-semibold">{t("remoteAccess.title")}</h2>
          <p className="mt-1.5 text-sm leading-6 text-muted-foreground">
            {t("remoteAccess.description")}
          </p>
        </div>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="h-11 shrink-0 gap-1.5"
          disabled={busy}
          onClick={() => void refresh()}
        >
          {pending === "load" ? (
            <LoaderCircle aria-hidden className="size-3.5 animate-spin motion-reduce:animate-none" />
          ) : (
            <RefreshCw aria-hidden className="size-3.5" />
          )}
          {t("remoteAccess.refresh")}
        </Button>
      </div>

      <Alert variant="destructive" className="mt-5">
        <ShieldAlert aria-hidden />
        <AlertTitle>{t("remoteAccess.warningTitle")}</AlertTitle>
        <AlertDescription>
          {t("remoteAccess.warningDescription")}
        </AlertDescription>
      </Alert>

      <section
        aria-labelledby="settings-remote-guide-heading"
        className="mt-4 rounded-xl border border-border bg-card/40 px-4 py-3.5"
      >
        <h3 id="settings-remote-guide-heading" className="text-sm font-medium">
          {t("remoteAccess.guide.heading")}
        </h3>
        <ol className="mt-2 list-decimal space-y-1 pl-5 text-xs leading-5 text-muted-foreground">
          <li>
            {t("remoteAccess.guide.step1")}
          </li>
          <li>{t("remoteAccess.guide.step2")}</li>
          <li>{t("remoteAccess.guide.step3")}</li>
          <li>
            {t("remoteAccess.guide.step4")}
          </li>
          <li>{t("remoteAccess.guide.step5")}</li>
        </ol>
      </section>

      {status && (
        <>
          <div className="mt-4 flex min-h-11 items-center justify-between gap-4 rounded-xl border border-border bg-card/40 px-4 py-3.5">
            <span className="min-w-0">
              <span className="block text-sm font-medium text-foreground">{t("remoteAccess.allow")}</span>
              <span role="status" className="mt-0.5 block text-xs leading-5 text-muted-foreground">
                {status.enabled
                  ? status.listening
                    ? t("remoteAccess.listening", { port: status.port })
                    : t("remoteAccess.notRunning")
                  : t("remoteAccess.off")}
              </span>
            </span>
            <Switch
              checked={status.enabled}
              disabled={busy}
              aria-label={t("remoteAccess.allow")}
              onCheckedChange={toggle}
            />
          </div>

          <form
            className="mt-3 grid min-w-0 gap-2 sm:grid-cols-[minmax(0,1fr)_auto]"
            onSubmit={(event) => {
              event.preventDefault();
              if (port === null) return;
              void run(
                "save",
                () => actions.save(status.enabled, port),
                applyStatus,
                t("remoteAccess.portSaveFailed"),
              );
            }}
          >
            <label htmlFor="settings-remote-port" className="grid min-w-0 gap-1.5 text-sm">
              <span className="font-medium text-foreground">{t("remoteAccess.port")}</span>
              <Input
                id="settings-remote-port"
                inputMode="numeric"
                autoComplete="off"
                value={portText}
                disabled={busy}
                aria-invalid={port === null}
                onChange={(event) => setPortText(event.target.value)}
                className="h-11"
              />
              {port === null && (
                <span className="text-xs text-destructive">
                  {t("remoteAccess.portInvalid")}
                </span>
              )}
            </label>
            <Button
              type="submit"
              variant="outline"
              className="min-h-11 self-end"
              disabled={busy || port === null || port === status.port}
            >
              {t("remoteAccess.savePort")}
            </Button>
          </form>

          <section
            aria-labelledby="settings-remote-tailscale-heading"
            className="mt-4 rounded-xl border border-border bg-card/40 px-4 py-3.5"
          >
            <h3 id="settings-remote-tailscale-heading" className="flex items-center gap-2 text-sm font-medium">
              <Globe aria-hidden className="size-4 text-primary" />
              Tailscale
            </h3>
            <p className="mt-1 text-xs leading-5 text-muted-foreground">{tailscaleLine}</p>
            {!status.tailscale.installed && status.tailscale.installSupported && (
              <div className="mt-3 grid gap-2">
                <p className="text-xs leading-5 text-muted-foreground">
                  {t("remoteAccess.installIntro")}
                </p>
                <Button
                  type="button"
                  className="min-h-11 justify-self-start gap-1.5"
                  disabled={busy}
                  onClick={install}
                >
                  {pending === "install" ? (
                    <LoaderCircle aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
                  ) : (
                    <Download aria-hidden className="size-4" />
                  )}
                  {t("remoteAccess.installTailscale")}
                </Button>
                {pending === "install" && (
                  <div role="status" className="grid gap-1.5">
                    {installProgress?.phase === "install" ? (
                      <p className="text-xs text-muted-foreground">
                        {t("remoteAccess.installing")}
                      </p>
                    ) : (
                      <>
                        <Progress
                          aria-label={t("remoteAccess.downloadLabel")}
                          value={
                            installProgress
                              ? downloadPercent(installProgress.downloaded, installProgress.total)
                              : 0
                          }
                        />
                        <p className="text-xs text-muted-foreground">
                          {installProgress
                            ? t("remoteAccess.downloading", {
                                progress: formatDownloadProgress(installProgress.downloaded, installProgress.total),
                              })
                            : t("remoteAccess.preparing")}
                        </p>
                      </>
                    )}
                  </div>
                )}
              </div>
            )}
            {!status.tailscale.installed && !status.tailscale.installSupported && (
              <Button
                type="button"
                variant="outline"
                className="mt-3 min-h-11 gap-1.5"
                onClick={() => void actions.openUrl(TAILSCALE_DOWNLOAD_URL).catch(() => undefined)}
              >
                <ExternalLink aria-hidden className="size-4" />
                {t("remoteAccess.openDownloadPage")}
              </Button>
            )}
            {status.tailscale.installed && !status.tailscale.running && (
              <div className="mt-3 grid gap-2">
                <Button
                  type="button"
                  className="min-h-11 justify-self-start gap-1.5"
                  disabled={busy}
                  onClick={login}
                >
                  {pending === "login" ? (
                    <LoaderCircle aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
                  ) : (
                    <LogIn aria-hidden className="size-4" />
                  )}
                  {status.tailscale.backendState === "Stopped" ? t("remoteAccess.connect") : t("remoteAccess.signIn")}
                </Button>
                {signInUrl && (
                  <div role="status" className="grid gap-1.5">
                    <p className="text-xs leading-5 text-muted-foreground">
                      {t("remoteAccess.signInWatch")}
                    </p>
                    <Button
                      type="button"
                      variant="outline"
                      className="min-h-11 justify-self-start gap-1.5"
                      onClick={() => void actions.openUrl(signInUrl).catch(() => undefined)}
                    >
                      <ExternalLink aria-hidden className="size-4" />
                      {t("remoteAccess.reopenSignIn")}
                    </Button>
                  </div>
                )}
              </div>
            )}
            {status.tailscale.funnel && (
              <Alert variant="destructive" className="mt-3">
                <ShieldAlert aria-hidden />
                <AlertTitle>{t("remoteAccess.funnelTitle")}</AlertTitle>
                <AlertDescription>
                  {t("remoteAccess.funnelDescription")}
                </AlertDescription>
              </Alert>
            )}
            <div className="mt-3 flex min-w-0 items-center gap-2">
              <code className="min-w-0 flex-1 overflow-x-auto whitespace-nowrap rounded-md border border-border bg-background px-3 py-2 font-mono text-xs">
                {status.tailscale.serveCommand}
              </code>
              <Button
                type="button"
                variant="outline"
                size="icon"
                className="size-11 shrink-0"
                aria-label={t("remoteAccess.copyServeLabel")}
                onClick={() => void copyCommand()}
              >
                <Copy aria-hidden className="size-4" />
              </Button>
            </div>
            {copied && (
              <p role="status" className="mt-1 text-xs text-muted-foreground">
                {t("remoteAccess.copied")}
              </p>
            )}
            <Button
              type="button"
              className="mt-3 min-h-11 gap-1.5"
              disabled={
                busy ||
                !status.tailscale.installed ||
                !status.tailscale.running ||
                status.tailscale.serveConfigured
              }
              onClick={() =>
                void run(
                  "serve",
                  () => actions.serve(),
                  (result) => {
                    applyStatus(result.status);
                    if (result.url && !result.status.tailscale.serveConfigured) {
                      setWatch({ kind: "serve", url: result.url });
                      void actions.openUrl(result.url).catch(() => undefined);
                    }
                  },
                  t("remoteAccess.serveFailed"),
                )
              }
            >
              {pending === "serve" && (
                <LoaderCircle aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
              )}
              {status.tailscale.serveConfigured ? t("remoteAccess.published") : t("remoteAccess.publish")}
            </Button>
            {serveUrl && (
              <div role="status" className="mt-2 grid gap-1.5">
                <p className="text-xs leading-5 text-muted-foreground">
                  {t("remoteAccess.httpsPending")}
                </p>
                <Button
                  type="button"
                  variant="outline"
                  className="min-h-11 justify-self-start gap-1.5"
                  onClick={() => void actions.openUrl(serveUrl).catch(() => undefined)}
                >
                  <ExternalLink aria-hidden className="size-4" />
                  {t("remoteAccess.reopenHttps")}
                </Button>
              </div>
            )}
          </section>

          <section
            aria-labelledby="settings-remote-devices-heading"
            className="mt-4 rounded-xl border border-border bg-card/40 px-4 py-3.5"
          >
            <div className="flex items-center justify-between gap-4">
              <h3 id="settings-remote-devices-heading" className="flex items-center gap-2 text-sm font-medium">
                <Smartphone aria-hidden className="size-4 text-primary" />
                {t("remoteAccess.devices.heading")}
              </h3>
              <Button
                type="button"
                variant="outline"
                className="min-h-11"
                disabled={busy || !status.enabled}
                onClick={() =>
                  void run("pair", () => actions.pairBegin(), setPairing, t("remoteAccess.pairCodeFailed"))
                }
              >
                {pending === "pair" && (
                  <LoaderCircle aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
                )}
                {t("remoteAccess.pairDevice")}
              </Button>
            </div>
            {!status.enabled && (
              <p className="mt-1 text-xs text-muted-foreground">
                {t("remoteAccess.enableFirst")}
              </p>
            )}

            {pairing && (
              <div className="mt-3 flex flex-col items-center gap-3 rounded-lg border border-border bg-background p-4 text-center">
                {pairingLive ? (
                  <>
                    <p className="text-xs text-muted-foreground">
                      {t("remoteAccess.scanHint")}
                    </p>
                    <p className="font-mono text-2xl font-semibold tracking-widest">
                      {formatPairingCode(pairing.code)}
                    </p>
                    <p role="timer" className="text-xs text-muted-foreground">
                      {t("remoteAccess.expiresIn", { time: formatCountdown(remaining) })}
                    </p>
                    {pairing.url ? (
                      <>
                        <PairingQr url={pairing.url} />
                        <p className="break-all text-xs text-muted-foreground">{pairing.url}</p>
                      </>
                    ) : (
                      <p className="text-xs text-muted-foreground">
                        {t("remoteAccess.noQr")}
                      </p>
                    )}
                  </>
                ) : (
                  <p className="text-sm text-muted-foreground">
                    {t("remoteAccess.codeExpired")}
                  </p>
                )}
              </div>
            )}

            {devices.length === 0 ? (
              <p className="mt-3 text-xs text-muted-foreground">{t("remoteAccess.noDevices")}</p>
            ) : (
              <ul className="mt-3 grid gap-2" aria-label={t("remoteAccess.deviceListLabel")}>
                {devices.map((device) => (
                  <li
                    key={device.id}
                    className="flex min-h-11 items-center justify-between gap-3 rounded-lg border border-border bg-background px-3 py-2"
                  >
                    <span className="min-w-0">
                      <span className="block truncate text-sm font-medium">{device.name}</span>
                      <span className="block text-xs text-muted-foreground">
                        {device.expired
                          ? t("remoteAccess.deviceExpired")
                          : formatLastSeen(device.lastSeenAt, Date.now())}
                      </span>
                      {device.ownerLogin && (
                        <span className="block truncate text-xs text-muted-foreground">
                          {device.ownerLogin}
                        </span>
                      )}
                    </span>
                    <Button
                      type="button"
                      variant="ghost"
                      className="min-h-11 shrink-0 gap-1.5 text-destructive"
                      aria-label={t("remoteAccess.disconnectLabel", { name: device.name })}
                      disabled={busy}
                      onClick={() =>
                        void run(
                          `revoke:${device.id}`,
                          () => actions.revoke(device.id),
                          setDevices,
                          t("remoteAccess.disconnectFailed"),
                        )
                      }
                    >
                      <Unplug aria-hidden className="size-4" />
                      {t("remoteAccess.disconnect")}
                    </Button>
                  </li>
                ))}
              </ul>
            )}
          </section>
        </>
      )}

      {notice && !error && !status?.error && (
        <p
          role="status"
          className="mt-4 rounded-lg border border-border bg-card/40 px-4 py-3 text-sm text-foreground"
        >
          {notice}
        </p>
      )}

      {(error || status?.error) && (
        <p
          role="alert"
          className="mt-4 whitespace-pre-wrap rounded-lg border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm text-destructive"
        >
          {error ?? status?.error}
        </p>
      )}
    </div>
  );
}
