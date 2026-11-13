/**
 * 폰 연결 — the header's one-click phone setup. It brings remote access up
 * step by step (install Tailscale, sign in, turn the server on, publish it to
 * the tailnet) and then shows a pairing QR that opens the public landing page,
 * refreshing the code before it expires until a phone pairs.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  CheckCircle2,
  ExternalLink,
  LoaderCircle,
  RefreshCw,
  ShieldAlert,
  Smartphone,
} from "lucide-react";

import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Progress } from "@/components/ui/progress";
import i18n from "@/lib/i18n";
import type { RemotePairing, RemoteStatus, TailscaleInstallProgress } from "@/lib/ipc";
import {
  downloadPercent,
  formatCountdown,
  formatDownloadProgress,
  formatPairingCode,
  secondsUntil,
} from "@/lib/remoteAccess";
import {
  CONNECT_WATCH_MS,
  LOGIN_WATCH_MS,
  PairingQr,
  TAILSCALE_DOWNLOAD_URL,
  WATCH_POLL_MS,
  errorText,
  type RemoteSettingsActions,
} from "./RemoteSettingsPanel";

/** A new code is requested this long before the shown one expires. */
export const PAIRING_REFRESH_MARGIN_MS = 15 * 1000;

const STEP_COUNT = 4;

type Phase =
  | { kind: "checking" }
  | { kind: "setup" }
  | { kind: "running"; step: number; label: string; url: string | null }
  | { kind: "manual" }
  | { kind: "qr"; pairing: RemotePairing }
  | { kind: "paired"; name: string }
  | { kind: "error"; message: string };

/** Thrown inside a run that a newer run or closing the dialog replaced. */
class Cancelled extends Error {}

