/**
 * Main chat/plan-feedback input:
 *   - generic ordered mentions plus text and session-owned attachments;
 *   - file picker, HTML5 drag-and-drop, pasted clipboard images, and project
 *     files picked from `@` search;
 *   - removable attachment and resource-mention chips;
 *   - Send is gated by the store's `canSend`; attachment-only and mention-only messages are valid.
 *   - While a turn runs, Send steers it instead (`onSteer`): the message
 *     (text, attachments, mentions) reaches the running turn at its next tool
 *     boundary. The per-turn toggles (인터뷰, skills, model) are off meanwhile,
 *     and a pending question is answered on its card, not here.
 *
 * The textarea is an accessible combobox named "지시 입력"; visible controls are
 * labelled "첨부" and "전송". Enter (without Shift / IME composition)
 * submits through PromptInput when mention search is closed.
 */
import {
  useEffect,
  useRef,
  useState,
  type ClipboardEvent,
  type DragEvent,
} from "react";
import {
  AudioLinesIcon,
  FileTextIcon,
  ImageIcon,
  CornerDownRightIcon,
  LoaderCircleIcon,
  MessageCircleQuestionMarkIcon,
  PaperclipIcon,
  PlusIcon,
  SendIcon,
  SparklesIcon,
  XIcon,
} from "lucide-react";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  PromptInput,
  PromptInputFooter,
  PromptInputSubmit,
  PromptInputTools,
} from "@/components/ai-elements/prompt-input";
import { PromptInputButton } from "@/components/ai-elements/prompt-input";
import { ProviderPromptControls } from "@/components/ProviderPromptControls";
import { AgentTurnStatus } from "@/components/AgentTurnStatus";
import { MentionComposer } from "@/components/MentionComposer";
import { isBusyPhase } from "@/state/store";
import type { PanelState } from "@/state/store";
import type {
  AutonomousRunState,
  ChatAttachment,
  MentionInstance,
  ExecutionMode,
  MentionSearchRequest,
  MentionSearchResponse,
  ReasoningSelection,
  SessionModelSettings,
  SkillSummary,
} from "@/lib/ipc";
import {
  attachmentErrorMessage,
  formatAttachmentSize,
  MAX_ATTACHMENTS_PER_TURN,
  MAX_AUDIO_BYTES,
  MAX_AUDIO_BYTES_PER_TURN,
  MAX_TEXT_BYTES,
} from "@/lib/attachments";
import type { ProjectFileSuggestion } from "@/lib/projectFiles";



const LUCK_DEFENSE = "luck-defense";
/**
 * Used until the backend registry loads, so a persisted ON is never dropped.
 * Empty while luck-defense is hidden (`HIDDEN_SKILLS` in `skills.rs`).
 */
const FALLBACK_SKILLS: readonly SkillSummary[] = [];

function skillsKey(project: string): string {
  return `eud-agent:skills:${project}`;
}

/** Selected skill ids for one project; migrates the single luck-defense flag. */
function loadActiveSkills(project: string): string[] {
  try {
    const stored = localStorage.getItem(skillsKey(project));
    if (stored !== null) {
      const parsed: unknown = JSON.parse(stored);
      return Array.isArray(parsed)
        ? parsed.filter((id): id is string => typeof id === "string")
        : [];
    }
    const legacyKey = `eud-agent:skill:luck-defense:${project}`;
    const legacy = localStorage.getItem(legacyKey);
    if (legacy === null) return [];
    const migrated = legacy === "true" ? [LUCK_DEFENSE] : [];
    localStorage.setItem(skillsKey(project), JSON.stringify(migrated));
    localStorage.removeItem(legacyKey);
    return migrated;
  } catch {
    return [];
  }
}

export interface ChatPayload {
  text: string;
  attachments: ChatAttachment[];
  mentions: MentionInstance[];
  executionMode: ExecutionMode;
  /** Interview before working on this one message (the toggle resets on send). */
  clarify?: boolean;
  /** Preserved only when retrying a transport-rejected submission. */
  clientTurnId?: string;
  /** Explicit per-skill ON/OFF directives; an absent id is unchanged. */
  skillSelections?: Record<string, boolean>;
}

/** A message sent to the running turn: no per-turn toggles ride along. */
export interface SteerPayload {
  text: string;
  attachments: ChatAttachment[];
  mentions: MentionInstance[];
}

