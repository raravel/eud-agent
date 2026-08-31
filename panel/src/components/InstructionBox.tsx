/**
 * Main chat/plan-feedback input:
 *   - generic ordered mentions plus text and session-owned attachments;
 *   - file picker, HTML5 drag-and-drop, and pasted clipboard images;
 *   - removable attachment and resource-mention chips;
 *   - Send is gated by the store's `canSend`; attachment-only and mention-only messages are valid.
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
  LoaderCircleIcon,
  PaperclipIcon,
  SendIcon,
  XIcon,
} from "lucide-react";
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
import type { PanelState } from "@/state/store";
import type {
  ChatAttachment,
  MentionInstance,
  MentionSearchRequest,
  MentionSearchResponse,
  ReasoningSelection,
  SessionModelSettings,
} from "@/lib/ipc";
import {
  attachmentErrorMessage,
  formatAttachmentSize,
  MAX_ATTACHMENTS_PER_TURN,
  MAX_AUDIO_BYTES_PER_TURN,
  MAX_TEXT_BYTES,
} from "@/lib/attachments";



export interface ChatPayload {
  text: string;
  attachments: ChatAttachment[];
  mentions: MentionInstance[];
  /** Preserved only when retrying a transport-rejected submission. */
  clientTurnId?: string;
}

export interface InstructionBoxProps {
  state: PanelState;
  onSend(msg: ChatPayload): void;
  /** Copy one browser File into app-owned attachment storage. */
  onStageAttachment?(file: File): Promise<ChatAttachment>;
  /** Delete an attachment draft removed before send. */
  onDiscardAttachment?(id: string): Promise<void>;
  /** Interrupt the active turn without discarding its prior conversation. */
  onCancel?(): void;
  /** A past user message restored for editing after a successful rewind. */
  draft?: ChatPayload | null;
  /** Backend-owned bounded resource search used by the generic composer. */
  onMentionSearch?(request: MentionSearchRequest): Promise<MentionSearchResponse>;
  /** Current editor project identity; a change invalidates unsent mention snapshots. */
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
}

export function InstructionBox({
  state,
  onSend,
  onStageAttachment,
  onDiscardAttachment,
  onCancel,
  draft,
  onMentionSearch,
  projectIdentity = state.project,
  scopeIdentity = "default",
  actionBusy = false,
  modelSettings,
  modelSettingsBusy = false,
  onModelSettingsChange,
  onModelSettingsReload,
}: InstructionBoxProps) {
  const [instruction, setInstruction] = useState("");
  const [attachments, setAttachments] = useState<ChatAttachment[]>([]);
  const [mentions, setMentions] = useState<MentionInstance[]>([]);
  const [attachmentError, setAttachmentError] = useState<string | null>(null);
  const [staging, setStaging] = useState(false);
  const [dragging, setDragging] = useState(false);
  const stagingRef = useRef(false);
  const retryClientTurnId = useRef<string | undefined>(undefined);
  const dragDepth = useRef(0);
  const fileInput = useRef<HTMLInputElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    if (draft === undefined || draft === null) return;
    setInstruction(draft.text);
    setAttachments([...draft.attachments]);
    setMentions(draft.mentions.map((mention) => ({ ...mention })));
    retryClientTurnId.current = draft.clientTurnId;
    setAttachmentError(null);
    textareaRef.current?.focus();
  }, [draft]);

  // The store owns connection/project/busy gating. Empty text remains valid when
  // at least one staged attachment or validated mention is present.
  const canSend = state.canSend && !actionBusy;
  const hasStaleMention = mentions.some((mention) => mention.stale === true);
  const turnInFlight = state.phase === "thinking";
  const ragLoading = state.rag === "loading";
  const editorDisconnected = !state.editorConnected;
  const attachmentInputDisabled =
    !canSend || staging || onStageAttachment === undefined;
  const placeholder = editorDisconnected
    ? "에디터가 연결되지 않았습니다. EUD Editor 3을 실행하세요"
    : ragLoading
      ? "RAG 모델 준비 중… 준비가 끝나면 입력할 수 있습니다"
      : state.phase === "plan_review"
        ? "계획 수정 피드백을 입력하세요 (승인은 계획 카드에서)"
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

  function handleSend() {
    const text = instruction.trim();
    if (
      !canSend ||
      stagingRef.current ||
      hasStaleMention ||
      (text.length === 0 && attachments.length === 0 && mentions.length === 0)
    ) {
      return;
    }
    onSend({
      text,
      attachments,
      mentions,
      ...(retryClientTurnId.current
        ? { clientTurnId: retryClientTurnId.current }
        : {}),
    });
    retryClientTurnId.current = undefined;
    setInstruction("");
    setAttachments([]);
    setMentions([]);
    setAttachmentError(null);
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
      {turnInFlight && (
        <AgentTurnStatus
          turn={state.turn}
          onCancel={onCancel}
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
        <MentionComposer
          text={instruction}
          onTextChange={setInstruction}
          mentions={mentions}
          onMentionsChange={setMentions}
          search={onMentionSearch}
          projectIdentity={projectIdentity}
          scopeIdentity={scopeIdentity}
          disabled={ragLoading || actionBusy}
          placeholder={placeholder}
          textareaRef={textareaRef}
          onPaste={handlePaste}
        />
        <PromptInputFooter>
          <PromptInputTools>
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
            aria-label="전송"
            disabled={!canSend || staging || hasStaleMention}
          >
            <SendIcon className="size-4" />
            전송
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
