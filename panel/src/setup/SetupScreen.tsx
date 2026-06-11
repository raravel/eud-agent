/**
 * First-run setup overlay (EUD-120, EUD-132).
 *
 * Two steps, mirroring feature 10's boot flow, rendered as a centered card with
 * a step indicator (1 에디터 폴더 → 2 에셋 다운로드):
 *  1. editor-path pick — shown while the configured editor path is missing or
 *     invalid; the button opens the native folder picker via the backend.
 *  2. asset download — rendered while bootstrap downloads/verifies first-run
 *     assets. Error mode swaps progress for a retry button; progress mode is
 *     determinate when a percent is available and indeterminate otherwise.
 *
 * Styling stays inside the panel's shadcn token system (dark theme, emerald
 * accent matching the Header status pills); icons are bundled lucide SVGs and
 * animations respect prefers-reduced-motion (rules.md: no CDN assets).
 */
import { useState, type ReactNode } from "react";
import {
  CheckIcon,
  CircleAlertIcon,
  DownloadIcon,
  FolderOpenIcon,
  KeyRoundIcon,
  Loader2Icon,
  LogInIcon,
  SparklesIcon,
} from "lucide-react";

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
  /** True once the model + RAG index are verified (drives the codex step). */
  assetsReady?: boolean;
  /** True when the codex CLI was found (PATH / CODEX_CMD). */
  codexResolved?: boolean;
  /** True once `codex login status` reports a logged-in session. */
  codexAuthed?: boolean;
  /** A login attempt is in flight (OAuth poll or API-key submit). */
  codexBusy?: boolean;
  /** Last login/install error, already mapped to user-facing text. */
  codexError?: string | null;
  /** Download + install the codex binary (backend), shown when not resolved. */
  onCodexInstall?: () => void;
  /** Launch the ChatGPT OAuth login (backend opens the browser). */
  onCodexOAuth?: () => void;
  /** Log in with an API key (passed to the backend over stdin). */
  onCodexApiKey?: (key: string) => void;
}

/** One entry of the two-step indicator. */
function Step({
  index,
  label,
  state,
}: {
  index: number;
  label: string;
  state: "done" | "current" | "pending";
}) {
  return (
    <li
      aria-current={state === "current" ? "step" : undefined}
      className="flex items-center gap-2"
    >
      <span
        className={cn(
          "flex size-7 shrink-0 items-center justify-center rounded-full border text-xs font-semibold transition-colors duration-300",
          state === "done" &&
            "border-emerald-500/40 bg-emerald-500/15 text-emerald-400",
          state === "current" &&
            "border-transparent bg-primary text-primary-foreground",
          state === "pending" && "border-border bg-muted text-muted-foreground",
        )}
      >
        {state === "done" ? <CheckIcon aria-hidden className="size-3.5" /> : index}
      </span>
      <span
        className={cn(
          "text-sm",
          state === "current"
            ? "font-medium text-foreground"
            : "text-muted-foreground",
        )}
      >
        {label}
      </span>
    </li>
  );
}

/** Inline destructive alert used by both the pick and download error states. */
function ErrorNotice({ children }: { children: ReactNode }) {
  return (
    <p className="flex items-start gap-2 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
      <CircleAlertIcon aria-hidden className="mt-0.5 size-4 shrink-0" />
      <span>{children}</span>
    </p>
  );
}

