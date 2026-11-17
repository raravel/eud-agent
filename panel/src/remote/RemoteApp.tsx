import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import i18n, { followRemoteAppLanguage } from "@/lib/i18n";
import { useTranslation } from "react-i18next";
import { LoaderCircle, Smartphone } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  defaultDeviceName,
  isPairingCode,
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

function messageOf(error: unknown): string {
  return error instanceof Error && error.message
    ? error.message
    : i18n.t("remote.api.connectFailed");
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
  const { t } = useTranslation();
  const [device, setDevice] = useState<RemoteDeviceInfo | null>(null);
  const [checking, setChecking] = useState(true);
  // The code a QR link brought; paired with once, automatically.
  const [linkCode] = useState(() => pairingCodeFromHash(window.location.hash));
  const [code, setCode] = useState(linkCode ?? "");
  const [deviceName, setDeviceName] = useState(() =>
    defaultDeviceName(navigator.userAgent, navigator.maxTouchPoints),
  );
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string>();
  const autoPaired = useRef(false);

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

  // Paired: the page speaks the PC's language (before, the browser's).
  const paired = device !== null;
  useEffect(() => {
    if (!paired) return;
    let active = true;
    let unfollow: (() => void) | undefined;
    void followRemoteAppLanguage().then((stop) => {
      if (active) unfollow = stop;
      else stop();
    });
    return () => {
      active = false;
      unfollow?.();
    };
  }, [paired]);

  const normalized = normalizePairingCode(code);

  const submit = useCallback(
    async (pairingCode: string, name: string) => {
      setPending(true);
      setError(undefined);
      try {
        const paired = await pair(pairingCode, name.trim(), fetchFn);
        setDevice(paired);
        setCode("");
        // The code is spent; keep it out of history and bookmarks.
        window.history.replaceState(null, "", window.location.pathname);
      } catch (caught) {
        setError(messageOf(caught));
      } finally {
        setPending(false);
      }
    },
    [fetchFn],
  );

  // A QR link's code pairs at once, without a tap; a browser already paired
  // only drops it from the address. The code leaves the address before the
  // attempt, so a refresh after a failure never retries it in a loop; the
  // form keeps it with the error.
  useEffect(() => {
    if (checking || autoPaired.current || !linkCode) return;
    autoPaired.current = true;
    if (device) {
      window.history.replaceState(null, "", window.location.pathname);
    } else if (isPairingCode(linkCode)) {
      window.history.replaceState(null, "", window.location.pathname);
      void submit(linkCode, deviceName);
    }
  }, [checking, device, deviceName, linkCode, submit]);

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
          {t("remote.app.title")}
        </h1>
        {checking ? (
          <p role="status" className="mt-4 flex items-center gap-2 text-sm text-muted-foreground">
            <LoaderCircle aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
            {t("remote.app.checking")}
          </p>
        ) : (
          <form
            className="mt-4 grid gap-4"
            onSubmit={(event) => {
              event.preventDefault();
              void submit(normalized, deviceName);
            }}
          >
            <p className="text-sm leading-6 text-muted-foreground">
              {t("remote.app.help")}
            </p>
            <label htmlFor="remote-pair-code" className="grid gap-1.5 text-sm">
              <span className="font-medium">{t("remote.app.codeLabel")}</span>
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
              <span className="font-medium">{t("remote.app.deviceLabel")}</span>
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
              {t("remote.app.connect")}
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
