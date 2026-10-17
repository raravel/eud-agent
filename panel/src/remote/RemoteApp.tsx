import { useCallback, useEffect, useState, type ReactNode } from "react";
import { LoaderCircle, Smartphone } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  normalizePairingCode,
  pairingCodeFromHash,
} from "@/lib/remoteAccess";
import { remoteTransport } from "@/lib/remoteTransport";
import {
  fetchMeStatus,
  logout,
  pair,
  type FetchFn,
  type RemoteDeviceInfo,
} from "./remoteApi";
import { RemoteWorkspace } from "./RemoteWorkspace";

const DEFAULT_DEVICE_NAME = "휴대폰";

function messageOf(error: unknown): string {
  return error instanceof Error && error.message
    ? error.message
    : "PC에 연결하지 못했습니다. 잠시 후 다시 시도해 주세요.";
}

export interface RemoteAppProps {
  fetchFn?: FetchFn;
  /** The paired screens; replaceable in tests. */
  workspace?: (props: { device: RemoteDeviceInfo; onDisconnect(): void }) => ReactNode;
}

function defaultWorkspace({ onDisconnect }: { onDisconnect(): void }) {
  return <RemoteWorkspace onDisconnect={onDisconnect} />;
}

/** Remote page: pair this browser with the desktop app, then work on its projects. */
export default function RemoteApp({ fetchFn = fetch, workspace = defaultWorkspace }: RemoteAppProps) {
  const [device, setDevice] = useState<RemoteDeviceInfo | null>(null);
  const [checking, setChecking] = useState(true);
  const [code, setCode] = useState(() => pairingCodeFromHash(window.location.hash) ?? "");
  const [deviceName, setDeviceName] = useState(DEFAULT_DEVICE_NAME);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string>();

  useEffect(() => {
    let active = true;
    fetchMeStatus(fetchFn)
      .then((me) => {
        if (!active) return;
        setDevice(me.device);
        if (me.pairAgain) setError(me.pairAgain.message);
      })
      .catch((caught) => {
        if (active) setError(messageOf(caught));
      })
      .finally(() => {
        if (active) setChecking(false);
      });
    return () => {
      active = false;
    };
  }, [fetchFn]);

  const normalized = normalizePairingCode(code);

  const submit = async () => {
    setPending(true);
    setError(undefined);
    try {
      const paired = await pair(normalized, deviceName.trim(), fetchFn);
      setDevice(paired);
      setCode("");
      // The code is spent; keep it out of history and bookmarks.
      window.history.replaceState(null, "", window.location.pathname);
    } catch (caught) {
      setError(messageOf(caught));
    } finally {
      setPending(false);
    }
  };

  const disconnect = useCallback(async () => {
    setPending(true);
    setError(undefined);
    remoteTransport().stop();
    try {
      await logout(fetchFn);
      setDevice(null);
    } catch (caught) {
      setError(messageOf(caught));
      setDevice(null);
    } finally {
      setPending(false);
    }
  }, [fetchFn]);
  const handleDisconnect = useCallback(() => void disconnect(), [disconnect]);

  if (device && !checking) {
    return <>{workspace({ device, onDisconnect: handleDisconnect })}</>;
  }

  return (
    <main className="min-h-dvh bg-background px-4 py-8 text-foreground">
      <section className="mx-auto w-full max-w-sm rounded-xl border border-border bg-card p-5">
        <h1 className="flex items-center gap-2 text-lg font-semibold">
          <Smartphone aria-hidden className="size-5 text-primary" />
          eud-agent 원격 접속
        </h1>
        {checking ? (
          <p role="status" className="mt-4 flex items-center gap-2 text-sm text-muted-foreground">
            <LoaderCircle aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
            연결 상태를 확인하는 중…
          </p>
        ) : (
          <form
            className="mt-4 grid gap-4"
            onSubmit={(event) => {
              event.preventDefault();
              void submit();
            }}
          >
            <p className="text-sm leading-6 text-muted-foreground">
              PC의 설정 → 원격 접속에서 기기 연결을 누르고 표시된 8자리 코드를 입력하세요.
            </p>
            <label htmlFor="remote-pair-code" className="grid gap-1.5 text-sm">
              <span className="font-medium">연결 코드</span>
              <Input
                id="remote-pair-code"
                autoComplete="one-time-code"
                autoCapitalize="characters"
                spellCheck={false}
                value={code}
                disabled={pending}
                placeholder="ABCD-2345"
                onChange={(event) => setCode(event.target.value)}
                className="h-11 font-mono tracking-widest"
              />
            </label>
            <label htmlFor="remote-device-name" className="grid gap-1.5 text-sm">
              <span className="font-medium">기기 이름</span>
              <Input
                id="remote-device-name"
                autoComplete="off"
                value={deviceName}
                disabled={pending}
                maxLength={64}
                onChange={(event) => setDeviceName(event.target.value)}
                className="h-11"
              />
            </label>
            <Button
              type="submit"
              className="min-h-11 gap-1.5"
              disabled={pending || normalized.length !== 8}
            >
              {pending && (
                <LoaderCircle aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
              )}
              연결
            </Button>
          </form>
        )}
        {error && (
          <p
            role="alert"
            className="mt-4 rounded-lg border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm text-destructive"
          >
            {error}
          </p>
        )}
      </section>
    </main>
  );
}
