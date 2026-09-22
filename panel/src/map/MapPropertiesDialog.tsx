import { useEffect, useId, useState } from "react";
import { CircleAlert, LoaderCircle, Save, SlidersHorizontal } from "lucide-react";

import { ForcesEditor } from "@/components/map-properties/ForcesEditor";
import { PlayerSlotsEditor } from "@/components/map-properties/PlayerSlotsEditor";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import {
  applyForceLayout,
  withForcePatch,
  withPlayerPatch,
  type ForceForm,
  type ForceLayout,
  type PlayerPatch,
} from "@/lib/mapSlots";
import {
  buildMapPropertiesRequest,
  propertiesFormEqual,
  propertiesFormFromDigest,
  validateMapProperties,
  type MapPropertiesForm,
} from "./mapProperties";
import type {
  CandidateStateView,
  MapContextSnapshot,
  MapPropertiesRequest,
} from "./mapProtocol";

export interface MapPropertiesDialogProps {
  open: boolean;
  context: MapContextSnapshot;
  candidate: CandidateStateView;
  busy?: boolean;
  onOpenChange(open: boolean): void;
  /** Rejects with the backend's Korean message; the dialog stays open and shows it. */
  onSave(properties: MapPropertiesRequest): Promise<void>;
}

const CANDIDATE_BLOCK_TEXT =
  "적용하지 않은 후보 revision이 있어 맵 속성을 저장할 수 없습니다. 먼저 적용하거나 폐기해 주세요.";
const STALE_BLOCK_TEXT =
  "원본 맵이 다시 저장되었습니다. 진행 중인 요청이 끝나고 저장된 원본을 반영한 뒤 다시 시도해 주세요.";
const DIGEST_MISSING_TEXT =
  "이 맵의 플레이어·포스 정보를 읽지 못했습니다. 맵을 다시 불러온 뒤 시도해 주세요.";

