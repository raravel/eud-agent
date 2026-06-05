/**
 * Changeset review (features/06 ## UI layout + Behaviors). Renders the
 * server-assembled `changeset.items[]`:
 *   - dat per objId: "{dat} [{objId}] {name?}" header + property old → new rows;
 *   - files by kind: created → content preview, modified → the SERVER unified
 *     diff with +/- coloring (NEVER Monaco DiffEditor — rules.md), deleted →
 *     name row;
 *   - settings/plugins/main and any other flat item → old → new rows.
 *
 * Each item exposes [✓ 적용]/[✗ 되돌리기]; bulk [전체 적용 유지]/[전체 되돌리기]
 * dispatch the literal "all". Decisions flow through `onDecide(decision, ids)`
 * (the App fires the WS `changeset_decision`; the store records it so the
 * inbound `rollback_result` is labelled per accept/reject). The per-item ids
 * come from {@link itemIds} (a dat group targets every property id). Resolved
 * rows show 적용 유지 / 되돌림 / 실패 (inline failure) from the store decisions.
 *
 * Diff/preview limits reuse lib/truncate (1 MiB UTF-16-consistent). Korean labels.
 */
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Spinner } from "@/components/ui/spinner";
import { cn } from "@/lib/utils";
import { classifyDiff } from "@/lib/diff";
import { truncateForDisplay } from "@/lib/truncate";
import {
  datProperties,
  itemIds,
  itemKey,
  itemState,
  type ItemState,
} from "@/lib/changeset";
import type { ChangesetState } from "@/state/store";
import type { ChangesetItem } from "@/ws/protocol";

export interface ChangesetViewProps {
  /** The active changeset under review (items + per-id decisions). */
  changeset: ChangesetState;
  /** A decision is in flight (disable the controls until rollback_result). */
  pending: boolean;
  /** Fire the changeset_decision; ids "all" for bulk, else the item's ids. */
  onDecide(decision: "accept" | "reject", ids: "all" | string[]): void;
}

/** Per-state Korean label + tone for the resolved row badge. */
const STATE_BADGE: Record<
  Exclude<ItemState, "undecided">,
  { label: string; tone: string }
> = {
  accepted: { label: "적용 유지", tone: "text-emerald-400" },
  rejected: { label: "되돌림", tone: "text-muted-foreground" },
  failed: { label: "되돌리기 실패", tone: "text-destructive" },
  mixed: { label: "일부 적용", tone: "text-amber-400" },
};

function asText(value: unknown): string {
  if (value === null || value === undefined) return "";
  if (typeof value === "object") return JSON.stringify(value);
  return String(value);
}

/** A single "old → new" row. */
function OldNewRow({ label, old, next }: { label: string; old: unknown; next: unknown }) {
  return (
    <div className="flex flex-wrap items-center gap-1 text-sm">
      <span className="font-medium">{label}</span>
      <span className="text-muted-foreground line-through">{asText(old)}</span>
      <span className="text-muted-foreground">→</span>
      <span className="text-emerald-400">{asText(next)}</span>
    </div>
  );
}

/** Server unified diff rendered with +/- coloring (no Monaco DiffEditor). */
function DiffBlock({ diff }: { diff: string }) {
  const { text, truncated } = truncateForDisplay(diff);
  const lines = classifyDiff(text);
  return (
    <div>
      <pre className="overflow-x-auto rounded bg-muted/40 p-2 text-xs">
        {lines.map((ln, i) => (
          <div
            key={i}
            data-diff={ln.kind}
            className={cn(
              "whitespace-pre-wrap break-words",
              ln.kind === "add" && "text-emerald-400",
              ln.kind === "del" && "text-destructive",
              ln.kind === "hunk" && "text-sky-400",
              ln.kind === "file" && "text-muted-foreground",
            )}
          >
            {ln.text || " "}
          </div>
        ))}
      </pre>
      {truncated && (
        <p className="text-xs text-amber-400">표시가 1 MiB에서 잘렸습니다.</p>
      )}
    </div>
  );
}

/** Content preview for a created file (truncated for display). */
function ContentPreview({ content }: { content: string }) {
  const { text, truncated } = truncateForDisplay(content);
  return (
    <div>
      <pre className="overflow-x-auto rounded bg-muted/40 p-2 text-xs whitespace-pre-wrap break-words">
        {text}
      </pre>
      {truncated && (
        <p className="text-xs text-amber-400">표시가 1 MiB에서 잘렸습니다.</p>
      )}
    </div>
  );
}

