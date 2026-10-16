import { useCallback, useEffect, useState } from "react";
import QRCode from "qrcode";
import {
  Copy,
  Globe,
  LoaderCircle,
  RefreshCw,
  ShieldAlert,
  Smartphone,
  Unplug,
} from "lucide-react";

import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import type { RemoteDevice, RemotePairing, RemoteStatus } from "@/lib/ipc";
import {
  formatCountdown,
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
  serve(): Promise<RemoteStatus>;
  copy(text: string): Promise<void>;
}

type Pending = "load" | "save" | "pair" | "serve" | `revoke:${string}` | null;

function errorText(error: unknown, fallback: string): string {
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

function PairingQr({ url }: { url: string }) {
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
      alt="휴대폰 카메라로 찍으면 연결 화면이 열리는 QR 코드"
      className="size-44 rounded-lg bg-white p-2"
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

  const run = async <T,>(
    kind: Exclude<Pending, null>,
    call: () => Promise<T>,
    apply: (value: T) => void,
    fallback: string,
  ) => {
    setPending(kind);
    setError(undefined);
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

  const tailscale = status?.tailscale;
  const tailscaleLine = !tailscale
    ? ""
    : !tailscale.installed
      ? "Tailscale이 설치되어 있지 않습니다. Tailscale을 설치하고 로그인해 주세요."
      : !tailscale.running
        ? "Tailscale이 실행 중이 아닙니다. Tailscale을 켜고 로그인해 주세요."
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
                  applyStatus,
                  "Tailscale로 공개하지 못했습니다.",
                )
              }
            >
              {pending === "serve" && (
                <LoaderCircle aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
              )}
              {status.tailscale.serveConfigured ? "공개됨" : "Tailscale로 공개"}
            </Button>
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
                    <p className="text-xs text-muted-foreground">휴대폰에서 아래 연결 코드를 입력하세요</p>
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
                        {formatLastSeen(device.lastSeenAt, Date.now())}
                      </span>
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
