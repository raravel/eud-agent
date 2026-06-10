/**
 * First-run setup overlay (EUD-120, EUD-132).
 *
 * Two steps, mirroring feature 10's boot flow:
 *  1. editor-path pick — shown while the configured editor path is missing or
 *     invalid; the button opens the native folder picker via the backend.
 *  2. asset download — rendered while bootstrap downloads/verifies first-run
 *     assets. Error mode swaps progress for a retry button; progress mode is
 *     determinate when a percent is available and indeterminate otherwise.
 */
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import type { BootstrapView } from "@/setup/bootstrap";

/** User-facing text for stable backend pick-error codes (never rendered raw). */
const PICK_ERROR_TEXT: Record<string, string> = {
  invalid_editor_folder:
    "선택한 폴더에서 EUD Editor 3을 찾지 못했습니다. Data\\Lua\\TriggerEditor 폴더가 있는 설치 폴더를 선택해 주세요.",
};

export interface SetupScreenProps {
  /** False while the editor install folder still needs to be picked. */
  editorValid: boolean;
  /** Stable error code from a rejected folder pick (null when none). */
  pickError: string | null;
  /** Open the native folder picker (backend validates + persists). */
  onPick: () => void;
  view: BootstrapView;
  error: string | null;
  onRetry: () => void;
}

export function SetupScreen({
  editorValid,
  pickError,
  onPick,
  view,
  error,
  onRetry,
}: SetupScreenProps) {
  const errorText = error?.trim() || (view.phase === "error" ? view.label : "");
  const errorMode = errorText.length > 0 || view.phase === "error";
  const determinate = view.pct !== null;
  const pct = view.pct === null ? undefined : view.pct;
  const pickMode = !editorValid;

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
            {pickMode
              ? "EUD Editor 3 설치 폴더를 선택해 주세요."
              : "필요한 파일을 준비하고 있습니다."}
          </p>
        </div>

        {pickMode ? (
          <div className="grid gap-4">
            {pickError !== null && (
              <p className="rounded border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
                {PICK_ERROR_TEXT[pickError] ??
                  "에디터 폴더를 설정하지 못했습니다. 다시 시도해 주세요."}
              </p>
            )}
            <Button type="button" className="w-fit" onClick={onPick}>
              에디터 폴더 선택
            </Button>
          </div>
        ) : errorMode ? (
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
