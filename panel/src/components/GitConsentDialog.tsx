/**
 * Repository consent.
 *
 * The app commits the project at every turn boundary, which is how 되돌리기
 * works. When the app created the repository itself that is unremarkable — but
 * when the project was ALREADY a git repository before the app touched it, the
 * history is the user's, and writing into it without asking would be taking
 * something that was not offered. The core reports that case as
 * `consent: "pending"` and commits nothing until it is answered; this dialog is
 * the question, asked once.
 */
import { useTranslation } from "react-i18next";
import { useState } from "react";
import { GitBranchIcon } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Spinner } from "@/components/ui/spinner";

export interface GitConsentDialogProps {
  /** True only while the core reports `consent: "pending"`. */
  open: boolean;
  /** `git_consent_set` — the answer is durable, so it is asked only once. */
  onDecide(granted: boolean): Promise<void>;
}

export function GitConsentDialog({ open, onDecide }: GitConsentDialogProps) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState<"grant" | "decline" | null>(null);
  const [error, setError] = useState<string | null>(null);

  const decide = async (granted: boolean) => {
    if (busy !== null) return;
    setBusy(granted ? "grant" : "decline");
    setError(null);
    try {
      await onDecide(granted);
    } catch (caught) {
      setError(
        caught instanceof Error ? caught.message : String(caught),
      );
    } finally {
      setBusy(null);
    }
  };

  return (
    // The question has no dismissal: closing it without an answer would leave
    // the app unable to record anything, with nothing on screen saying so.
    <Dialog open={open}>
      <DialogContent
        className="sm:max-w-lg"
        showCloseButton={false}
        onEscapeKeyDown={(event) => event.preventDefault()}
        onInteractOutside={(event) => event.preventDefault()}
      >
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <GitBranchIcon aria-hidden className="size-4 text-primary" />
            {t("workspace.gitConsent.title")}
          </DialogTitle>
          <DialogDescription>
            {t("workspace.gitConsent.description")}
          </DialogDescription>
        </DialogHeader>
        {error && (
          <p role="alert" className="text-sm text-destructive">
            {error}
          </p>
        )}
        <DialogFooter>
          <Button
            type="button"
            variant="outline"
            disabled={busy !== null}
            onClick={() => void decide(false)}
          >
            {busy === "decline" && (
              <Spinner
                aria-label={t("workspace.gitConsent.saving")}
                className="size-3.5 shrink-0 motion-reduce:animate-none"
              />
            )}
            {t("workspace.gitConsent.decline")}
          </Button>
          <Button
            type="button"
            disabled={busy !== null}
            onClick={() => void decide(true)}
          >
            {busy === "grant" && (
              <Spinner
                aria-label={t("workspace.gitConsent.saving")}
                className="size-3.5 shrink-0 motion-reduce:animate-none"
              />
            )}
            {t("workspace.gitConsent.grant")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