/** The body of one changeset item, by category/kind. */
function ItemBody({ item }: { item: ChangesetItem }) {
  if (item.category === "dat") {
    const dat = asText(item.dat);
    const objId = asText(item.objId);
    const name = asText(item.name);
    return (
      <div className="flex flex-col gap-1">
        <div className="text-sm font-semibold">
          {dat} [{objId}]{name && <span className="ml-1 font-normal">{name}</span>}
        </div>
        {datProperties(item).map((p) => (
          <OldNewRow key={p.id} label={p.property} old={p.old} next={p.new} />
        ))}
      </div>
    );
  }

  if (item.category === "file") {
    const path = asText(item.path);
    const kind = asText(item.kind);
    if (kind === "created") {
      return (
        <div className="flex flex-col gap-1">
          <div className="text-sm">
            <span className="text-emerald-400">+생성</span> {path}
          </div>
          {typeof item.content === "string" && (
            <ContentPreview content={item.content} />
          )}
        </div>
      );
    }
    if (kind === "deleted") {
      return (
        <div className="text-sm">
          <span className="text-destructive">-삭제</span> {path}
        </div>
      );
    }
    // modified
    return (
      <div className="flex flex-col gap-1">
        <div className="text-sm">
          <span className="text-amber-400">~수정</span> {path}
        </div>
        {typeof item.diff === "string" && item.diff !== "" && (
          <DiffBlock diff={item.diff} />
        )}
      </div>
    );
  }

  // flat (settings / plugins / main / tbl / req / btn): old → new row.
  return (
    <OldNewRow
      label={asText(item.target) || item.category}
      old={item.old}
      next={item.new}
    />
  );
}

export function ChangesetView({ changeset, pending, onDecide }: ChangesetViewProps) {
  const { items, decisions } = changeset;

  return (
    <section
      aria-label="변경사항 검토"
      className="flex max-h-[40vh] flex-col gap-3 overflow-y-auto border-t border-border p-4"
    >
      {items.map((item) => {
        const state = itemState(item, decisions);
        const ids = itemIds(item);
        const decided = state !== "undecided";
        // Stable identity for keying + testid. A dat group has no item-level
        // id, so itemKey falls back to the joined property ids (NEVER undefined).
        const key = itemKey(item);
        return (
          <Card
            key={key}
            data-testid={`cs-item-${key}`}
            className="gap-2 py-2 shadow-none"
          >
            <CardContent className="flex flex-col gap-2 px-3">
              <ItemBody item={item} />
              <div className="flex items-center justify-end gap-2">
              {decided ? (
                <Badge
                  variant="outline"
                  className={cn("text-xs font-medium", STATE_BADGE[state].tone)}
                >
                  {STATE_BADGE[state].label}
                </Badge>
              ) : (
                <>
                  <Button
                    type="button"
                    size="xs"
                    variant="outline"
                    disabled={pending}
                    aria-label="적용 유지"
                    onClick={() => onDecide("accept", ids)}
                  >
                    ✓ 적용
                  </Button>
                  <Button
                    type="button"
                    size="xs"
                    variant="outline"
                    disabled={pending}
                    aria-label="되돌리기"
                    onClick={() => onDecide("reject", ids)}
                  >
                    ✗ 되돌리기
                  </Button>
                </>
              )}
              </div>
            </CardContent>
          </Card>
        );
      })}

      {/* EUD-070: in-flight notice — a rollback waits on the 1s bridge tick per
          inverse op (2-4s for a dat group), so the wait must be visible, not
          just silently-disabled buttons. */}
      {pending && (
        <div className="flex items-center gap-2 text-sm text-muted-foreground">
          <Spinner className="size-3.5 shrink-0" />
          <span>결정 처리 중… (되돌리기는 에디터에 한 건씩 적용됩니다)</span>
        </div>
      )}

      <div className="flex items-center justify-end gap-2 border-t border-border pt-2">
        <Button
          type="button"
          size="sm"
          disabled={pending}
          onClick={() => onDecide("accept", "all")}
        >
          전체 적용 유지
        </Button>
        <Button
          type="button"
          size="sm"
          variant="outline"
          disabled={pending}
          onClick={() => onDecide("reject", "all")}
        >
          전체 되돌리기
        </Button>
      </div>
    </section>
  );
}
