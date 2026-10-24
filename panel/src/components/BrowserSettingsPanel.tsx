import { useState } from "react";
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
  return "로그인 데이터를 지우지 못했습니다. 브라우저 창을 닫은 뒤 다시 시도하세요.";
}

export function BrowserSettingsPanel({
  actions,
  supported,
  disabled = false,
}: BrowserSettingsPanelProps) {
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
      <h2 className="text-base font-semibold">내장 브라우저</h2>
      <p className="mt-1.5 text-sm leading-6 text-muted-foreground">
        에이전트가 웹 페이지를 읽고 효과음 생성 같은 사이트를 쓸 때 쓰는 앱 전용
        브라우저입니다.
      </p>

      {supported ? (
        <>
          <section
            aria-labelledby="settings-browser-about-heading"
            className="mt-5 rounded-xl border border-border bg-card/40 px-4 py-3.5"
          >
            <h3 id="settings-browser-about-heading" className="text-sm font-medium">
              브라우저 창
            </h3>
            <ul className="mt-2 list-disc space-y-1 pl-5 text-xs leading-5 text-muted-foreground">
              <li>
                에이전트는 앱의 별도 &lsquo;브라우저&rsquo; 창에서 세션마다 자기 탭을 열어
                조작합니다. 같은 창에서 직접 보고 조작할 수 있습니다.
              </li>
              <li>
                그 창에서 로그인한 사이트는 모든 프로젝트와 세션이 함께 쓰고, 앱을 다시 열어도
                유지됩니다.
              </li>
              <li>
                비밀번호와 결제 정보는 에이전트가 입력하지 않습니다. 로그인은 브라우저 창에서
                직접 해 주세요.
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
                  로그인 데이터 초기화
                </h3>
                <p className="mt-1 text-xs leading-5 text-muted-foreground">
                  모든 사이트에서 로그아웃하고 브라우저가 받은 파일을 지웁니다. 브라우저 창이
                  닫혀 있을 때만 할 수 있습니다.
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
                {resetting ? "지우는 중…" : "로그인 데이터 초기화"}
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
                로그인 데이터를 지웠습니다. 다음에 브라우저 창이 열리면 다시 로그인해야 합니다.
              </p>
            )}
          </section>

          <Dialog open={confirming} onOpenChange={setConfirming}>
            <DialogContent className="sm:max-w-md">
              <DialogHeader>
                <DialogTitle>로그인 데이터를 초기화할까요?</DialogTitle>
                <DialogDescription>
                  내장 브라우저에서 로그인한 모든 사이트에서 로그아웃되고, 브라우저가 받은 파일도
                  지워집니다. 모든 프로젝트와 세션에 적용되며 되돌릴 수 없습니다.
                </DialogDescription>
              </DialogHeader>
              <DialogFooter>
                <DialogClose asChild>
                  <Button type="button" variant="outline" className="h-11">
                    취소
                  </Button>
                </DialogClose>
                <Button
                  type="button"
                  variant="destructive"
                  className="h-11"
                  onClick={() => void reset()}
                >
                  초기화
                </Button>
              </DialogFooter>
            </DialogContent>
          </Dialog>
        </>
      ) : (
        <Alert className="mt-5">
          <Info aria-hidden />
          <AlertTitle>내장 브라우저는 Windows에서만 쓸 수 있습니다</AlertTitle>
          <AlertDescription>
            이 플랫폼에서는 에이전트에게 브라우저 도구가 제공되지 않습니다. 웹에서 받은 파일은
            프로젝트 폴더에 넣거나 메시지에 첨부해 주세요.
          </AlertDescription>
        </Alert>
      )}
    </div>
  );
}
