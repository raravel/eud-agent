import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { Switch } from "@/components/ui/switch";
import type { MapTestSettings } from "@/lib/ipc";

/** Backend calls behind Settings → 컴파일 → 맵 실행 테스트 (see `mapTestSettingsGet` & co. in ipc). */
export interface MapTestSettingsActions {
  load(): Promise<MapTestSettings>;
  save(enabled: boolean): Promise<MapTestSettings>;
  remove(): Promise<MapTestSettings>;
  /** Subscribes to the download progress (0..=100); returns the unsubscribe. */
  onProgress(listener: (pct: number) => void): Promise<() => void>;
}

function errorText(caught: unknown): string {
  return caught instanceof Error ? caught.message : String(caught);
}

export function MapTestSettingsPanel({
  actions,
  disabled,
}: {
  actions: MapTestSettingsActions;
  disabled: boolean;
}) {
  const { t } = useTranslation();
  const [state, setState] = useState<MapTestSettings | null>(null);
  const [pending, setPending] = useState<"load" | "save" | "remove" | null>(
    "load",
  );
  const [progress, setProgress] = useState<number | null>(null);
  const [error, setError] = useState<string>();

  useEffect(() => {
    let active = true;
    actions
      .load()
      .then((value) => {
        if (active) setState(value);
      })
      .catch((caught) => {
        if (active) setError(errorText(caught));
      })
      .finally(() => {
        if (active) setPending(null);
      });
    let unlisten: (() => void) | undefined;
    void actions
      .onProgress((pct) => {
        if (active) setProgress(pct);
      })
      .then((stop) => {
        if (active) unlisten = stop;
        else stop();
      });
    return () => {
      active = false;
      unlisten?.();
    };
  }, [actions]);

  const run = async (
    kind: "save" | "remove",
    call: () => Promise<MapTestSettings>,
  ) => {
    setPending(kind);
    setError(undefined);
    setProgress(null);
    try {
      setState(await call());
    } catch (caught) {
      setError(errorText(caught));
      try {
        setState(await actions.load());
      } catch {
        // The error above already tells the user what failed.
      }
    } finally {
      setPending(null);
      setProgress(null);
    }
  };

  const busy = disabled || pending !== null || state?.installing === true;
  const megabytes = Math.round((state?.downloadBytes ?? 0) / (1024 * 1024));

  return (
    <section
      aria-labelledby="settings-map-test-heading"
      className="mt-4 rounded-xl border border-border bg-card/40 px-4 py-3.5"
    >
      <div className="flex min-h-11 items-center justify-between gap-4">
        <span className="min-w-0">
          <span
            id="settings-map-test-heading"
            className="block text-sm font-medium text-foreground"
          >
            {t("settings.mapTest.title")}
          </span>
          <span className="mt-0.5 block text-xs leading-5 text-muted-foreground">
            {t("settings.mapTest.description")}
          </span>
        </span>
        <Switch
          checked={state?.enabled ?? false}
          disabled={busy || !state?.supported}
          aria-label={t("settings.mapTest.toggle")}
          onCheckedChange={(checked) =>
            void run("save", () => actions.save(checked))
          }
        />
      </div>
      {state && !state.supported && (
        <p className="mt-2 text-xs leading-5 text-muted-foreground">
          {t("settings.mapTest.unsupported")}
        </p>
      )}
      {state?.supported && !state.outdated && (
        <p className="mt-2 text-xs leading-5 text-muted-foreground">
          {state.installed
            ? t("settings.mapTest.installed", { bundle: state.bundle })
            : t("settings.mapTest.notInstalled", { size: megabytes })}
        </p>
      )}
      {state?.supported && state.outdated && (
        <div className="mt-2 flex flex-wrap items-center justify-between gap-3">
          <p className="min-w-0 text-xs leading-5 text-foreground">
            {t("settings.mapTest.outdated", {
              bundle: state.bundle,
              size: megabytes,
            })}
          </p>
          <Button
            type="button"
            className="min-h-11"
            disabled={busy}
            onClick={() => void run("save", () => actions.save(true))}
          >
            {t("settings.mapTest.update")}
          </Button>
        </div>
      )}
      {pending === "save" && progress !== null && (
        <div className="mt-3" role="status" aria-live="polite">
          <Progress value={progress} aria-label={t("settings.mapTest.downloading")} />
          <p className="mt-1 text-xs text-muted-foreground">
            {t("settings.mapTest.downloadingPct", { pct: progress })}
          </p>
        </div>
      )}
      {state?.installed && (
        <div className="mt-3 flex justify-end">
          <Button
            type="button"
            variant="outline"
            className="min-h-11"
            disabled={busy}
            onClick={() => void run("remove", () => actions.remove())}
          >
            {t("settings.mapTest.remove")}
          </Button>
        </div>
      )}
      {error && (
        <p role="alert" className="mt-2 text-xs leading-5 text-destructive">
          {error}
        </p>
      )}
    </section>
  );
}