export interface InstructionBoxProps {
  state: PanelState;
  onSend(msg: ChatPayload): void;
  /** Send a message composed while a turn runs to that turn; absent means Send waits. */
  onSteer?(msg: SteerPayload): void;
  /** Copy one browser File into app-owned attachment storage. */
  onStageAttachment?(file: File): Promise<ChatAttachment>;
  /** Delete an attachment draft removed before send. */
  onDiscardAttachment?(id: string): Promise<void>;
  /** Interrupt the active turn without discarding its prior conversation. */
  onCancel?(): void;
  /** Durable autonomous lifecycle controls, separate from turn cancellation. */
  autonomousRun?: AutonomousRunState | null;
  onAutonomousPause?(): void;
  onAutonomousResume?(): void;
  onAutonomousStop?(): void;
  /** A past user message restored for editing after a successful rewind. */
  draft?: ChatPayload | null;
  /** Backend-owned bounded resource search used by the generic composer. */
  onMentionSearch?(request: MentionSearchRequest): Promise<MentionSearchResponse>;
  /** Project-root files offered by `@` search next to the map resources. */
  onProjectFileSearch?(query: string): Promise<ProjectFileSuggestion[]>;
  /** Read one offered project file so it can be staged like a picked file. */
  onReadProjectFile?(file: ProjectFileSuggestion): Promise<File>;
  /** Current native project identity; a change invalidates unsent mention snapshots. */
  projectIdentity?: string;
  /** Selected session/draft identity; mention drafts never cross this boundary. */
  scopeIdentity?: string;
  /** A cancel/rewind command is waiting for the core. */
  actionBusy?: boolean;
  /** Current session's immutable provider and provider-scoped model catalog. */
  modelSettings?: SessionModelSettings | null;
  /** Catalog load or session settings save in flight. */
  modelSettingsBusy?: boolean;
  /** Persist a model selection within the already-bound provider. */
  onModelSettingsChange?(
    model: string,
    reasoning: ReasoningSelection | undefined,
  ): void;
  /** Retry loading the bound provider catalog. */
  onModelSettingsReload?(): void;
  /** Compiled skill registry offered by the composer's skill picker. */
  skills?: readonly SkillSummary[];
  /** False for sessions whose turns ignore skill selections (Map, workers). */
  skillsAvailable?: boolean;
}