export function isRemoteReady(status: RemoteStatus): boolean {
  const { tailscale } = status;
  return (
    status.enabled &&
    status.listening &&
    tailscale.running &&
    tailscale.serveConfigured &&
    tailscale.dnsName !== null
  );
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

export interface PhoneConnectDialogProps {
  open: boolean;
  onOpenChange(open: boolean): void;
  actions: RemoteSettingsActions;
  /** How often a wait re-reads the status or the device list; shorter in tests. */
  pollMs?: number;
}

export function PhoneConnectDialog({
  open,
  onOpenChange,
  actions,
  pollMs = WATCH_POLL_MS,
}: PhoneConnectDialogProps) {
  const { t } = useTranslation();
  const [phase, setPhase] = useState<Phase>({ kind: "checking" });
  const [status, setStatus] = useState<RemoteStatus | null>(null);
  const [installProgress, setInstallProgress] = useState<TailscaleInstallProgress | null>(null);
  // Every run (open, setup, retry, new code) takes a number; closing the
  // dialog or a newer run makes the older one stop at its next await.
  const runRef = useRef(0);
  // Devices present when the code was shown, so a new one marks the pairing.
  const knownDevices = useRef<Set<string> | null>(null);

  const guard = useCallback((run: number) => {
    if (runRef.current !== run) throw new Cancelled();
  }, []);

  const sleep = useCallback(
    (ms: number) => new Promise<void>((resolve) => window.setTimeout(resolve, ms)),
    [],
  );

  const showCode = useCallback(
    async (run: number) => {
      const devices = await actions.devices().catch(() => null);
      guard(run);
      knownDevices.current = devices ? new Set(devices.map((device) => device.id)) : null;
      const pairing = await actions.pairBegin();
      guard(run);
      setPhase({ kind: "qr", pairing });
    },
    [actions, guard],
  );

  const fail = useCallback((run: number, caught: unknown, fallback: string) => {
    if (caught instanceof Cancelled || runRef.current !== run) return;
    setPhase({ kind: "error", message: errorText(caught, fallback) });
  }, []);

  /** Re-read the status until `done` holds, at most `limit` ms. */
  const waitFor = useCallback(
    async (run: number, done: (status: RemoteStatus) => boolean, limit: number, timeout: string) => {
      const started = Date.now();
      for (;;) {
        await sleep(pollMs);
        guard(run);
        const next = await actions.status().catch(() => null);
        guard(run);
        if (next) {
          setStatus(next);
          if (done(next)) return next;
        }
        if (Date.now() - started > limit) throw new Error(timeout);
      }
    },
    [actions, guard, pollMs, sleep],
  );

  const newCode = useCallback(() => {
    const run = ++runRef.current;
    void showCode(run).catch((caught) => fail(run, caught, i18n.t("remoteAccess.pairCodeFailed")));
  }, [fail, showCode]);

  // On open: straight to the QR when everything is ready, else offer setup.
  const check = useCallback(() => {
    const run = ++runRef.current;
    setPhase({ kind: "checking" });
    void (async () => {
      const next = await actions.status();
      guard(run);
      setStatus(next);
      if (isRemoteReady(next)) await showCode(run);
      else setPhase({ kind: "setup" });
    })().catch((caught) => fail(run, caught, i18n.t("remoteAccess.phone.statusFailed")));
  }, [actions, fail, guard, showCode]);

  const setup = useCallback(() => {
    const run = ++runRef.current;
    const step = (index: number, label: string, url: string | null = null) => {
      guard(run);
      setPhase({ kind: "running", step: index, label, url });
    };
    const openBrowser = (url: string) => void actions.openUrl(url).catch(() => undefined);
    step(0, i18n.t("remoteAccess.phone.checking"));
    void (async () => {
      let next = await actions.status();
      guard(run);
      setStatus(next);

      if (!next.tailscale.installed) {
        if (!next.tailscale.installSupported) {
          setPhase({ kind: "manual" });
          return;
        }
        step(1, i18n.t("remoteAccess.phone.installStep"));
        setInstallProgress(null);
        try {
          next = (await actions.install()).status;
        } finally {
          setInstallProgress(null);
        }
        guard(run);
        setStatus(next);
        if (!next.tailscale.installed) {
          throw new Error(i18n.t("remoteAccess.phone.installUnverified"));
        }
      }

      if (!next.tailscale.running) {
        step(2, i18n.t("remoteAccess.phone.connectStep"));
        const login = await actions.login();
        guard(run);
        next = login.status;
        setStatus(next);
        if (!next.tailscale.running) {
          if (login.url) {
            step(2, i18n.t("remoteAccess.phone.finishSignIn"), login.url);
            openBrowser(login.url);
          }
          next = await waitFor(
            run,
            (value) => value.tailscale.running,
            login.url ? LOGIN_WATCH_MS : CONNECT_WATCH_MS,
            login.url
              ? i18n.t("remoteAccess.phone.loginTimeout")
              : i18n.t("remoteAccess.phone.connectTimeout"),
          );
        }
      }

      if (!next.enabled || !next.listening) {
        step(3, i18n.t("remoteAccess.phone.enableStep"));
        next = await actions.save(true, next.port);
        guard(run);
        setStatus(next);
        if (!next.listening) {
          throw new Error(next.error ?? i18n.t("remoteAccess.phone.serverFailed"));
        }
      }

      if (!next.tailscale.serveConfigured) {
        step(4, i18n.t("remoteAccess.phone.publishStep"));
        const serve = await actions.serve();
        guard(run);
        next = serve.status;
        setStatus(next);
        if (!next.tailscale.serveConfigured) {
          if (serve.url) {
            step(4, i18n.t("remoteAccess.phone.allowHttps"), serve.url);
            openBrowser(serve.url);
          }
          next = await waitFor(
            run,
            (value) => value.tailscale.serveConfigured,
            serve.url ? LOGIN_WATCH_MS : CONNECT_WATCH_MS,
            serve.url
              ? i18n.t("remoteAccess.phone.serveTimeout")
              : i18n.t("remoteAccess.phone.serveFailed"),
          );
        }
      }

      if (!next.tailscale.dnsName) {
        throw new Error(
          i18n.t("remoteAccess.phone.noDnsName"),
        );
      }
      await showCode(run);
    })().catch((caught) => fail(run, caught, i18n.t("remoteAccess.phone.enableFailed")));
  }, [actions, fail, guard, showCode, waitFor]);

  // Opening checks the state; closing stops every run, poll and timer.
  useEffect(() => {
    if (open) {
      check();
      return () => {
        runRef.current += 1;
      };
    }
    setPhase({ kind: "checking" });
    return undefined;
  }, [check, open]);

  useEffect(() => {
    if (!open || !actions.onInstallProgress) return;
    return actions.onInstallProgress(setInstallProgress);
  }, [actions, open]);

  // A phone that pairs (or a code locked by wrong guesses) shows at once.
  useEffect(() => {
    if (!open || !actions.onPairing) return;
    return actions.onPairing((notice) => {
      if (notice.kind === "paired") {
        runRef.current += 1;
        setPhase({ kind: "paired", name: notice.name });
      } else if (notice.kind === "codeLocked") {
        runRef.current += 1;
        setPhase({
          kind: "error",
          message:
            i18n.t("remoteAccess.phone.codeLocked"),
        });
      }
    });
  }, [actions, open]);

  const pairing = phase.kind === "qr" ? phase.pairing : null;

  // While a code is shown, request the next one before it expires (at once
  // when it already has).
  useEffect(() => {
    if (!pairing) return;
    const timer = window.setTimeout(
      newCode,
      Math.max(0, pairing.expiresAt - PAIRING_REFRESH_MARGIN_MS - Date.now()),
    );
    return () => window.clearTimeout(timer);
  }, [newCode, pairing]);

  // Without the pairing event, a device that appears marks the pairing.
  useEffect(() => {
    if (!pairing) return;
    const timer = window.setInterval(() => {
      const known = knownDevices.current;
      if (!known) return;
      actions
        .devices()
        .then((devices) => {
          const added = devices.find((device) => !known.has(device.id));
          if (added) {
            runRef.current += 1;
            setPhase({ kind: "paired", name: added.name });
          }
        })
        .catch(() => undefined);
    }, pollMs);
    return () => window.clearInterval(timer);
  }, [actions, pairing, pollMs]);

  const now = useNow(pairing !== null);
  // Counts down to the rotation, which comes before the code's own expiry.
  const remaining = pairing
    ? secondsUntil(pairing.expiresAt - PAIRING_REFRESH_MARGIN_MS, now)
    : 0;
  const dnsName = status?.tailscale.dnsName ?? null;
  const funnel = status?.tailscale.funnel === true;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <Smartphone aria-hidden className="size-5 text-primary" />
            {t("remoteAccess.phone.title")}
          </DialogTitle>
          <DialogDescription>
            {t("remoteAccess.phone.description")}
          </DialogDescription>
        </DialogHeader>

        {phase.kind === "checking" && (
          <p role="status" className="flex items-center gap-2 text-sm text-muted-foreground">
            <LoaderCircle aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
            {t("remoteAccess.phone.checking")}
          </p>
        )}

        {phase.kind === "setup" && (
          <div className="grid gap-3">
            <p className="text-sm leading-6 text-muted-foreground">
              {t("remoteAccess.phone.setupIntro")}
            </p>
            <Button type="button" className="min-h-11 gap-1.5" onClick={setup}>
              <Smartphone aria-hidden className="size-4" />
              {t("remoteAccess.phone.enable")}
            </Button>
          </div>
        )}

        {phase.kind === "running" && (
          <div role="status" className="grid gap-3">
            <p className="flex items-center gap-2 text-sm font-medium">
              <LoaderCircle aria-hidden className="size-4 shrink-0 animate-spin motion-reduce:animate-none" />
              {phase.step > 0 ? `${phase.step}/${STEP_COUNT} ${phase.label}` : phase.label}
            </p>
            {phase.step === 1 &&
              (installProgress?.phase === "install" ? (
                <p className="text-xs text-muted-foreground">
                  {t("remoteAccess.installing")}
                </p>
              ) : (
                <div className="grid gap-1.5">
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
                </div>
              ))}
            {phase.url && (
              <>
                <p className="text-xs leading-5 text-muted-foreground">
                  {t("remoteAccess.phone.afterBrowser")}
                </p>
                <Button
                  type="button"
                  variant="outline"
                  className="min-h-11 justify-self-start gap-1.5"
                  onClick={() => {
                    if (phase.url) void actions.openUrl(phase.url).catch(() => undefined);
                  }}
                >
                  <ExternalLink aria-hidden className="size-4" />
                  {t("remoteAccess.phone.reopenPage")}
                </Button>
              </>
            )}
          </div>
        )}

        {phase.kind === "manual" && (
          <div className="grid gap-3">
            <p className="text-sm leading-6 text-muted-foreground">
              {t("remoteAccess.phone.manualIntro")}
            </p>
            <div className="flex flex-wrap gap-2">
              <Button
                type="button"
                variant="outline"
                className="min-h-11 gap-1.5"
                onClick={() => void actions.openUrl(TAILSCALE_DOWNLOAD_URL).catch(() => undefined)}
              >
                <ExternalLink aria-hidden className="size-4" />
                {t("remoteAccess.openDownloadPage")}
              </Button>
              <Button type="button" className="min-h-11 gap-1.5" onClick={setup}>
                <RefreshCw aria-hidden className="size-4" />
                {t("remoteAccess.retry")}
              </Button>
            </div>
          </div>
        )}

        {phase.kind === "qr" && (
          <div className="flex flex-col items-center gap-3 text-center">
            {phase.pairing.url ? (
              <PairingQr url={phase.pairing.url} className="size-64" />
            ) : (
              <p className="text-xs text-muted-foreground">
                {t("remoteAccess.noQr")}
              </p>
            )}
            <p className="text-sm leading-6">
              {t("remoteAccess.phone.scanInstruction")}
            </p>
            <p className="font-mono text-2xl font-semibold tracking-widest">
              {formatPairingCode(phase.pairing.code)}
            </p>
            {dnsName && (
              <p className="grid gap-0.5 text-xs text-muted-foreground">
                <span>
                  {t("remoteAccess.phone.pcAddress")} <span className="font-mono font-medium text-foreground">{dnsName}</span>
                </span>
                <span>{t("remoteAccess.phone.sameAddress")}</span>
              </p>
            )}
            <p role="timer" className="text-xs text-muted-foreground">
              {t("remoteAccess.phone.rotatesIn", { time: formatCountdown(remaining) })}
            </p>
            {funnel && (
              <Alert variant="destructive" className="text-left">
                <ShieldAlert aria-hidden />
                <AlertTitle>{t("remoteAccess.funnelTitle")}</AlertTitle>
                <AlertDescription>
                  {t("remoteAccess.funnelDescription")}
                </AlertDescription>
              </Alert>
            )}
          </div>
        )}

        {phase.kind === "paired" && (
          <div className="grid gap-3">
            <p role="status" className="flex items-center gap-2 text-sm font-medium">
              <CheckCircle2 aria-hidden className="size-5 shrink-0 text-emerald-500" />
              {t("remoteAccess.phone.paired", { name: phase.name })}
            </p>
            <Button type="button" variant="outline" className="min-h-11 gap-1.5" onClick={newCode}>
              <Smartphone aria-hidden className="size-4" />
              {t("remoteAccess.phone.pairAnother")}
            </Button>
          </div>
        )}

        {phase.kind === "error" && (
          <div className="grid gap-3">
            <p
              role="alert"
              className="whitespace-pre-wrap rounded-lg border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm text-destructive"
            >
              {phase.message}
            </p>
            <Button type="button" className="min-h-11 gap-1.5" onClick={setup}>
              <RefreshCw aria-hidden className="size-4" />
              {t("remoteAccess.retry")}
            </Button>
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}