/** Step 3: codex login. ChatGPT OAuth (launches the browser) or an API key. */
function CodexStep({
  resolved,
  busy,
  error,
  onInstall,
  onOAuth,
  onApiKey,
}: {
  resolved: boolean;
  busy: boolean;
  error: string | null;
  onInstall?: () => void;
  onOAuth?: () => void;
  onApiKey?: (key: string) => void;
}) {
  const [apiKey, setApiKey] = useState("");

  // codex is not installed yet: the app downloads and installs it (no manual
  // step). Resolution flips on success and the login controls take over.
  if (!resolved) {
    return (
      <div className="grid gap-4">
        <div className="flex items-start gap-3 rounded-lg border border-dashed border-border bg-muted/30 p-4">
          <DownloadIcon
            aria-hidden
            className="mt-0.5 size-5 shrink-0 text-muted-foreground"
          />
          <div className="grid gap-1">
            <p className="text-sm">codex CLI가 필요합니다. 앱이 자동으로 설치합니다.</p>
            <p className="text-xs text-muted-foreground">
              최신 codex 실행 파일(수십 MB)을 내려받아 설치합니다.
            </p>
          </div>
        </div>
        {error !== null && <ErrorNotice>{error}</ErrorNotice>}
        <Button
          type="button"
          size="lg"
          className="w-full"
          onClick={onInstall}
          disabled={busy}
        >
          {busy ? (
            <Loader2Icon
              aria-hidden
              className="size-4 animate-spin motion-reduce:animate-none"
            />
          ) : (
            <DownloadIcon aria-hidden />
          )}
          {busy ? "codex 설치 중…" : "codex 설치"}
        </Button>
      </div>
    );
  }

  const trimmed = apiKey.trim();

  return (
    <div className="grid gap-4">
      <div className="flex items-start gap-3 rounded-lg border border-dashed border-border bg-muted/30 p-4">
        <LogInIcon
          aria-hidden
          className="mt-0.5 size-5 shrink-0 text-muted-foreground"
        />
        <div className="grid gap-1">
          <p className="text-sm">에이전트 실행에 사용할 codex 로그인이 필요합니다.</p>
          <p className="text-xs text-muted-foreground">
            ChatGPT로 로그인하면 브라우저 창이 열립니다. 인증을 마치면 자동으로
            진행됩니다.
          </p>
        </div>
      </div>

      {error !== null && <ErrorNotice>{error}</ErrorNotice>}

      <Button
        type="button"
        size="lg"
        className="w-full"
        onClick={onOAuth}
        disabled={busy}
      >
        {busy ? (
          <Loader2Icon
            aria-hidden
            className="size-4 animate-spin motion-reduce:animate-none"
          />
        ) : (
          <LogInIcon aria-hidden />
        )}
        {busy ? "로그인 진행 중…" : "ChatGPT로 로그인"}
      </Button>

      <div className="flex items-center gap-3 text-xs text-muted-foreground">
        <span className="h-px flex-1 bg-border" />
        또는 API 키로 로그인
        <span className="h-px flex-1 bg-border" />
      </div>

      <form
        className="grid gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          if (trimmed.length > 0) onApiKey?.(trimmed);
        }}
      >
        <input
          type="password"
          value={apiKey}
          onChange={(event) => setApiKey(event.target.value)}
          placeholder="OpenAI API 키 (sk-…)"
          disabled={busy}
          aria-label="OpenAI API 키"
          className="h-9 rounded-md border border-border bg-background px-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50"
        />
        <Button
          type="submit"
          variant="secondary"
          className="w-full"
          disabled={busy || trimmed.length === 0}
        >
          <KeyRoundIcon aria-hidden />
          API 키로 로그인
        </Button>
      </form>
    </div>
  );
}

