/**
 * First-run setup overlay (EUD-120).
 *
 * Rendered while bootstrap downloads/verifies first-run assets. Error mode
 * swaps progress for a retry button; progress mode is determinate when a
 * percent is available and indeterminate otherwise.
 */
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import type { BootstrapView } from "@/setup/bootstrap";

export interface SetupScreenProps {
  view: BootstrapView;
  error: string | null;
  onRetry: () => void;
}

export function SetupScreen({ view, error, onRetry }: SetupScreenProps) {
  const errorText = error?.trim() || (view.phase === "error" ? view.label : "");
  const errorMode = errorText.length > 0 || view.phase === "error";
  const determinate = view.pct !== null;
  const pct = view.pct === null ? undefined : view.pct;

  return (
    <main
      role="dialog"
      aria-label="최초 실행 설정"
      className="flex h-screen flex-col items-center justify-center gap-5 bg-background px-6 text-foreground"
    >
      <div className="flex w-full max-w-md flex-col gap-4">
        <div className="grid gap-1">
          <h1 className="text-xl font-semibold">최초 실행 설정</h1>
          <p className="text-sm text-muted-foreground">
            필요한 파일을 준비하고 있습니다.
          </p>
        </div>

        {errorMode ? (
          <div className="grid gap-4">
            <p className="rounded border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
              {errorText || view.label}
            </p>
            <Button type="button" className="w-fit" onClick={onRetry}>
              다시 시도
            </Button>
          </div>
        ) : (
          <div className="grid gap-2">
            <div
              role="progressbar"
              aria-valuenow={pct}
              aria-valuemin={determinate ? 0 : undefined}
              aria-valuemax={determinate ? 100 : undefined}
              aria-busy={determinate ? undefined : true}
              className="h-2 overflow-hidden rounded-full bg-muted"
            >
              <div
                className={cn(
                  "h-full rounded-full bg-primary transition-all",
                  !determinate && "w-1/3 animate-pulse",
                )}
                style={determinate ? { width: `${view.pct}%` } : undefined}
              />
            </div>
            <p className="text-sm text-muted-foreground">{view.label}</p>
          </div>
        )}
      </div>
    </main>
  );
}
