import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react";
import {
  ArrowLeftIcon,
  ArrowRightIcon,
  CircleAlertIcon,
  FolderOpenIcon,
  Loader2Icon,
  MapIcon,
  SparklesIcon,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { ForcesEditor } from "@/components/map-properties/ForcesEditor";
import { PlayerSlotsEditor } from "@/components/map-properties/PlayerSlotsEditor";
import {
  applyForceLayout,
  buildMapNewSpec,
  defaultWizardForm,
  MAX_PLAYERS,
  slotNeedsStart,
  startLocationPreset,
  validateProjectName,
  validateSize,
  VERSION_OPTIONS,
  type BlankProjectRequest,
  type BlankProjectResult,
  type BrushOption,
  type ForceForm,
  type ForceLayout,
  type MapNewOptions,
  type MapVersion,
  type PlayerPatch,
  type WizardForm,
  withForcePatch,
  withPlayerPatch,
} from "@/lib/mapNew";
import type { E3sDestinationSelection } from "@/lib/projectImport";
import type { SetupMessage } from "@/lib/protocol";
import { cn, formatPathForDisplay } from "@/lib/utils";

export interface NewMapWizardProps {
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
  readonly loadOptions: () => Promise<MapNewOptions>;
  readonly loadBrushes: (tileset: number) => Promise<BrushOption[]>;
  readonly pickStarcraft: () => Promise<MapNewOptions>;
  readonly pickDestination: () => Promise<E3sDestinationSelection | null>;
  readonly create: (request: BlankProjectRequest) => Promise<BlankProjectResult>;
  readonly onCreated: (setup: SetupMessage) => void;
}

type Step = "basic" | "terrain" | "players" | "done";
const STEPS: readonly { id: Step; label: string }[] = [
  { id: "basic", label: "기본" },
  { id: "terrain", label: "지형" },
  { id: "players", label: "플레이어" },
  { id: "done", label: "완료" },
];

function createFailureText(error: string): string {
  if (error.includes("destination must be empty")) {
    return "선택한 작업 폴더가 비어 있지 않습니다. 새 빈 폴더를 만든 뒤 다시 선택해 주세요.";
  }
  if (error.includes("StarCraft")) {
    return "StarCraft 데이터를 읽지 못했습니다. StarCraft 설치 폴더를 다시 선택해 주세요.";
  }
  if (error.includes("terrain brush")) {
    return "선택한 초기 지형을 이 타일셋에서 쓸 수 없습니다. 다른 지형을 선택해 주세요.";
  }
  return "맵을 만들지 못했습니다. 오류 상세를 확인한 뒤 다시 시도해 주세요.";
}

export function NewMapWizard({
  open,
  onOpenChange,
  loadOptions,
  loadBrushes,
  pickStarcraft,
  pickDestination,
  create,
  onCreated,
}: NewMapWizardProps) {
  const idPrefix = useId();
  const [step, setStep] = useState<Step>("basic");
  const [form, setForm] = useState<WizardForm>(defaultWizardForm);
  const [widthText, setWidthText] = useState("128");
  const [heightText, setHeightText] = useState("128");
  const [options, setOptions] = useState<MapNewOptions | null>(null);
  const [optionsError, setOptionsError] = useState<string | null>(null);
  const [brushes, setBrushes] = useState<BrushOption[]>([]);
  const [brushesLoading, setBrushesLoading] = useState(false);
  const [brushesError, setBrushesError] = useState<string | null>(null);
  const [destination, setDestination] = useState<E3sDestinationSelection | null>(null);
  const [activeTask, setActiveTask] = useState<"options" | "starcraft" | "destination" | "create" | null>(null);
  const [failure, setFailure] = useState<{ message: string; detail?: string } | null>(null);
  const [created, setCreated] = useState<BlankProjectResult | null>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const openButtonRef = useRef<HTMLButtonElement>(null);
  const busy = activeTask !== null;
  const starcraftReady = options?.starcraft.available === true;

  const reset = useCallback(() => {
    setStep("basic");
    setForm(defaultWizardForm());
    setWidthText("128");
    setHeightText("128");
    setBrushes([]);
    setBrushesLoading(false);
    setBrushesError(null);
    setDestination(null);
    setActiveTask(null);
    setFailure(null);
    setCreated(null);
  }, []);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setActiveTask("options");
    setOptionsError(null);
    loadOptions()
      .then((loaded) => {
        if (!cancelled) setOptions(loaded);
      })
      .catch((error: unknown) => {
        if (!cancelled) {
          setOptionsError(error instanceof Error ? error.message : String(error));
        }
      })
      .finally(() => {
        if (!cancelled) setActiveTask(null);
      });
    return () => {
      cancelled = true;
    };
  }, [open, loadOptions]);

  useEffect(() => {
    if (!open || !starcraftReady) return;
    let cancelled = false;
    setBrushesLoading(true);
    setBrushesError(null);
    loadBrushes(form.tileset)
      .then((loaded) => {
        if (cancelled) return;
        setBrushes(loaded);
        // Brush ids are per-tileset, so a tileset change always re-picks the
        // first graphics-valid brush instead of silently keeping a same-numbered
        // brush of another tileset.
        const first = loaded.find((brush) => brush.graphicsValid) ?? loaded[0];
        setForm((current) => ({ ...current, terrainType: first?.id ?? 0 }));
      })
      .catch((error: unknown) => {
        if (cancelled) return;
        setBrushes([]);
        setForm((current) => ({ ...current, terrainType: 0 }));
        setBrushesError(error instanceof Error ? error.message : String(error));
      })
      .finally(() => {
        if (!cancelled) setBrushesLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [open, starcraftReady, form.tileset, loadBrushes]);

  useEffect(() => {
    if (step === "done") openButtonRef.current?.focus();
  }, [step]);

  useEffect(() => {
    if (failure === null || contentRef.current === null) return;
    contentRef.current.scrollTop = contentRef.current.scrollHeight;
  }, [failure]);

  const changeOpen = (nextOpen: boolean) => {
    if (!nextOpen && busy) return;
    if (!nextOpen && created !== null) onCreated(created.setup);
    if (!nextOpen) reset();
    onOpenChange(nextOpen);
  };

  const nameError = validateProjectName(form.name);
  const widthError = validateSize(form.width, options?.sizeMin, options?.sizeMax);
  const heightError = validateSize(form.height, options?.sizeMin, options?.sizeMax);
  const basicReady = nameError === null && destination?.empty === true && starcraftReady;
  const terrainReady =
    widthError === null &&
    heightError === null &&
    !brushesLoading &&
    brushes.some((brush) => brush.id === form.terrainType);
  const playersReady = form.players.length >= 1;
  const stepIndex = STEPS.findIndex((entry) => entry.id === step);

  const updateForm = (patch: Partial<WizardForm>) => setForm((current) => ({ ...current, ...patch }));

  const setSize = (axis: "width" | "height", text: string) => {
    if (axis === "width") setWidthText(text);
    else setHeightText(text);
    const trimmed = text.trim();
    const value = /^\d+$/u.test(trimmed) ? Number(trimmed) : Number.NaN;
    updateForm({ [axis]: value });
  };

  const setPlayer = (index: number, patch: PlayerPatch) => setForm((current) => withPlayerPatch(current, index, patch));
  const setForce = (index: number, patch: Partial<ForceForm>) => setForm((current) => withForcePatch(current, index, patch));
  const setLayout = (layout: ForceLayout) => setForm((current) => applyForceLayout(current, layout));

  const selectStarcraft = async () => {
    setActiveTask("starcraft");
    setFailure(null);
    try {
      setOptions(await pickStarcraft());
    } catch (error) {
      setFailure({
        message: "StarCraft 폴더를 설정하지 못했습니다. StarCraft: Remastered 설치 폴더를 선택해 주세요.",
        detail: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setActiveTask(null);
    }
  };

  const selectDestination = async () => {
    setActiveTask("destination");
    setFailure(null);
    try {
      const selected = await pickDestination();
      if (selected !== null) {
        setDestination(selected);
        if (!selected.empty) {
          setFailure({ message: "선택한 작업 폴더가 비어 있지 않습니다. 새 빈 폴더를 만든 뒤 다시 선택해 주세요." });
        }
      }
    } catch {
      setFailure({ message: "작업 폴더 선택 창을 열지 못했습니다. 잠시 후 다시 시도해 주세요." });
    } finally {
      setActiveTask(null);
    }
  };

  const runCreate = async () => {
    if (!basicReady || !terrainReady || !playersReady || destination === null) return;
    setActiveTask("create");
    setFailure(null);
    try {
      const request: BlankProjectRequest = {
        destination: destination.path,
        name: form.name.trim(),
        spec: buildMapNewSpec(form),
      };
      const result = await create(request);
      if (result.setup.error) {
        setFailure({ message: createFailureText(result.setup.error), detail: result.setup.error });
        return;
      }
      if (!result.setup.projectOpened || result.preview === null) {
        setFailure({
          message: "맵 생성이 완료되지 않았습니다. 프로젝트를 활성화하지 않았습니다.",
          detail: "setup_create_blank_project returned projectOpened=false",
        });
        return;
      }
      setCreated(result);
      setStep("done");
    } catch (error) {
      setFailure({
        message: "맵 생성 명령을 실행하지 못했습니다. 오류 상세를 확인하고 다시 시도해 주세요.",
        detail: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setActiveTask(null);
    }
  };

  const finish = () => {
    if (created === null) return;
    const setup = created.setup;
    reset();
    onOpenChange(false);
    onCreated(setup);
  };

  const previewStarts = useMemo(() => {
    if (!form.autoStart) return [];
    const playable = form.players
      .map((player, index) => ({ player, index }))
      .filter(({ player, index }) => index < MAX_PLAYERS && slotNeedsStart(player.type));
    const preset = startLocationPreset(form.width, form.height, playable.length);
    return playable.map(({ index }, position) => ({ index, start: preset[position]! }));
  }, [form.autoStart, form.width, form.height, form.players]);
  const tilesetLabel = options?.tilesets.find((entry) => entry.id === form.tileset)?.label ?? "";
  const brushLabel = brushes.find((brush) => brush.id === form.terrainType)?.name ?? "";

  return (
    <Dialog open={open} onOpenChange={changeOpen}>
      <DialogContent
        aria-busy={busy}
        closeLabel="닫기"
        className="max-h-[calc(100dvh-2rem)] flex flex-col gap-0 overflow-hidden p-0 sm:max-w-2xl [&>button]:z-20"
        onEscapeKeyDown={(event) => {
          if (busy) event.preventDefault();
        }}
        onInteractOutside={(event) => {
          if (busy) event.preventDefault();
        }}
      >
        <DialogHeader className="relative z-10 shrink-0 border-b border-border bg-background px-6 py-5 text-left">
          <DialogTitle className="flex items-center gap-2">
            <MapIcon aria-hidden className="size-4 text-primary" />
            빈 맵으로 새 프로젝트
          </DialogTitle>
          <DialogDescription className="break-keep leading-6">
            SCMDraft 없이 타일셋·크기·지형·플레이어·포스·시작 위치까지 한 번에 정해 새 맵과 프로젝트를 만듭니다.
          </DialogDescription>
          <ol className="mt-3 flex flex-wrap gap-2" aria-label="진행 단계">
            {STEPS.map((entry, index) => (
              <li
                key={entry.id}
                aria-current={entry.id === step ? "step" : undefined}
                className={cn(
                  "flex items-center gap-2 rounded-full border px-3 py-1 text-xs",
                  entry.id === step && "border-primary bg-primary/10 text-foreground",
                  index < stepIndex && "border-emerald-500/30 text-emerald-300",
                  index > stepIndex && "border-border text-muted-foreground",
                )}
              >
                <span className="font-semibold tabular-nums">{index + 1}</span>
                {entry.label}
              </li>
            ))}
          </ol>
        </DialogHeader>

        <div ref={contentRef} className="relative z-0 min-h-0 flex-1 overflow-y-auto px-4 py-4 sm:px-6">
          {activeTask === "options" && options === null && (
            <p role="status" className="flex items-center gap-2 text-sm text-muted-foreground">
              <Loader2Icon aria-hidden className="size-4 animate-spin motion-reduce:animate-none" /> 맵 생성 옵션을 불러오는 중…
            </p>
          )}
          {optionsError && (
            <p role="alert" className="flex items-start gap-2 break-keep text-sm leading-6 text-destructive">
              <CircleAlertIcon aria-hidden className="mt-1 size-4 shrink-0" /> 맵 생성 옵션을 불러오지 못했습니다. 앱을 다시 연 뒤 시도해 주세요. ({optionsError})
            </p>
          )}

          {options !== null && !starcraftReady && step !== "done" && (
            <section
              aria-label="StarCraft 데이터 필요"
              className="mb-4 break-keep rounded-lg border border-amber-500/40 bg-amber-500/5 p-4 text-sm leading-6"
            >
              <p className="font-medium">StarCraft 설치 폴더가 필요합니다.</p>
              <p className="mt-1 text-muted-foreground">
                빈 맵의 지형은 StarCraft: Remastered의 타일셋 데이터로 만듭니다. 설치 폴더를 선택하면 바로 이어서 진행할 수 있습니다.
              </p>
              {options.starcraft.reason && (
                <p className="mt-1 font-mono text-xs text-muted-foreground">{options.starcraft.reason}</p>
              )}
              <Button type="button" className="mt-3 min-h-11" disabled={busy} onClick={() => void selectStarcraft()}>
                {activeTask === "starcraft" ? (
                  <Loader2Icon aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
                ) : (
                  <FolderOpenIcon aria-hidden className="size-4" />
                )}
                StarCraft 폴더 선택
              </Button>
            </section>
          )}

          {options !== null && step === "basic" && (
            <div className="grid gap-4">
              <div className="grid gap-1.5">
                <label htmlFor={`${idPrefix}-name`} className="text-sm font-medium">프로젝트 이름</label>
                <Input
                  id={`${idPrefix}-name`}
                  value={form.name}
                  disabled={busy}
                  aria-invalid={form.name !== "" && nameError !== null}
                  aria-describedby={`${idPrefix}-name-hint`}
                  placeholder="예: 협동 방어전"
                  onChange={(event) => updateForm({ name: event.target.value })}
                />
                <p id={`${idPrefix}-name-hint`} className={cn("break-keep text-xs leading-5", form.name !== "" && nameError ? "text-destructive" : "text-muted-foreground")}>
                  {form.name !== "" && nameError
                    ? nameError
                    : `맵 파일은 maps/${form.name.trim() || "이름"}.scx, 빌드 결과는 build/[EUD]${form.name.trim() || "이름"}.scx 로 저장됩니다.`}
                </p>
              </div>
              <div className="grid gap-1.5">
                <label htmlFor={`${idPrefix}-title`} className="text-sm font-medium">맵 제목 <span className="font-normal text-muted-foreground">(비우면 프로젝트 이름)</span></label>
                <Input
                  id={`${idPrefix}-title`}
                  value={form.title}
                  disabled={busy}
                  maxLength={256}
                  onChange={(event) => updateForm({ title: event.target.value })}
                />
              </div>
              <div className="grid gap-1.5">
                <label htmlFor={`${idPrefix}-description`} className="text-sm font-medium">맵 설명 <span className="font-normal text-muted-foreground">(선택)</span></label>
                <Textarea
                  id={`${idPrefix}-description`}
                  value={form.description}
                  disabled={busy}
                  rows={3}
                  maxLength={1024}
                  onChange={(event) => updateForm({ description: event.target.value })}
                />
              </div>
              <div className="grid gap-2">
                <span id={`${idPrefix}-version-label`} className="text-sm font-medium">맵 형식</span>
                <RadioGroup
                  aria-labelledby={`${idPrefix}-version-label`}
                  value={form.version}
                  disabled={busy}
                  className="grid gap-2 sm:grid-cols-2"
                  onValueChange={(value) => updateForm({ version: value as MapVersion })}
                >
                  {VERSION_OPTIONS.map((entry) => (
                    <label
                      key={entry.value}
                      htmlFor={`${idPrefix}-version-${entry.value}`}
                      className={cn(
                        "flex cursor-pointer items-start gap-3 rounded-lg border p-3 text-sm",
                        form.version === entry.value ? "border-primary bg-primary/5" : "border-border",
                      )}
                    >
                      <RadioGroupItem id={`${idPrefix}-version-${entry.value}`} value={entry.value} className="mt-0.5" />
                      <span>
                        <span className="block font-medium">{entry.label}</span>
                        <span className="block text-xs text-muted-foreground">{entry.hint}</span>
                      </span>
                    </label>
                  ))}
                </RadioGroup>
              </div>
              <div className="grid gap-1.5">
                <span className="text-sm font-medium">작업 폴더</span>
                <div className="flex flex-wrap items-center gap-3">
                  <Button
                    type="button"
                    variant={destination === null ? "default" : "outline"}
                    className="min-h-11"
                    disabled={busy}
                    onClick={() => void selectDestination()}
                  >
                    {activeTask === "destination" ? (
                      <Loader2Icon aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
                    ) : (
                      <FolderOpenIcon aria-hidden className="size-4" />
                    )}
                    {destination === null ? "새 작업 폴더 선택" : "다른 작업 폴더 선택"}
                  </Button>
                  {destination !== null && (
                    <span className={cn("break-all font-mono text-xs", destination.empty ? "text-muted-foreground" : "text-destructive")}>
                      {formatPathForDisplay(destination.path)}
                    </span>
                  )}
                </div>
                <p className="break-keep text-xs leading-5 text-muted-foreground">선택 창에서 새 폴더를 만든 뒤 그 빈 폴더를 선택합니다. 맵과 프로젝트 파일이 이곳에 만들어집니다.</p>
              </div>
            </div>
          )}

          {options !== null && step === "terrain" && (
            <div className="grid gap-4">
              <div className="grid gap-1.5">
                <label htmlFor={`${idPrefix}-tileset`} className="text-sm font-medium">타일셋</label>
                <Select
                  value={String(form.tileset)}
                  disabled={busy}
                  onValueChange={(value) => updateForm({ tileset: Number(value) })}
                >
                  <SelectTrigger id={`${idPrefix}-tileset`} className="w-full">
                    <SelectValue placeholder="타일셋 선택" />
                  </SelectTrigger>
                  <SelectContent>
                    {options.tilesets.map((entry) => (
                      <SelectItem key={entry.id} value={String(entry.id)}>{entry.label}</SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              <fieldset className="grid gap-2">
                <legend className="text-sm font-medium">맵 크기 (타일)</legend>
                <div className="flex flex-wrap gap-2" role="group" aria-label="크기 프리셋">
                  {options.sizePresets.map((size) => (
                    <Button
                      key={size}
                      type="button"
                      size="sm"
                      variant={form.width === size && form.height === size ? "default" : "outline"}
                      disabled={busy}
                      aria-pressed={form.width === size && form.height === size}
                      onClick={() => {
                        setWidthText(String(size));
                        setHeightText(String(size));
                        updateForm({ width: size, height: size });
                      }}
                    >
                      {size}×{size}
                    </Button>
                  ))}
                </div>
                <div className="grid grid-cols-2 gap-3">
                  <div className="grid gap-1.5">
                    <label htmlFor={`${idPrefix}-width`} className="text-xs font-medium text-muted-foreground">가로</label>
                    <Input
                      id={`${idPrefix}-width`}
                      inputMode="numeric"
                      value={widthText}
                      disabled={busy}
                      aria-invalid={widthError !== null}
                      aria-describedby={`${idPrefix}-size-hint`}
                      onChange={(event) => setSize("width", event.target.value)}
                    />
                  </div>
                  <div className="grid gap-1.5">
                    <label htmlFor={`${idPrefix}-height`} className="text-xs font-medium text-muted-foreground">세로</label>
                    <Input
                      id={`${idPrefix}-height`}
                      inputMode="numeric"
                      value={heightText}
                      disabled={busy}
                      aria-invalid={heightError !== null}
                      aria-describedby={`${idPrefix}-size-hint`}
                      onChange={(event) => setSize("height", event.target.value)}
                    />
                  </div>
                </div>
                <p
                  id={`${idPrefix}-size-hint`}
                  role={widthError ?? heightError ? "alert" : undefined}
                  className={cn("break-keep text-xs leading-5", widthError ?? heightError ? "text-destructive" : "text-muted-foreground")}
                >
                  {widthError ?? heightError ?? `${options.sizeMin}~${options.sizeMax} 사이의 값을 자유롭게 넣을 수 있습니다.`}
                </p>
              </fieldset>
              <div className="grid gap-1.5">
                <label htmlFor={`${idPrefix}-brush`} className="text-sm font-medium">초기 지형</label>
                <Select
                  value={brushes.some((brush) => brush.id === form.terrainType) ? String(form.terrainType) : ""}
                  disabled={busy || brushesLoading || brushes.length === 0}
                  onValueChange={(value) => updateForm({ terrainType: Number(value) })}
                >
                  <SelectTrigger id={`${idPrefix}-brush`} className="w-full">
                    <SelectValue placeholder={brushesLoading ? "불러오는 중…" : "지형 없음"} />
                  </SelectTrigger>
                  <SelectContent>
                    {brushes.map((brush) => (
                      <SelectItem key={brush.id} value={String(brush.id)}>
                        {brush.name}
                        {brush.graphicsValid ? "" : " (그래픽 없음)"}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                {brushesError && (
                  <div role="alert" className="break-keep rounded-lg border border-destructive/40 bg-destructive/5 p-3 text-xs leading-5">
                    <p className="text-destructive">지형 브러시를 불러오지 못했습니다. 선택된 StarCraft 폴더에 타일셋 데이터가 없을 수 있습니다.</p>
                    <p className="mt-1 break-all font-mono text-muted-foreground">{brushesError}</p>
                    <Button type="button" variant="outline" size="sm" className="mt-2" disabled={busy} onClick={() => void selectStarcraft()}>
                      <FolderOpenIcon aria-hidden className="size-4" /> StarCraft 폴더 다시 선택
                    </Button>
                  </div>
                )}
                <p className="break-keep text-xs leading-5 text-muted-foreground">맵 전체를 이 지형으로 채웁니다. 이후 에이전트나 지형 편집으로 바꿀 수 있습니다.</p>
              </div>
            </div>
          )}

          {options !== null && step === "players" && (
            <div className="grid gap-4">
              <p className="break-keep text-xs leading-5 text-muted-foreground">
                CHK 슬롯 12개를 모두 설정합니다. 포스와 시작 위치는 P1~P8에만 적용됩니다.
              </p>
              <PlayerSlotsEditor players={form.players} forces={form.forces} disabled={busy} onPlayerChange={setPlayer} />
              <ForcesEditor
                forces={form.forces}
                players={form.players}
                disabled={busy}
                onForceChange={setForce}
                onLayout={setLayout}
              />
              <label className="flex items-start gap-3 rounded-lg border border-border p-3 text-sm">
                <Checkbox
                  checked={form.autoStart}
                  disabled={busy}
                  aria-label="시작 위치 자동 배치"
                  onCheckedChange={(checked) => updateForm({ autoStart: checked === true })}
                />
                <span>
                  <span className="block font-medium">시작 위치 자동 배치</span>
                  <span className="block break-keep text-xs leading-5 text-muted-foreground">
                    모든 시작 위치를 맵 왼쪽 위(가장자리 4타일 안쪽)에 겹치지 않게 모아 둡니다. 나중에 원하는 자리로 옮기세요. 끄면 시작 위치 없이 만듭니다.
                  </span>
                  {form.autoStart && previewStarts.length > 0 && (
                    <span className="mt-1 block font-mono text-xs text-muted-foreground">
                      {previewStarts.map(({ index, start }) => `P${index + 1} (${Math.floor(start.x / 32)}, ${Math.floor(start.y / 32)})`).join(" · ")}
                    </span>
                  )}
                </span>
              </label>
            </div>
          )}

          {step === "done" && created?.preview && (
            <div className="grid gap-4">
              <p role="status" className="flex items-center gap-2 text-sm font-medium">
                <SparklesIcon aria-hidden className="size-4 text-primary" /> 맵과 프로젝트를 만들었습니다.
              </p>
              <img
                src={`data:image/png;base64,${created.preview.previewPng}`}
                alt={`${created.preview.width}×${created.preview.height} ${created.preview.tileset} 맵 미리보기`}
                className="max-h-96 w-full rounded-lg border border-border object-contain"
              />
              <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-1 text-sm">
                <dt className="text-muted-foreground">크기</dt>
                <dd>{created.preview.width}×{created.preview.height} 타일</dd>
                <dt className="text-muted-foreground">타일셋</dt>
                <dd>{tilesetLabel || created.preview.tileset}</dd>
                <dt className="text-muted-foreground">지형</dt>
                <dd>{brushLabel || String(form.terrainType)}</dd>
                <dt className="text-muted-foreground">플레이어</dt>
                <dd>{created.preview.players}명 · 시작 위치 {created.preview.startLocations}개</dd>
                <dt className="text-muted-foreground">맵 파일</dt>
                <dd className="break-all font-mono text-xs">{formatPathForDisplay(created.preview.mapPath)}</dd>
                <dt className="text-muted-foreground">빌드 결과</dt>
                <dd className="break-all font-mono text-xs">{created.preview.outputMap}</dd>
              </dl>
            </div>
          )}

          {failure && (
            <div role="alert" className="mt-4 break-keep rounded-lg border border-destructive/40 bg-destructive/5 p-3 text-sm leading-6">
              <p className="flex items-start gap-2 text-destructive">
                <CircleAlertIcon aria-hidden className="mt-1 size-4 shrink-0" /> {failure.message}
              </p>
              {failure.detail && (
                <p className="mt-1 break-all font-mono text-xs text-muted-foreground">{failure.detail}</p>
              )}
            </div>
          )}
        </div>

        <DialogFooter className="relative z-10 shrink-0 flex-row items-center justify-between gap-2 border-t border-border bg-background px-4 py-3 sm:px-6">
          {step === "done" ? (
            <>
              <span />
              <Button ref={openButtonRef} type="button" className="min-h-11" onClick={finish}>
                프로젝트 열기 <ArrowRightIcon aria-hidden className="size-4" />
              </Button>
            </>
          ) : (
            <>
              <Button
                type="button"
                variant="ghost"
                className="min-h-11"
                disabled={busy || step === "basic"}
                onClick={() => setStep(step === "players" ? "terrain" : "basic")}
              >
                <ArrowLeftIcon aria-hidden className="size-4" /> 이전
              </Button>
              {step === "basic" && (
                <Button type="button" className="min-h-11" disabled={busy || !basicReady} onClick={() => setStep("terrain")}>
                  다음 <ArrowRightIcon aria-hidden className="size-4" />
                </Button>
              )}
              {step === "terrain" && (
                <Button type="button" className="min-h-11" disabled={busy || !terrainReady} onClick={() => setStep("players")}>
                  다음 <ArrowRightIcon aria-hidden className="size-4" />
                </Button>
              )}
              {step === "players" && (
                <Button
                  type="button"
                  className="min-h-11"
                  disabled={busy || !basicReady || !terrainReady || !playersReady}
                  onClick={() => void runCreate()}
                >
                  {activeTask === "create" ? (
                    <Loader2Icon aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
                  ) : (
                    <MapIcon aria-hidden className="size-4" />
                  )}
                  맵 만들기
                </Button>
              )}
            </>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