export function SetupScreen({
  editorValid,
  pickError,
  onPick,
  view,
  error,
  onRetry,
  assetsReady = false,
  codexResolved = true,
  codexAuthed = true,
  codexBusy = false,
  codexError = null,
  onCodexInstall,
  onCodexOAuth,
  onCodexApiKey,
}: SetupScreenProps) {
  const errorText = error?.trim() || (view.phase === "error" ? view.label : "");
  const errorMode = errorText.length > 0 || view.phase === "error";
  const determinate = view.pct !== null;
  const pct = view.pct === null ? undefined : view.pct;
  const pickMode = !editorValid;
  // The codex login step runs LAST: editor picked + assets verified, but codex
  // is not yet logged in. Until then the download step owns the non-pick screen.
  const codexMode = !pickMode && assetsReady && !codexAuthed;

  return (
    <main
      role="dialog"
      aria-label="최초 실행 설정"
      className="relative flex h-screen flex-col items-center justify-center bg-background px-6 text-foreground"
    >
      {/* Ambient backdrop glow — decorative only, token/accent driven. */}
      <div
        aria-hidden
        className="pointer-events-none absolute inset-0 overflow-hidden"
      >
        <div className="absolute -top-40 left-1/2 size-80 -translate-x-1/2 rounded-full bg-emerald-500/10 blur-3xl" />
        <div className="absolute -right-20 bottom-0 size-64 rounded-full bg-primary/5 blur-3xl" />
      </div>

      <div className="relative w-full max-w-md animate-in fade-in zoom-in-95 duration-300 motion-reduce:animate-none">
        <div className="flex flex-col gap-6 rounded-xl border border-border bg-card/80 p-8 shadow-2xl backdrop-blur">
          {/* Branding + title */}
          <div className="flex items-center gap-3">
            <span className="flex size-11 shrink-0 items-center justify-center rounded-xl border border-emerald-500/30 bg-emerald-500/15 text-emerald-400">
              <SparklesIcon aria-hidden className="size-5" />
            </span>
            <div className="grid gap-0.5">
              <p className="text-xs font-medium tracking-wide text-muted-foreground">
                EUD 에이전트
              </p>
              <h1 className="text-xl font-semibold">최초 실행 설정</h1>
            </div>
          </div>

          {/* Step indicator */}
          <ol className="flex items-center gap-3">
            <Step
              index={1}
              label="에디터 폴더"
              state={pickMode ? "current" : "done"}
            />
            <span aria-hidden className="h-px min-w-6 flex-1 bg-border" />
            <Step
              index={2}
              label="에셋 다운로드"
              state={pickMode ? "pending" : assetsReady ? "done" : "current"}
            />
            <span aria-hidden className="h-px min-w-6 flex-1 bg-border" />
            <Step
              index={3}
              label="codex 로그인"
              state={codexMode ? "current" : codexAuthed ? "done" : "pending"}
            />
          </ol>

          {pickMode ? (
            <div className="grid gap-4">
              <div className="flex items-start gap-3 rounded-lg border border-dashed border-border bg-muted/30 p-4">
                <FolderOpenIcon
                  aria-hidden
                  className="mt-0.5 size-5 shrink-0 text-muted-foreground"
                />
                <div className="grid gap-1">
                  <p className="text-sm">
                    EUD Editor 3 설치 폴더를 선택해 주세요.
                  </p>
                  <p className="text-xs text-muted-foreground">
                    Data\Lua\TriggerEditor 폴더가 들어 있는 위치입니다.
                  </p>
                </div>
              </div>
              {pickError !== null && (
                <ErrorNotice>
                  {PICK_ERROR_TEXT[pickError] ??
                    "에디터 폴더를 설정하지 못했습니다. 다시 시도해 주세요."}
                </ErrorNotice>
              )}
              <Button type="button" size="lg" className="w-full" onClick={onPick}>
                <FolderOpenIcon aria-hidden />
                에디터 폴더 선택
              </Button>
            </div>
          ) : codexMode ? (
            <CodexStep
              resolved={codexResolved}
              busy={codexBusy}
              error={codexError}
              onInstall={onCodexInstall}
              onOAuth={onCodexOAuth}
              onApiKey={onCodexApiKey}
            />
          ) : errorMode ? (
            <div className="grid gap-4">
              <ErrorNotice>{errorText || view.label}</ErrorNotice>
              <Button type="button" className="w-fit" onClick={onRetry}>
                <DownloadIcon aria-hidden />
                다시 시도
              </Button>
            </div>
          ) : (
            <div className="grid gap-3">
              <div className="flex items-baseline justify-between gap-2">
                <span className="flex items-center gap-2 text-sm text-muted-foreground">
                  <Loader2Icon
                    aria-hidden
                    className="size-3.5 animate-spin motion-reduce:animate-none"
                  />
                  {view.label}
                </span>
                {determinate && (
                  <span className="text-sm font-semibold tabular-nums text-emerald-400">
                    {view.pct}%
                  </span>
                )}
              </div>
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
                    "h-full rounded-full bg-emerald-500 transition-[width] duration-300",
                    !determinate &&
                      "w-1/3 animate-pulse motion-reduce:animate-none",
                  )}
                  style={determinate ? { width: `${view.pct}%` } : undefined}
                />
              </div>
            </div>
          )}

          {/* Footer reassurance — install is one-time and integrity-checked. */}
          <p className="border-t border-border pt-4 text-center text-xs text-muted-foreground">
            이 설정은 최초 1회만 진행되며, 모든 파일은 무결성 검증(sha256) 후
            설치됩니다.
          </p>
        </div>
      </div>
    </main>
  );
}
