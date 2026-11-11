/**
 * 폰 연결 — the header's one-click phone setup. It brings remote access up
 * step by step (install Tailscale, sign in, turn the server on, publish it to
 * the tailnet) and then shows a pairing QR that opens the public landing page,
 * refreshing the code before it expires until a phone pairs.
 */
import { useCallback, useEffect, useRef, useState } from "react";
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
    void showCode(run).catch((caught) => fail(run, caught, "연결 코드를 만들지 못했습니다."));
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
    })().catch((caught) => fail(run, caught, "원격 접속 상태를 확인하지 못했습니다."));
  }, [actions, fail, guard, showCode]);

  const setup = useCallback(() => {
    const run = ++runRef.current;
    const step = (index: number, label: string, url: string | null = null) => {
      guard(run);
      setPhase({ kind: "running", step: index, label, url });
    };
    const openBrowser = (url: string) => void actions.openUrl(url).catch(() => undefined);
    step(0, "원격 접속 상태를 확인하는 중…");
    void (async () => {
      let next = await actions.status();
      guard(run);
      setStatus(next);

      if (!next.tailscale.installed) {
        if (!next.tailscale.installSupported) {
          setPhase({ kind: "manual" });
          return;
        }
        step(1, "Tailscale 설치 중…");
        setInstallProgress(null);
        try {
          next = (await actions.install()).status;
        } finally {
          setInstallProgress(null);
        }
        guard(run);
        setStatus(next);
        if (!next.tailscale.installed) {
          throw new Error("Tailscale 설치를 확인하지 못했습니다. PC를 다시 시작한 뒤 다시 시도해 주세요.");
        }
      }

      if (!next.tailscale.running) {
        step(2, "Tailscale에 연결하는 중…");
        const login = await actions.login();
        guard(run);
        next = login.status;
        setStatus(next);
        if (!next.tailscale.running) {
          if (login.url) {
            step(2, "브라우저에서 Tailscale 로그인을 마쳐 주세요", login.url);
            openBrowser(login.url);
          }
          next = await waitFor(
            run,
            (value) => value.tailscale.running,
            login.url ? LOGIN_WATCH_MS : CONNECT_WATCH_MS,
            login.url
              ? "Tailscale 로그인을 기다리다 멈췄습니다. 로그인을 마쳤다면 다시 시도를 눌러 주세요."
              : "Tailscale이 아직 연결되지 않았습니다. 작업 표시줄의 Tailscale 아이콘을 확인한 뒤 다시 시도를 눌러 주세요.",
          );
        }
      }

      if (!next.enabled || !next.listening) {
        step(3, "원격 접속 켜는 중…");
        next = await actions.save(true, next.port);
        guard(run);
        setStatus(next);
        if (!next.listening) {
          throw new Error(next.error ?? "원격 접속 서버를 시작하지 못했습니다. 다시 시도를 눌러 주세요.");
        }
      }

      if (!next.tailscale.serveConfigured) {
        step(4, "테일넷에 공개하는 중…");
        const serve = await actions.serve();
        guard(run);
        next = serve.status;
        setStatus(next);
        if (!next.tailscale.serveConfigured) {
          if (serve.url) {
            step(4, "브라우저에서 HTTPS 허용을 눌러 주세요", serve.url);
            openBrowser(serve.url);
          }
          next = await waitFor(
            run,
            (value) => value.tailscale.serveConfigured,
            serve.url ? LOGIN_WATCH_MS : CONNECT_WATCH_MS,
            serve.url
              ? "테일넷의 HTTPS 허용을 기다리다 멈췄습니다. 허용했다면 다시 시도를 눌러 주세요."
              : "테일넷 공개를 확인하지 못했습니다. 다시 시도를 눌러 주세요.",
          );
        }
      }

      if (!next.tailscale.dnsName) {
        throw new Error(
          "이 PC의 Tailscale 주소를 확인하지 못했습니다. Tailscale 관리 페이지에서 MagicDNS를 켠 뒤 다시 시도해 주세요.",
        );
      }
      await showCode(run);
    })().catch((caught) => fail(run, caught, "원격 접속을 켜지 못했습니다."));
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
            "잘못된 코드가 여러 번 입력되어 연결 코드를 폐기했습니다. 본인 기기가 아니라면 무시하고, 다시 연결하려면 다시 시도를 누르세요.",
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
            폰 연결
          </DialogTitle>
          <DialogDescription>
            Tailscale로 연결된 휴대폰 브라우저에서 이 PC의 에이전트를 사용합니다. 본인 기기만
            연결해 주세요.
          </DialogDescription>
        </DialogHeader>

        {phase.kind === "checking" && (
          <p role="status" className="flex items-center gap-2 text-sm text-muted-foreground">
            <LoaderCircle aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
            원격 접속 상태를 확인하는 중…
          </p>
        )}

        {phase.kind === "setup" && (
          <div className="grid gap-3">
            <p className="text-sm leading-6 text-muted-foreground">
              아래 버튼을 누르면 Tailscale 설치와 로그인, 원격 접속 켜기, 테일넷 공개를 차례로
              진행합니다. 중간에 브라우저나 Windows 확인 창이 뜨면 안내에 따라 주세요.
            </p>
            <Button type="button" className="min-h-11 gap-1.5" onClick={setup}>
              <Smartphone aria-hidden className="size-4" />
              원격 접속 켜기
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
                  설치하는 중입니다. 관리자 권한 확인 창이 뜨면 예를 눌러 주세요.
                </p>
              ) : (
                <div className="grid gap-1.5">
                  <Progress
                    aria-label="Tailscale 설치 파일 내려받기"
                    value={
                      installProgress
                        ? downloadPercent(installProgress.downloaded, installProgress.total)
                        : 0
                    }
                  />
                  <p className="text-xs text-muted-foreground">
                    {installProgress
                      ? `내려받는 중 ${formatDownloadProgress(installProgress.downloaded, installProgress.total)}`
                      : "설치 파일을 준비하는 중입니다."}
                  </p>
                </div>
              ))}
            {phase.url && (
              <>
                <p className="text-xs leading-5 text-muted-foreground">
                  브라우저에서 마치면 이 창이 자동으로 이어집니다.
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
                  브라우저 페이지 다시 열기
                </Button>
              </>
            )}
          </div>
        )}

        {phase.kind === "manual" && (
          <div className="grid gap-3">
            <p className="text-sm leading-6 text-muted-foreground">
              이 PC에는 앱이 Tailscale을 설치할 수 없습니다. 내려받기 페이지에서 Tailscale을
              설치하고 로그인한 뒤 다시 시도를 눌러 주세요.
            </p>
            <div className="flex flex-wrap gap-2">
              <Button
                type="button"
                variant="outline"
                className="min-h-11 gap-1.5"
                onClick={() => void actions.openUrl(TAILSCALE_DOWNLOAD_URL).catch(() => undefined)}
              >
                <ExternalLink aria-hidden className="size-4" />
                Tailscale 내려받기 페이지 열기
              </Button>
              <Button type="button" className="min-h-11 gap-1.5" onClick={setup}>
                <RefreshCw aria-hidden className="size-4" />
                다시 시도
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
                Tailscale 주소를 확인하지 못해 QR 코드를 만들 수 없습니다. 휴대폰에서 이 PC의
                Tailscale 주소 뒤에 /remote/ 를 붙여 열고 코드를 입력해 주세요.
              </p>
            )}
            <p className="text-sm leading-6">
              {"휴대폰 카메라로 찍고 'PC 열기'를 누르세요. Tailscale이 없으면 그 페이지가 안내합니다."}
            </p>
            <p className="font-mono text-2xl font-semibold tracking-widest">
              {formatPairingCode(phase.pairing.code)}
            </p>
            {dnsName && (
              <p className="grid gap-0.5 text-xs text-muted-foreground">
                <span>
                  PC 주소: <span className="font-mono font-medium text-foreground">{dnsName}</span>
                </span>
                <span>휴대폰 안내 페이지에 같은 주소가 보이는지 확인하세요.</span>
              </p>
            )}
            <p role="timer" className="text-xs text-muted-foreground">
              {formatCountdown(remaining)} 후 새 코드로 바뀝니다 · 한 번만 사용할 수 있습니다
            </p>
            {funnel && (
              <Alert variant="destructive" className="text-left">
                <ShieldAlert aria-hidden />
                <AlertTitle>Funnel이 이 포트를 공개 인터넷에 열어 두었습니다</AlertTitle>
                <AlertDescription>
                  Funnel이 켜져 있는 동안 원격 접속은 이 PC 외의 요청을 모두 거절합니다.
                  터미널에서 tailscale funnel을 끄고(tailscale funnel reset 또는 tailscale funnel
                  --https=443 off) 테일넷 안에서만 공개해 주세요.
                </AlertDescription>
              </Alert>
            )}
          </div>
        )}

        {phase.kind === "paired" && (
          <div className="grid gap-3">
            <p role="status" className="flex items-center gap-2 text-sm font-medium">
              <CheckCircle2 aria-hidden className="size-5 shrink-0 text-emerald-500" />
              연결됨: {phase.name}
            </p>
            <Button type="button" variant="outline" className="min-h-11 gap-1.5" onClick={newCode}>
              <Smartphone aria-hidden className="size-4" />
              다른 휴대폰 연결
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
              다시 시도
            </Button>
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}
