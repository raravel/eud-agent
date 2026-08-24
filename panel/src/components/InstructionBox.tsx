/**
 * Main chat/plan-feedback input:
 *   - `chat {sessionId, text, attachments}` / session-scoped `plan_feedback`;
 *   - file picker, HTML5 drag-and-drop, and pasted clipboard images;
 *   - removable draft chips with generated image thumbnails;
 *   - Send is gated by the store's `canSend`; attachment-only messages are valid.
 *
 * The textarea keeps the accessible name "지시 입력"; visible controls are
 * labelled "첨부" and "전송". Enter (without Shift / IME composition)
 * submits through PromptInput.
 */
import {
  useEffect,
  useRef,
  useState,
  type ClipboardEvent,
  type DragEvent,
} from "react";
import {
  FileTextIcon,
  ImageIcon,
  LoaderCircleIcon,
  PaperclipIcon,
  SendIcon,
  XIcon,
} from "lucide-react";
import {
  PromptInput,
  PromptInputBody,
  PromptInputFooter,
  PromptInputSubmit,
  PromptInputTextarea,
  PromptInputTools,
} from "@/components/ai-elements/prompt-input";
import { PromptInputButton } from "@/components/ai-elements/prompt-input";
import { CodexPromptControls } from "@/components/CodexPromptControls";
import { AgentTurnStatus } from "@/components/AgentTurnStatus";
import type { PanelState } from "@/state/store";
import type { ChatAttachment, CodexModelSettings } from "@/lib/ipc";
import {
  attachmentErrorMessage,
  formatAttachmentSize,
  MAX_ATTACHMENTS_PER_TURN,
  MAX_TEXT_BYTES,
} from "@/lib/attachments";



export interface ChatPayload {
  text: string;
  attachments: ChatAttachment[];
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
  /** A cancel/rewind command is waiting for the core. */
  actionBusy?: boolean;
  /** Current Codex catalog + persisted selection. */
  codexSettings?: CodexModelSettings | null;
  /** Catalog load or settings save in flight. */
  codexSettingsBusy?: boolean;
  /** Persist and apply the pair to subsequent Codex turns. */
  onCodexSettingsChange?(model: string, reasoningEffort: string): void;
  /** Retry loading the catalog after startup or a transient failure. */
  onCodexSettingsReload?(): void;
}

export function InstructionBox({
  state,
  onSend,
  onStageAttachment,
  onDiscardAttachment,
  onCancel,
  draft,
  actionBusy = false,
  codexSettings,
  codexSettingsBusy = false,
  onCodexSettingsChange,
  onCodexSettingsReload,
}: InstructionBoxProps) {
  const [instruction, setInstruction] = useState("");
  const [attachments, setAttachments] = useState<ChatAttachment[]>([]);
  const [attachmentError, setAttachmentError] = useState<string | null>(null);
  const [staging, setStaging] = useState(false);
  const [dragging, setDragging] = useState(false);
  const stagingRef = useRef(false);
  const dragDepth = useRef(0);
  const fileInput = useRef<HTMLInputElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const retryClientTurnId = useRef<string | undefined>(undefined);

  useEffect(() => {
    if (draft === undefined || draft === null) return;
    setInstruction(draft.text);
    setAttachments([...draft.attachments]);
    retryClientTurnId.current = draft.clientTurnId;
    setAttachmentError(null);
    textareaRef.current?.focus();
  }, [draft]);

  // Send gating v2: the store's single `canSend` selector (connected &&
  // hasProject && !busy). Empty text is valid only when a staged attachment exists.
  const canSend = state.canSend && !actionBusy;
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
      (text.length === 0 && attachments.length === 0)
    ) {
      return;
    }
    onSend({
      text,
      attachments,
      ...(retryClientTurnId.current ? { clientTurnId: retryClientTurnId.current } : {}),
    });
    retryClientTurnId.current = undefined;
    setInstruction("");
    setAttachments([]);
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
        <PromptInputBody>
          <PromptInputTextarea
            ref={textareaRef}
            aria-label="지시 입력"
            value={instruction}
            onChange={(event) => setInstruction(event.target.value)}
            onPaste={handlePaste}
            placeholder={placeholder}
            disabled={ragLoading || actionBusy}
          />
        </PromptInputBody>
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
                  accept="image/png,image/jpeg,image/webp,image/gif,text/*,.eps,.json,.md,.csv,.xml,.yaml,.yml,.toml,.js,.ts,.tsx,.py,.rs,.lua"
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
            <CodexPromptControls
              settings={codexSettings}
              busy={codexSettingsBusy}
              disabled={turnInFlight || actionBusy}
              contextUsage={state.contextUsage}
              onChange={onCodexSettingsChange}
              onReload={onCodexSettingsReload}
            />
          </PromptInputTools>
          <PromptInputSubmit aria-label="전송" disabled={!canSend || staging}>
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
