/**
 * Target picker (features/03 ## Behaviors → Target picker):
 *   - file list fed by the store's `files`.
 *   - non-settable (GUI) options disabled with tooltip "읽기 전용 파일 형식".
 *   - refresh button.
 *   - no project (hasProject false) → placeholder "프로젝트를 열어주세요".
 *   - new-file toggle + NEWEPS filename input with inline validation error.
 *
 * Implementation note: the file chooser is an accessible custom listbox (not the
 * Radix Select portal). The Radix Select mounts its options in a portal only
 * while open, which both (a) makes the options untestable headless and (b) has
 * known pointer-capture friction in the WebView2/Chromium host. A flat listbox
 * styled with the shadcn control classes renders every option eagerly, keeps
 * disabled GUI rows non-selectable, and carries the read-only tooltip via the
 * native `title` — same UX, no portal.
 */
import { RefreshCwIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";
import { validateNewEpsName, type PanelState } from "@/state/store";

export interface TargetPickerProps {
  state: PanelState;
  /** Raw NEWEPS filename (controlled by the parent for validation + payload). */
  newEpsName: string;
  onSelectTarget(path: string): void;
  onToggleNewFile(on: boolean): void;
  onRefresh(): void;
  onChangeNewEpsName(name: string): void;
}

const READ_ONLY_TOOLTIP = "읽기 전용 파일 형식";

export function TargetPicker({
  state,
  newEpsName,
  onSelectTarget,
  onToggleNewFile,
  onRefresh,
  onChangeNewEpsName,
}: TargetPickerProps) {
  const { hasProject, files, selectedTarget, newFileMode } = state;
  const nameValidation = validateNewEpsName(newEpsName);
  const nameError = newFileMode && !nameValidation.ok ? nameValidation.reason : null;

  return (
    <div className="flex flex-col gap-2 border-t border-border px-4 py-3">
      <div className="flex items-center gap-2">
        <span className="text-sm text-muted-foreground">대상</span>
        <div className="flex-1">
          {hasProject ? (
            <ul
              role="listbox"
              aria-label="대상 파일"
              className="max-h-40 overflow-y-auto rounded-md border border-input bg-input/30 p-1"
            >
              {files.map((f) => {
                const disabled = !f.settable;
                const selected = f.path === selectedTarget;
                return (
                  <li key={f.path}>
                    <button
                      type="button"
                      role="option"
                      aria-selected={selected}
                      aria-disabled={disabled ? "true" : undefined}
                      disabled={disabled}
                      title={disabled ? READ_ONLY_TOOLTIP : undefined}
                      data-testid={`target-option-${f.path}`}
                      onClick={() => {
                        if (!disabled) onSelectTarget(f.path);
                      }}
                      className={cn(
                        "flex w-full items-center justify-between gap-2 rounded-sm px-2 py-1 text-left text-sm",
                        disabled
                          ? "cursor-not-allowed opacity-50"
                          : "hover:bg-accent hover:text-accent-foreground",
                        selected && "bg-accent text-accent-foreground",
                      )}
                    >
                      <span className="truncate">{f.path}</span>
                      <span className="shrink-0 text-xs text-muted-foreground">
                        {f.ftype}
                      </span>
                    </button>
                  </li>
                );
              })}
            </ul>
          ) : (
            <div className="rounded-md border border-dashed border-input px-2 py-2 text-sm text-muted-foreground">
              프로젝트를 열어주세요
            </div>
          )}
        </div>
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={onRefresh}
          aria-label="새로고침"
        >
          <RefreshCwIcon className="size-4" />
        </Button>
      </div>

      <label className="flex w-fit items-center gap-2 text-sm">
        <Checkbox
          checked={newFileMode}
          onCheckedChange={(v) => onToggleNewFile(v === true)}
          aria-label="새 파일"
        />
        새 파일
      </label>

      {newFileMode && (
        <div className="flex flex-col gap-1">
          <label htmlFor="neweps-name" className="text-xs text-muted-foreground">
            새 파일 이름
          </label>
          <Input
            id="neweps-name"
            value={newEpsName}
            onChange={(e) => onChangeNewEpsName(e.target.value)}
            placeholder="예: my_trigger.eps"
            aria-invalid={nameError ? true : undefined}
          />
          {nameError && (
            <span className="text-xs text-destructive">{nameError}</span>
          )}
        </div>
      )}
    </div>
  );
}