/** Edits scenario title, description, 12 slots and 4 forces; saving writes the source map. */
export function MapPropertiesDialog({
  open,
  context,
  candidate,
  busy = false,
  onOpenChange,
  onSave,
}: MapPropertiesDialogProps) {
  const idPrefix = useId();
  const [initial, setInitial] = useState<MapPropertiesForm | null>(null);
  const [form, setForm] = useState<MapPropertiesForm | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    const next = propertiesFormFromDigest(context.digest);
    setInitial(next);
    setForm(next);
    setSaving(false);
    setError(null);
  }, [open, context]);

  const blockedReason =
    form === null
      ? DIGEST_MISSING_TEXT
      : candidate.currentRevision !== 0
        ? CANDIDATE_BLOCK_TEXT
        : candidate.stale
          ? STALE_BLOCK_TEXT
          : null;
  const validation = form === null ? null : validateMapProperties(form);
  const changed = form !== null && initial !== null && !propertiesFormEqual(form, initial);
  const disabled = busy || saving || blockedReason !== null;
  const canSave = !disabled && changed && validation === null;

  const update = (patch: Partial<MapPropertiesForm>) =>
    setForm((current) => (current === null ? current : { ...current, ...patch }));
  const setPlayer = (index: number, patch: PlayerPatch) =>
    setForm((current) => (current === null ? current : withPlayerPatch(current, index, patch)));
  const setForce = (index: number, patch: Partial<ForceForm>) =>
    setForm((current) => (current === null ? current : withForcePatch(current, index, patch)));
  const setLayout = (layout: ForceLayout) =>
    setForm((current) => (current === null ? current : applyForceLayout(current, layout)));

  const changeOpen = (nextOpen: boolean) => {
    if (!nextOpen && saving) return;
    onOpenChange(nextOpen);
  };

  const save = async () => {
    if (form === null || !canSave) return;
    setSaving(true);
    setError(null);
    try {
      await onSave(buildMapPropertiesRequest(form));
      onOpenChange(false);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setSaving(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={changeOpen}>
      <DialogContent
        aria-busy={saving}
        closeLabel="닫기"
        className="max-h-[calc(100dvh-2rem)] flex flex-col gap-0 overflow-hidden p-0 sm:max-w-2xl [&>button]:z-20"
        onEscapeKeyDown={(event) => {
          if (saving) event.preventDefault();
        }}
        onInteractOutside={(event) => {
          if (saving) event.preventDefault();
        }}
      >
        <DialogHeader className="relative z-10 shrink-0 border-b border-border bg-background px-6 py-5 text-left">
          <DialogTitle className="flex items-center gap-2">
            <SlidersHorizontal aria-hidden="true" className="size-4 text-primary" />
            맵 속성
          </DialogTitle>
          <DialogDescription className="break-keep leading-6">
            제목·설명·플레이어 슬롯·포스를 원본 맵에 바로 저장합니다. 저장은 마지막 적용 취소로 되돌릴 수 있습니다.
          </DialogDescription>
        </DialogHeader>

        <div className="relative z-0 min-h-0 flex-1 overflow-y-auto px-4 py-4 sm:px-6">
          {blockedReason && (
            <p role="status" className="mb-4 flex items-start gap-2 break-keep rounded-lg border border-amber-500/40 bg-amber-500/5 p-3 text-sm leading-6">
              <CircleAlert aria-hidden="true" className="mt-1 size-4 shrink-0 text-amber-400" />
              {blockedReason}
            </p>
          )}
          {form !== null && (
            <Tabs defaultValue="basic">
              <TabsList aria-label="맵 속성 항목">
                <TabsTrigger value="basic">기본</TabsTrigger>
                <TabsTrigger value="players">플레이어</TabsTrigger>
                <TabsTrigger value="forces">포스</TabsTrigger>
              </TabsList>
              <TabsContent value="basic" className="grid gap-4 pt-3">
                <div className="grid gap-1.5">
                  <label htmlFor={`${idPrefix}-title`} className="text-sm font-medium">맵 제목</label>
                  <Input
                    id={`${idPrefix}-title`}
                    value={form.title}
                    disabled={disabled}
                    maxLength={256}
                    aria-invalid={form.title.trim() === ""}
                    onChange={(event) => update({ title: event.target.value })}
                  />
                </div>
                <div className="grid gap-1.5">
                  <label htmlFor={`${idPrefix}-description`} className="text-sm font-medium">맵 설명</label>
                  <Textarea
                    id={`${idPrefix}-description`}
                    value={form.description}
                    disabled={disabled}
                    rows={4}
                    maxLength={1024}
                    onChange={(event) => update({ description: event.target.value })}
                  />
                </div>
              </TabsContent>
              <TabsContent value="players" className="pt-3">
                <PlayerSlotsEditor players={form.players} forces={form.forces} disabled={disabled} onPlayerChange={setPlayer} />
              </TabsContent>
              <TabsContent value="forces" className="pt-3">
                <ForcesEditor
                  forces={form.forces}
                  players={form.players}
                  disabled={disabled}
                  onForceChange={setForce}
                  onLayout={setLayout}
                />
              </TabsContent>
            </Tabs>
          )}
          {error && (
            <div role="alert" className="mt-4 break-keep rounded-lg border border-destructive/40 bg-destructive/5 p-3 text-sm leading-6">
              <p className="flex items-start gap-2 text-destructive">
                <CircleAlert aria-hidden="true" className="mt-1 size-4 shrink-0" /> 맵 속성을 저장하지 못했습니다. 아래 원인을 확인한 뒤 다시 시도해 주세요.
              </p>
              <p className="mt-1 break-all text-xs text-muted-foreground">{error}</p>
            </div>
          )}
        </div>

        <DialogFooter className="relative z-10 shrink-0 flex-row items-center justify-between gap-2 border-t border-border bg-background px-4 py-3 sm:px-6">
          <span className="break-keep text-xs text-muted-foreground">
            {validation ?? (changed ? "변경 사항이 있습니다." : "변경 사항이 없습니다.")}
          </span>
          <div className="flex items-center gap-2">
            <Button type="button" variant="ghost" className="min-h-11" disabled={saving} onClick={() => changeOpen(false)}>
              닫기
            </Button>
            <Button type="button" className="min-h-11" disabled={!canSave} onClick={() => void save()}>
              {saving ? (
                <LoaderCircle aria-hidden="true" className="size-4 animate-spin motion-reduce:animate-none" />
              ) : (
                <Save aria-hidden="true" className="size-4" />
              )}
              {saving ? "저장 중…" : "저장"}
            </Button>
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