export function InstructionBox({
  state,
  onSend,
  onSteer,
  onStageAttachment,
  onDiscardAttachment,
  onCancel,
  autonomousRun = null,
  onAutonomousPause,
  onAutonomousResume,
  onAutonomousStop,
  draft,
  onMentionSearch,
  onProjectFileSearch,
  onReadProjectFile,
  projectIdentity = state.project,
  scopeIdentity = "default",
  actionBusy = false,
  modelSettings,
  modelSettingsBusy = false,
  onModelSettingsChange,
  onModelSettingsReload,
  skills: loadedSkills,
  skillsAvailable = true,
}: InstructionBoxProps) {
  const [instruction, setInstruction] = useState("");
  const [activeSkillIds, setActiveSkillIds] = useState(() =>
    loadActiveSkills(projectIdentity),
  );
  const [skillSelections, setSkillSelections] = useState<
    Record<string, boolean>
  >({});
  const [attachments, setAttachments] = useState<ChatAttachment[]>([]);
  const [mentions, setMentions] = useState<MentionInstance[]>([]);
  const [attachmentError, setAttachmentError] = useState<string | null>(null);
  const [staging, setStaging] = useState(false);
  const [dragging, setDragging] = useState(false);
  const [clarify, setClarify] = useState(false);
  const stagingRef = useRef(false);
  const retryClientTurnId = useRef<string | undefined>(undefined);
  const dragDepth = useRef(0);
  const fileInput = useRef<HTMLInputElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    setActiveSkillIds(loadActiveSkills(projectIdentity));
    // Pending directives belong to the project they were made in.
    setSkillSelections({});
  }, [projectIdentity]);

  useEffect(() => {
    if (draft === undefined || draft === null) return;
    setInstruction(draft.text);
    setAttachments([...draft.attachments]);
    setMentions(draft.mentions.map((mention) => ({ ...mention })));
    setClarify(draft.clarify === true);
    retryClientTurnId.current = draft.clientTurnId;
    setSkillSelections({ ...draft.skillSelections });
    setAttachmentError(null);
    textareaRef.current?.focus();
  }, [draft]);

  // The store owns connection/project/busy gating. Empty text remains valid when
  // at least one staged attachment or validated mention is present.
  const autonomousBlocksNewRequest =
    autonomousRun !== null &&
    !["completed", "cancelled", "failed", "safety_stopped"].includes(
      autonomousRun.status,
    );
  const hasStaleMention = mentions.some((mention) => mention.stale === true);
  const turnInFlight = isBusyPhase(state.phase);
  const ragLoading = state.rag === "loading";
  const projectUnavailable = !state.projectAvailable;
  // A running turn takes the message at its next tool boundary. A pending
  // question is answered on its card instead.
  const steering =
    onSteer !== undefined &&
    turnInFlight &&
    state.ask === null &&
    state.connected &&
    state.hasProject &&
    state.projectAvailable &&
    !ragLoading;
  const canSend =
    (state.canSend || steering) &&
    !actionBusy &&
    (steering || !autonomousBlocksNewRequest);
  // Plan feedback revises the proposed plan; it never starts an interview, and
  // a message to a running turn carries no per-turn toggle.
  const clarifyAvailable = state.phase !== "plan_review" && !turnInFlight;
  const clarifyOn = clarify && clarifyAvailable;
  const attachmentInputDisabled =
    !canSend || staging || onStageAttachment === undefined;
  // Only registry skills are shown, and only shown skills are sent as ON.
  const skills =
    loadedSkills !== undefined && loadedSkills.length > 0
      ? loadedSkills
      : FALLBACK_SKILLS;
  const activeSkills = skillsAvailable
    ? skills.filter((skill) => activeSkillIds.includes(skill.id))
    : [];
  const skillPickerDisabled =
    !skillsAvailable ||
    !state.hasProject ||
    state.phase === "plan_review" ||
    turnInFlight;
  const luckDefenseActive = activeSkills.some((skill) => skill.id === LUCK_DEFENSE);
  const placeholder = projectUnavailable
    ? "Native 프로젝트를 열 수 없습니다. .eap 파일 경로를 확인하세요"
    : ragLoading
      ? "RAG 모델 준비 중… 준비가 끝나면 입력할 수 있습니다"
      : state.phase === "plan_review"
        ? "계획 수정 피드백을 입력하세요 (승인은 계획 탭에서)"
        : steering
          ? "작업 중인 AI에게 전달할 메시지 (다음 도구 호출 사이에 반영됩니다)"
          : luckDefenseActive
          ? "운빨 디펜스를 어떻게 만들까요?"
          : "무엇을 만들까요? (예: 게임 시작 시 미네랄 +1000 트리거 추가)";

  async function stageFiles(source: FileList | readonly File[]) {
    if (
      onStageAttachment === undefined ||
      stagingRef.current ||
      !canSend
    ) {
      return;
    }
    const available = MAX_ATTACHMENTS_PER_TURN - attachments.length;
    const files = Array.from(source).slice(0, Math.max(available, 0));
    if (files.length === 0) {
      setAttachmentError(
        `한 번에 첨부할 수 있는 파일은 최대 ${MAX_ATTACHMENTS_PER_TURN}개입니다.`,
      );
      return;
    }
    const omitted = Array.from(source).length - files.length;
    setAttachmentError(
      omitted > 0
        ? `최대 ${MAX_ATTACHMENTS_PER_TURN}개까지만 첨부했습니다.`
        : null,
    );
    stagingRef.current = true;
    setStaging(true);
    let next = attachments;
    try {
      for (const file of files) {
        const staged = await onStageAttachment(file);
        const textBytes =
          next
            .filter((attachment) => attachment.kind === "text")
            .reduce((total, attachment) => total + attachment.size, 0) +
          (staged.kind === "text" ? staged.size : 0);
        if (textBytes > MAX_TEXT_BYTES) {
          await onDiscardAttachment?.(staged.id);
          throw new Error(
            "한 번에 첨부하는 텍스트/코드는 합계 512KB 이하여야 합니다.",
          );
        }
        const audioBytes =
          next
            .filter((attachment) => attachment.kind === "audio")
            .reduce((total, attachment) => total + attachment.size, 0) +
          (staged.kind === "audio" ? staged.size : 0);
        if (audioBytes > MAX_AUDIO_BYTES_PER_TURN) {
          await onDiscardAttachment?.(staged.id);
          throw new Error(
            "한 번에 첨부하는 오디오는 합계 128MB 이하여야 합니다.",
          );
        }
        next = [...next, staged];
        setAttachments(next);
      }
    } catch (error) {
      setAttachmentError(attachmentErrorMessage(error));
    } finally {
      stagingRef.current = false;
      setStaging(false);
      if (fileInput.current !== null) fileInput.current.value = "";
    }
  }

  async function attachProjectFile(file: ProjectFileSuggestion) {
    if (onReadProjectFile === undefined || attachmentInputDisabled) return;
    if (file.size > MAX_AUDIO_BYTES) {
      setAttachmentError(`첨부할 수 없는 큰 파일입니다: ${file.path}`);
      return;
    }
    try {
      await stageFiles([await onReadProjectFile(file)]);
    } catch (error) {
      setAttachmentError(
        `${file.path} 파일을 첨부하지 못했습니다: ${attachmentErrorMessage(error)}`,
      );
    }
  }

  async function removeAttachment(attachment: ChatAttachment) {
    setAttachments((current) =>
      current.filter((candidate) => candidate.id !== attachment.id),
    );
    try {
      await onDiscardAttachment?.(attachment.id);
    } catch (error) {
      setAttachmentError(attachmentErrorMessage(error));
    }
  }

  function setSkillActive(id: string, active: boolean) {
    const next = active
      ? [...activeSkillIds.filter((candidate) => candidate !== id), id]
      : activeSkillIds.filter((candidate) => candidate !== id);
    try {
      localStorage.setItem(skillsKey(projectIdentity), JSON.stringify(next));
    } catch {
      // The selection still applies to this window when storage is unavailable.
    }
    setActiveSkillIds(next);
    setSkillSelections((current) => ({ ...current, [id]: active }));
  }

  function submit() {
    const text = instruction.trim();
    if (
      !canSend ||
      stagingRef.current ||
      hasStaleMention ||
      (text.length === 0 && attachments.length === 0 && mentions.length === 0)
    ) {
      return;
    }
    if (steering) {
      onSteer?.({ text, attachments, mentions });
      setInstruction("");
      setAttachments([]);
      setMentions([]);
      setAttachmentError(null);
      return;
    }
    // Map and worker turns ignore selections, so none are sent there.
    const selectedSkills: Record<string, boolean> = skillsAvailable
      ? {
          ...Object.fromEntries(activeSkills.map((skill) => [skill.id, true])),
          ...skillSelections,
        }
      : {};
    const payload: ChatPayload = {
      text,
      attachments,
      mentions,
      executionMode: "interactive",
      ...(clarifyOn ? { clarify: true } : {}),
      ...(Object.keys(selectedSkills).length === 0
        ? {}
        : { skillSelections: selectedSkills }),
      ...(retryClientTurnId.current
        ? { clientTurnId: retryClientTurnId.current }
        : {}),
    };
    onSend(payload);
    retryClientTurnId.current = undefined;
    setSkillSelections({});
    setInstruction("");
    setAttachments([]);
    setMentions([]);
    setClarify(false);
    setAttachmentError(null);
  }

  function handleSend() {
    submit();
  }

  function handleDrop(event: DragEvent<HTMLDivElement>) {
    event.preventDefault();
    dragDepth.current = 0;
    setDragging(false);
    void stageFiles(event.dataTransfer.files);
  }

  function handlePaste(event: ClipboardEvent<HTMLTextAreaElement>) {
    const images = Array.from(event.clipboardData.files).filter((file) =>
      file.type.startsWith("image/"),
    );
    if (images.length > 0) void stageFiles(images);
  }

  return (
    <div
      data-testid="prompt-drop-zone"
      className="relative border-t border-border p-3"
      onDragEnter={(event) => {
        if (!event.dataTransfer.types.includes("Files")) return;
        event.preventDefault();
        dragDepth.current += 1;
        setDragging(true);
      }}
      onDragOver={(event) => {
        if (!event.dataTransfer.types.includes("Files")) return;
        event.preventDefault();
        event.dataTransfer.dropEffect = "copy";
      }}
      onDragLeave={(event) => {
        event.preventDefault();
        dragDepth.current = Math.max(0, dragDepth.current - 1);
        if (dragDepth.current === 0) setDragging(false);
      }}
      onDrop={handleDrop}
    >
      {(turnInFlight || autonomousRun !== null) && (
        <AgentTurnStatus
          turn={state.turn}
          autonomousRun={autonomousRun}
          onCancel={onCancel}
          onPause={onAutonomousPause}
          onResume={onAutonomousResume}
          onStop={onAutonomousStop}
          cancelDisabled={actionBusy}
        />
      )}
      {dragging && !attachmentInputDisabled && (
        <div
          role="status"
          className="pointer-events-none absolute inset-2 z-20 flex items-center justify-center rounded-lg border-2 border-dashed border-emerald-500/60 bg-background/95 text-sm font-medium text-emerald-500 shadow-sm"
        >
          여기에 놓아 첨부
        </div>
      )}
      <PromptInput onSubmit={handleSend}>
        {attachments.length > 0 && (
          <div
            data-align="block-start"
            className="flex w-full flex-wrap gap-2 border-b border-border/70 px-3 py-2"
          >
            {attachments.map((attachment) => {
              const preview =
                attachment.previewUrl?.startsWith("data:image/") === true
                  ? attachment.previewUrl
                  : null;
              return (
                <div
                  key={attachment.id}
                  className="flex min-w-0 max-w-64 items-center gap-2 rounded-lg border border-border bg-muted/45 p-1.5 pr-1 text-left"
                  title={attachment.name}
                >
                  {preview !== null ? (
                    <img
                      src={preview}
                      alt=""
                      className="size-9 shrink-0 rounded-md border border-border object-cover"
                    />
                  ) : (
                    <span className="flex size-9 shrink-0 items-center justify-center rounded-md bg-background text-muted-foreground">
                      {attachment.kind === "image" ? (
                        <ImageIcon className="size-4" />
                      ) : attachment.kind === "audio" ? (
                        <AudioLinesIcon className="size-4" />
                      ) : (
                        <FileTextIcon className="size-4" />
                      )}
                    </span>
                  )}
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-xs font-medium text-foreground">
                      {attachment.name}
                    </span>
                    <span className="block text-[11px] text-muted-foreground">
                      {formatAttachmentSize(attachment.size)}
                    </span>
                  </span>
                  <button
                    type="button"
                    aria-label={`${attachment.name} 첨부 제거`}
                    disabled={staging}
                    className="flex size-7 shrink-0 cursor-pointer items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-background hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-default disabled:opacity-50"
                    onClick={() => void removeAttachment(attachment)}
                  >
                    <XIcon className="size-3.5" />
                  </button>
                </div>
              );
            })}
          </div>
        )}
        {activeSkills.length > 0 && (
          <ul
            data-align="block-start"
            aria-label="사용 중인 스킬"
            className="flex w-full flex-wrap gap-1.5 px-3 pt-2"
          >
            {activeSkills.map((skill) => (
              <li
                key={skill.id}
                className="flex min-w-0 max-w-64 items-center gap-1 rounded-md border border-primary/40 bg-primary/10 py-0.5 pr-0.5 pl-2 text-xs font-medium text-foreground"
                title={skill.description}
              >
                <SparklesIcon
                  className="size-3.5 shrink-0 text-primary"
                  aria-hidden="true"
                />
                <span className="min-w-0 truncate">{skill.title}</span>
                <button
                  type="button"
                  aria-label={`${skill.title} 스킬 끄기`}
                  disabled={skillPickerDisabled}
                  className="flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-sm text-muted-foreground transition-colors hover:bg-background hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-default disabled:opacity-50"
                  onClick={() => setSkillActive(skill.id, false)}
                >
                  <XIcon className="size-3.5" aria-hidden="true" />
                </button>
              </li>
            ))}
          </ul>
        )}
        <MentionComposer
          text={instruction}
          onTextChange={setInstruction}
          mentions={mentions}
          onMentionsChange={setMentions}
          search={onMentionSearch}
          fileSearch={
            onReadProjectFile !== undefined && onStageAttachment !== undefined
              ? onProjectFileSearch
              : undefined
          }
          onAttachFile={(file) => void attachProjectFile(file)}
          projectIdentity={projectIdentity}
          scopeIdentity={scopeIdentity}
          disabled={ragLoading || actionBusy}
          placeholder={placeholder}
          textareaRef={textareaRef}
          onPaste={handlePaste}
        />
        <PromptInputFooter className="flex-wrap">
          <PromptInputTools className="flex-wrap">
            <DropdownMenu modal={false}>
              <DropdownMenuTrigger asChild>
                <PromptInputButton
                  type="button"
                  variant={activeSkills.length > 0 ? "secondary" : "ghost"}
                  aria-label="스킬 선택"
                  title={
                    skillsAvailable
                      ? "스킬 선택"
                      : "스킬은 EPS 대화에서만 사용할 수 있습니다"
                  }
                  className="min-h-11 min-w-11"
                  disabled={skillPickerDisabled}
                >
                  <PlusIcon className="size-4" aria-hidden="true" />
                </PromptInputButton>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="start" side="top" className="w-72">
                <DropdownMenuLabel>스킬</DropdownMenuLabel>
                {skills.map((skill) => (
                  <DropdownMenuCheckboxItem
                    key={skill.id}
                    checked={activeSkillIds.includes(skill.id)}
                    onCheckedChange={(checked) =>
                      setSkillActive(skill.id, checked === true)
                    }
                    onSelect={(event) => event.preventDefault()}
                  >
                    <span className="flex min-w-0 flex-col gap-0.5">
                      <span className="font-medium">{skill.title}</span>
                      {skill.description !== "" && (
                        <span className="line-clamp-2 text-xs text-muted-foreground">
                          {skill.description}
                        </span>
                      )}
                    </span>
                  </DropdownMenuCheckboxItem>
                ))}
              </DropdownMenuContent>
            </DropdownMenu>
            {onStageAttachment !== undefined && (
              <>
                <input
                  ref={fileInput}
                  type="file"
                  multiple
                  aria-label="파일 첨부"
                  className="hidden"
                  accept="image/png,image/jpeg,image/webp,image/gif,audio/*,text/*,.eps,.json,.md,.csv,.xml,.yaml,.yml,.toml,.js,.ts,.tsx,.py,.rs,.lua,.wav,.ogg,.mp3,.flac,.m4a,.aac,.wma,.aiff,.aif,.opus"
                  disabled={attachmentInputDisabled}
                  onChange={(event) => {
                    if (event.target.files !== null) {
                      void stageFiles(event.target.files);
                    }
                  }}
                />
                <PromptInputButton
                  type="button"
                  aria-label="첨부"
                  disabled={attachmentInputDisabled}
                  onClick={() => fileInput.current?.click()}
                >
                  {staging ? (
                    <LoaderCircleIcon className="size-4 animate-spin motion-reduce:animate-none" />
                  ) : (
                    <PaperclipIcon className="size-4" />
                  )}
                  첨부
                </PromptInputButton>
              </>
            )}
            <PromptInputButton
              type="button"
              variant="ghost"
              className={
                clarifyOn
                  ? "bg-primary/15 text-primary hover:bg-primary/25 hover:text-primary"
                  : undefined
              }
              aria-pressed={clarifyOn}
              aria-label="인터뷰"
              disabled={!clarifyAvailable}
              title={
                clarifyAvailable
                  ? "보내기 전에 요구사항을 먼저 묻고 확정합니다. 이 메시지 한 번에만 적용됩니다."
                  : turnInFlight
                    ? "작업이 진행 중일 때는 인터뷰를 켤 수 없습니다. 작업이 끝난 뒤 새 요청에서 켜세요."
                    : "계획 검토 중에는 인터뷰를 켤 수 없습니다. 계획을 승인하거나 새 요청으로 시작하세요."
              }
              onClick={() => setClarify((value) => !value)}
            >
              <MessageCircleQuestionMarkIcon className="size-4" />
              인터뷰
            </PromptInputButton>
            <ProviderPromptControls
              settings={modelSettings}
              busy={modelSettingsBusy}
              disabled={turnInFlight || actionBusy}
              contextUsage={state.contextUsage}
              onChange={onModelSettingsChange}
              onReload={onModelSettingsReload}
            />
          </PromptInputTools>
          <PromptInputSubmit
            aria-label={steering ? "작업 중인 AI에게 전달" : "실행"}
            disabled={!canSend || staging || hasStaleMention}
          >
            {steering ? (
              <CornerDownRightIcon className="size-4" />
            ) : (
              <SendIcon className="size-4" />
            )}
            {steering ? "전달" : "실행"}
          </PromptInputSubmit>
        </PromptInputFooter>
      </PromptInput>
      {attachmentError !== null && (
        <p role="alert" className="mt-1.5 text-xs text-destructive">
          {attachmentError}
        </p>
      )}
    </div>
  );
}
