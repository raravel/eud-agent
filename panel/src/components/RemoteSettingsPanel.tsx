import { useCallback, useEffect, useState } from "react";
import QRCode from "qrcode";
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
      alt="휴대폰 카메라로 찍으면 연결 안내 페이지가 열리는 QR 코드"
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
      setError(errorText(caught, "원격 접속 설정을 불러오지 못했습니다."));
    } finally {
      setPending(null);
    }
  }, [actions, applyStatus]);

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
        setError("잘못된 코드가 여러 번 입력되어 연결 코드를 폐기했습니다. 본인 기기가 아니라면 무시하고, 다시 연결하려면 새 코드를 만드세요.");
      }
      actions.devices().then(setDevices).catch(() => undefined);
    });
  }, [actions]);

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
            ? "테일넷의 HTTPS 허용을 기다리다 멈췄습니다. 허용했다면 새로고침을, 아니면 Tailscale로 공개를 다시 눌러 주세요."
            : watch.url
              ? "Tailscale 로그인을 기다리다 멈췄습니다. 로그인을 마쳤다면 새로고침을, 아니면 Tailscale 로그인을 다시 눌러 주세요."
              : "Tailscale이 아직 연결되지 않았습니다. 새로고침을 누르거나 작업 표시줄의 Tailscale 아이콘을 확인해 주세요.",
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
                ? "테일넷에 공개했습니다."
                : watch.url
                  ? "Tailscale에 로그인했습니다."
                  : "Tailscale이 연결되었습니다.",
            );
          }
        })
        .catch(() => undefined);
    }, WATCH_POLL_MS);
    return () => window.clearInterval(timer);
  }, [actions, watch]);

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
      "원격 접속 설정을 저장하지 못했습니다.",
    );
  };

  const copyCommand = async () => {
    if (!status) return;
    try {
      await actions.copy(status.tailscale.serveCommand);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
    } catch {
      setError("클립보드에 복사하지 못했습니다. 명령을 직접 선택해 복사해 주세요.");
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
            ? "Tailscale을 설치했습니다. 설치를 마치려면 PC를 다시 시작해야 할 수 있습니다. 이어서 Tailscale 로그인을 눌러 주세요."
            : "Tailscale을 설치했습니다. 이어서 Tailscale 로그인을 눌러 휴대폰과 같은 계정으로 로그인해 주세요.",
        );
      },
      "Tailscale을 설치하지 못했습니다.",
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
          setNotice("Tailscale이 연결되었습니다.");
        } else {
          setWatch({ kind: "login", url: null });
        }
      },
      "Tailscale 로그인을 시작하지 못했습니다.",
    );

  const serveUrl = watch?.kind === "serve" ? watch.url : null;
  const signInUrl = watch?.kind === "login" ? watch.url : null;
  const tailscale = status?.tailscale;
  const tailscaleLine = !tailscale
    ? ""
    : !tailscale.installed
      ? tailscale.installSupported
        ? "이 PC에 Tailscale이 설치되어 있지 않습니다."
        : "이 PC에 Tailscale이 설치되어 있지 않습니다. Tailscale 내려받기 페이지에서 설치하고 로그인해 주세요."
      : !tailscale.running
        ? tailscale.backendState === "NeedsLogin"
          ? "Tailscale에 로그인하지 않았습니다. Tailscale 로그인을 눌러 휴대폰과 같은 계정으로 로그인해 주세요."
          : tailscale.backendState === "Stopped"
            ? "Tailscale 연결이 꺼져 있습니다. Tailscale 연결을 눌러 켜 주세요."
            : "Tailscale이 실행 중이 아닙니다. 잠시 후 새로고침을 누르거나 Tailscale 로그인을 눌러 주세요."
        : tailscale.serveConfigured && tailscale.dnsName
          ? `테일넷에 공개됨: https://${tailscale.dnsName}/remote/`
          : `연결됨${tailscale.dnsName ? ` (${tailscale.dnsName})` : ""} · 아직 이 앱을 공개하지 않았습니다.`;

  return (
    <div className="animate-in fade-in duration-200 motion-reduce:animate-none">
      <div className="flex items-start justify-between gap-4">
        <div>
          <h2 className="text-base font-semibold">원격 접속</h2>
          <p className="mt-1.5 text-sm leading-6 text-muted-foreground">
            Tailscale로 연결된 휴대폰 브라우저에서 이 PC의 에이전트를 사용합니다.
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
          새로고침
        </Button>
      </div>

      <Alert variant="destructive" className="mt-5">
        <ShieldAlert aria-hidden />
        <AlertTitle>연결한 기기는 이 PC에서 에이전트를 실행할 수 있습니다</AlertTitle>
        <AlertDescription>
          에이전트는 이 PC에서 파일을 고치고 프로그램을 실행합니다. 본인 기기만
          연결하고, 쓰지 않는 기기는 바로 해제해 주세요.
        </AlertDescription>
      </Alert>

      <section
        aria-labelledby="settings-remote-guide-heading"
        className="mt-4 rounded-xl border border-border bg-card/40 px-4 py-3.5"
      >
        <h3 id="settings-remote-guide-heading" className="text-sm font-medium">
          연결 방법
        </h3>
        <ol className="mt-2 list-decimal space-y-1 pl-5 text-xs leading-5 text-muted-foreground">
          <li>
            PC에는 아래 Tailscale 설치로, 휴대폰에는 앱스토어에서 Tailscale을 설치하고 같은
            계정으로 로그인합니다.
          </li>
          <li>아래 원격 접속 허용을 켭니다.</li>
          <li>Tailscale로 공개를 눌러 이 PC를 Tailscale 주소(HTTPS)로 엽니다.</li>
          <li>
            기기 연결을 누르고 휴대폰 카메라로 QR 코드를 찍은 뒤, 열린 안내 페이지에서 PC 열기를
            누릅니다. 헤더의 폰 연결을 누르면 이 과정을 한 번에 진행합니다.
          </li>
          <li>휴대폰 브라우저 메뉴의 홈 화면에 추가로 앱처럼 열 수 있습니다.</li>
        </ol>
      </section>

      {status && (
        <>
          <div className="mt-4 flex min-h-11 items-center justify-between gap-4 rounded-xl border border-border bg-card/40 px-4 py-3.5">
            <span className="min-w-0">
              <span className="block text-sm font-medium text-foreground">원격 접속 허용</span>
              <span role="status" className="mt-0.5 block text-xs leading-5 text-muted-foreground">
                {status.enabled
                  ? status.listening
                    ? `127.0.0.1:${status.port}에서 대기 중`
                    : "서버가 실행되지 않았습니다."
                  : "꺼져 있습니다."}
              </span>
            </span>
            <Switch
              checked={status.enabled}
              disabled={busy}
              aria-label="원격 접속 허용"
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
                "포트를 저장하지 못했습니다.",
              );
            }}
          >
            <label htmlFor="settings-remote-port" className="grid min-w-0 gap-1.5 text-sm">
              <span className="font-medium text-foreground">포트</span>
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
                  1024부터 65535 사이의 숫자를 입력해 주세요.
                </span>
              )}
            </label>
            <Button
              type="submit"
              variant="outline"
              className="min-h-11 self-end"
              disabled={busy || port === null || port === status.port}
            >
              포트 저장
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
                  pkgs.tailscale.com에서 공식 설치 파일을 받아 검증한 뒤 설치합니다. 설치 중
                  Windows 관리자 권한 확인 창이 한 번 뜹니다.
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
                  Tailscale 설치
                </Button>
                {pending === "install" && (
                  <div role="status" className="grid gap-1.5">
                    {installProgress?.phase === "install" ? (
                      <p className="text-xs text-muted-foreground">
                        설치하는 중입니다. 관리자 권한 확인 창이 뜨면 예를 눌러 주세요.
                      </p>
                    ) : (
                      <>
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
                Tailscale 내려받기 페이지 열기
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
                  {status.tailscale.backendState === "Stopped" ? "Tailscale 연결" : "Tailscale 로그인"}
                </Button>
                {signInUrl && (
                  <div role="status" className="grid gap-1.5">
                    <p className="text-xs leading-5 text-muted-foreground">
                      브라우저에서 Tailscale 로그인을 마치면 이 화면이 자동으로 이어집니다.
                    </p>
                    <Button
                      type="button"
                      variant="outline"
                      className="min-h-11 justify-self-start gap-1.5"
                      onClick={() => void actions.openUrl(signInUrl).catch(() => undefined)}
                    >
                      <ExternalLink aria-hidden className="size-4" />
                      로그인 페이지 다시 열기
                    </Button>
                  </div>
                )}
              </div>
            )}
            {status.tailscale.funnel && (
              <Alert variant="destructive" className="mt-3">
                <ShieldAlert aria-hidden />
                <AlertTitle>Funnel이 이 포트를 공개 인터넷에 열어 두었습니다</AlertTitle>
                <AlertDescription>
                  Funnel이 켜져 있는 동안 원격 접속은 이 PC 외의 요청을 모두 거절합니다.
                  터미널에서 tailscale funnel을 끄고(tailscale funnel reset 또는 tailscale funnel
                  --https=443 off) 테일넷 안에서만 공개해 주세요.
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
                aria-label="공개 명령 복사"
                onClick={() => void copyCommand()}
              >
                <Copy aria-hidden className="size-4" />
              </Button>
            </div>
            {copied && (
              <p role="status" className="mt-1 text-xs text-muted-foreground">
                복사했습니다.
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
                  "Tailscale로 공개하지 못했습니다.",
                )
              }
            >
              {pending === "serve" && (
                <LoaderCircle aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
              )}
              {status.tailscale.serveConfigured ? "공개됨" : "Tailscale로 공개"}
            </Button>
            {serveUrl && (
              <div role="status" className="mt-2 grid gap-1.5">
                <p className="text-xs leading-5 text-muted-foreground">
                  테일넷에서 HTTPS를 아직 쓰지 않습니다. 브라우저에 열린 페이지에서 허용을 누르면
                  이 화면이 자동으로 이어집니다.
                </p>
                <Button
                  type="button"
                  variant="outline"
                  className="min-h-11 justify-self-start gap-1.5"
                  onClick={() => void actions.openUrl(serveUrl).catch(() => undefined)}
                >
                  <ExternalLink aria-hidden className="size-4" />
                  HTTPS 허용 페이지 다시 열기
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
                연결된 기기
              </h3>
              <Button
                type="button"
                variant="outline"
                className="min-h-11"
                disabled={busy || !status.enabled}
                onClick={() =>
                  void run("pair", () => actions.pairBegin(), setPairing, "연결 코드를 만들지 못했습니다.")
                }
              >
                {pending === "pair" && (
                  <LoaderCircle aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
                )}
                기기 연결
              </Button>
            </div>
            {!status.enabled && (
              <p className="mt-1 text-xs text-muted-foreground">
                기기를 연결하려면 원격 접속을 먼저 켜 주세요.
              </p>
            )}

            {pairing && (
              <div className="mt-3 flex flex-col items-center gap-3 rounded-lg border border-border bg-background p-4 text-center">
                {pairingLive ? (
                  <>
                    <p className="text-xs text-muted-foreground">
                      휴대폰 카메라로 QR 코드를 찍고 안내 페이지의 PC 열기를 누르세요. 코드를 직접
                      입력해도 됩니다.
                    </p>
                    <p className="font-mono text-2xl font-semibold tracking-widest">
                      {formatPairingCode(pairing.code)}
                    </p>
                    <p role="timer" className="text-xs text-muted-foreground">
                      {formatCountdown(remaining)} 후 만료 · 한 번만 사용할 수 있습니다
                    </p>
                    {pairing.url ? (
                      <>
                        <PairingQr url={pairing.url} />
                        <p className="break-all text-xs text-muted-foreground">{pairing.url}</p>
                      </>
                    ) : (
                      <p className="text-xs text-muted-foreground">
                        Tailscale 주소를 확인하지 못해 QR 코드를 만들 수 없습니다. 휴대폰에서
                        이 PC의 Tailscale 주소 뒤에 /remote/ 를 붙여 열고 코드를 입력해 주세요.
                      </p>
                    )}
                  </>
                ) : (
                  <p className="text-sm text-muted-foreground">
                    코드가 만료되었습니다. 기기 연결을 다시 눌러 새 코드를 만드세요.
                  </p>
                )}
              </div>
            )}

            {devices.length === 0 ? (
              <p className="mt-3 text-xs text-muted-foreground">연결된 기기가 없습니다.</p>
            ) : (
              <ul className="mt-3 grid gap-2" aria-label="연결된 기기 목록">
                {devices.map((device) => (
                  <li
                    key={device.id}
                    className="flex min-h-11 items-center justify-between gap-3 rounded-lg border border-border bg-background px-3 py-2"
                  >
                    <span className="min-w-0">
                      <span className="block truncate text-sm font-medium">{device.name}</span>
                      <span className="block text-xs text-muted-foreground">
                        {device.expired
                          ? "만료됨 · 30일 동안 사용하지 않아 다시 연결해야 합니다"
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
                      aria-label={`${device.name} 연결 해제`}
                      disabled={busy}
                      onClick={() =>
                        void run(
                          `revoke:${device.id}`,
                          () => actions.revoke(device.id),
                          setDevices,
                          "기기 연결을 해제하지 못했습니다.",
                        )
                      }
                    >
                      <Unplug aria-hidden className="size-4" />
                      해제
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
