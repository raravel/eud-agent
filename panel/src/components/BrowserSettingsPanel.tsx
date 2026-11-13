import { useState } from "react";
import { useTranslation } from "react-i18next";
import { CheckCircle2, Info, LoaderCircle, Trash2 } from "lucide-react";

import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import i18n from "@/lib/i18n";

/** Backend calls behind Settings → 브라우저 (see `browserProfileReset` in ipc). */
export interface BrowserSettingsActions {
  /** Delete the built-in browser's profile and downloads; refused while its window is open. */
  resetProfile(): Promise<void>;
}

export interface BrowserSettingsPanelProps {
  actions: BrowserSettingsActions;
  /** The built-in browser exists on Windows only. */
  supported: boolean;
  disabled?: boolean;
}

function errorText(error: unknown): string {
  if (typeof error === "string" && error.trim()) return error;
  if (error instanceof Error && error.message.trim()) return error.message;
  return i18n.t("settings.browser.resetFailed");
}

export function BrowserSettingsPanel({
  actions,
  supported,
  disabled = false,
}: BrowserSettingsPanelProps) {
  const { t } = useTranslation();
  const [confirming, setConfirming] = useState(false);
  const [resetting, setResetting] = useState(false);
  const [error, setError] = useState<string>();
  const [done, setDone] = useState(false);

  const reset = async () => {
    setConfirming(false);
    setResetting(true);
    setError(undefined);
    setDone(false);
    try {
      await actions.resetProfile();
      setDone(true);
    } catch (caught) {
      setError(errorText(caught));
    } finally {
      setResetting(false);
    }
  };

  return (
    <div className="animate-in fade-in duration-200 motion-reduce:animate-none">
      <h2 className="text-base font-semibold">{t("settings.browser.title")}</h2>
      <p className="mt-1.5 text-sm leading-6 text-muted-foreground">
        {t("settings.browser.description")}
      </p>

      {supported ? (
        <>
          <section
            aria-labelledby="settings-browser-about-heading"
            className="mt-5 rounded-xl border border-border bg-card/40 px-4 py-3.5"
          >
            <h3 id="settings-browser-about-heading" className="text-sm font-medium">
              {t("settings.browser.windowHeading")}
            </h3>
            <ul className="mt-2 list-disc space-y-1 pl-5 text-xs leading-5 text-muted-foreground">
              <li>
                {t("settings.browser.about1")}
              </li>
              <li>
                {t("settings.browser.about2")}
              </li>
              <li>
                {t("settings.browser.about3")}
              </li>
            </ul>
          </section>

          <section
            aria-labelledby="settings-browser-reset-heading"
            className="mt-4 rounded-xl border border-border bg-card/40 px-4 py-3.5"
          >
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div className="min-w-0 flex-1">
                <h3 id="settings-browser-reset-heading" className="text-sm font-medium">
                  {t("settings.browser.resetHeading")}
                </h3>
                <p className="mt-1 text-xs leading-5 text-muted-foreground">
                  {t("settings.browser.resetDescription")}
                </p>
              </div>
              <Button
                type="button"
                variant="destructive"
                className="h-11 shrink-0 gap-1.5"
                disabled={disabled || resetting}
                onClick={() => setConfirming(true)}
              >
                {resetting ? (
                  <LoaderCircle
                    aria-hidden
                    className="size-4 animate-spin motion-reduce:animate-none"
                  />
                ) : (
                  <Trash2 aria-hidden className="size-4" />
                )}
                {resetting ? t("settings.browser.resetting") : t("settings.browser.resetHeading")}
              </Button>
            </div>
            {error && (
              <p role="alert" className="mt-3 text-xs leading-5 text-destructive">
                {error}
              </p>
            )}
            {done && (
              <p
                role="status"
                className="mt-3 flex items-center gap-1.5 text-xs leading-5 text-muted-foreground"
              >
                <CheckCircle2 aria-hidden className="size-3.5 text-primary" />
                {t("settings.browser.resetDone")}
              </p>
            )}
          </section>

          <Dialog open={confirming} onOpenChange={setConfirming}>
            <DialogContent className="sm:max-w-md">
              <DialogHeader>
                <DialogTitle>{t("settings.browser.confirmTitle")}</DialogTitle>
                <DialogDescription>
                  {t("settings.browser.confirmDescription")}
                </DialogDescription>
              </DialogHeader>
              <DialogFooter>
                <DialogClose asChild>
                  <Button type="button" variant="outline" className="h-11">
                    {t("settings.browser.cancel")}
                  </Button>
                </DialogClose>
                <Button
                  type="button"
                  variant="destructive"
                  className="h-11"
                  onClick={() => void reset()}
                >
                  {t("settings.browser.confirmAction")}
                </Button>
              </DialogFooter>
            </DialogContent>
          </Dialog>
        </>
      ) : (
        <Alert className="mt-5">
          <Info aria-hidden />
          <AlertTitle>{t("settings.browser.unsupportedTitle")}</AlertTitle>
          <AlertDescription>
            {t("settings.browser.unsupportedDescription")}
          </AlertDescription>
        </Alert>
      )}
    </div>
  );
}
