import { useEffect, useState } from "react";
import { CheckCircle2, LoaderCircle, Smartphone, Unplug } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  normalizePairingCode,
  pairingCodeFromHash,
} from "@/lib/remoteAccess";
import {
  fetchMe,
  logout,
  pair,
  type FetchFn,
  type RemoteDeviceInfo,
} from "./remoteApi";

const DEFAULT_DEVICE_NAME = "휴대폰";

function messageOf(error: unknown): string {
  return error instanceof Error && error.message
    ? error.message
    : "PC에 연결하지 못했습니다. 잠시 후 다시 시도해 주세요.";
}

/** Remote page: pair this browser with the desktop app. */
export default function RemoteApp({ fetchFn = fetch }: { fetchFn?: FetchFn }) {
  const [device, setDevice] = useState<RemoteDeviceInfo | null>(null);
  const [checking, setChecking] = useState(true);
  const [code, setCode] = useState(() => pairingCodeFromHash(window.location.hash) ?? "");
  const [deviceName, setDeviceName] = useState(DEFAULT_DEVICE_NAME);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string>();

  useEffect(() => {
    let active = true;
    fetchMe(fetchFn)
      .then((me) => {
        if (active) setDevice(me);
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

  const disconnect = async () => {
    setPending(true);
    setError(undefined);
    try {
      await logout(fetchFn);
      setDevice(null);
    } catch (caught) {
      setError(messageOf(caught));
    } finally {
      setPending(false);
    }
  };

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
        ) : device ? (
          <div className="mt-4 grid gap-4">
            <p role="status" className="flex items-center gap-2 text-sm font-medium">
              <CheckCircle2 aria-hidden className="size-4 text-primary" />
              연결됨
            </p>
            <p className="text-sm text-muted-foreground">
              이 기기는 <span className="font-medium text-foreground">{device.name}</span>
              (으)로 PC에 연결되어 있습니다.
            </p>
            <Button
              type="button"
              variant="outline"
              className="min-h-11 gap-1.5"
              disabled={pending}
              onClick={() => void disconnect()}
            >
              <Unplug aria-hidden className="size-4" />
              연결 해제
            </Button>
          </div>
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
