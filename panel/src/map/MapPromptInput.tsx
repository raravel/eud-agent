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

import { AgentTurnStatus } from "@/components/AgentTurnStatus";
import { ProviderPromptControls } from "@/components/ProviderPromptControls";
import {
  PromptInput,
  PromptInputBody,
  PromptInputButton,
  PromptInputFooter,
  PromptInputSubmit,
  PromptInputTextarea,
  PromptInputTools,
} from "@/components/ai-elements/prompt-input";
import {
  attachmentErrorMessage,
  formatAttachmentSize,
  MAX_ATTACHMENTS_PER_TURN,
  MAX_TEXT_BYTES,
} from "@/lib/attachments";
import type {
  ChatAttachment,
  ContextUsage,
  ReasoningSelection,
  SessionModelSettings,
} from "@/lib/ipc";
import type { TurnState } from "@/state/store";

export interface MapPromptInputProps {
  turn: TurnState;
  live: boolean;
  actionBusy?: boolean;
  mentionCount: number;
  hasStaleMentions: boolean;
  draftScope: string;
  contextUsage?: ContextUsage | null;
  modelSettings?: SessionModelSettings | null;
  modelSettingsBusy?: boolean;
  onSend(text: string, attachments: ChatAttachment[]): void;
  onCancel(): void;
  onStageAttachment?(file: File): Promise<ChatAttachment>;
  onDiscardAttachment?(id: string): Promise<void>;
  onModelSettingsChange?(
    model: string,
    reasoning: ReasoningSelection | undefined,
  ): void;
  onModelSettingsReload?(): void;
}

export function MapPromptInput({
  turn,
  live,
  actionBusy = false,
  mentionCount,
  hasStaleMentions,
  draftScope,
  contextUsage,
  modelSettings,
  modelSettingsBusy = false,
  onSend,
  onCancel,
  onStageAttachment,
  onDiscardAttachment,
  onModelSettingsChange,
  onModelSettingsReload,
}: MapPromptInputProps) {
  const [text, setText] = useState("");
  const [attachments, setAttachments] = useState<ChatAttachment[]>([]);
  const [attachmentError, setAttachmentError] = useState<string | null>(null);
  const [staging, setStaging] = useState(false);
  const [dragging, setDragging] = useState(false);
  const stagingRef = useRef(false);
  const dragDepth = useRef(0);
  const fileInput = useRef<HTMLInputElement>(null);

  useEffect(() => {
    setText("");
  }, [draftScope]);

  const attachmentInputDisabled =
    live || actionBusy || staging || onStageAttachment === undefined;
  const canSend =
    !live &&
    !actionBusy &&
    !staging &&
    !hasStaleMentions &&
    (text.trim().length > 0 || mentionCount > 0 || attachments.length > 0);

  async function stageFiles(source: FileList | readonly File[]) {
    if (
      onStageAttachment === undefined ||
      stagingRef.current ||
      live ||
      actionBusy
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
    if (!canSend || stagingRef.current) return;
    onSend(text, attachments);
    setText("");
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
      data-testid="map-prompt-drop-zone"
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
      {live && (
        <AgentTurnStatus
          turn={turn}
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
                  className="flex min-w-0 max-w-full items-center gap-2 rounded-lg border border-border bg-muted/45 p-1.5 pr-1 text-left"
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
            aria-label="맵 요청 입력"
            value={text}
            disabled={actionBusy}
            placeholder="예: target 영역 안에 P5 벙커 2개와 어울리는 정글 지형을 구성해줘"
            onChange={(event) => setText(event.target.value)}
            onPaste={handlePaste}
          />
        </PromptInputBody>
        <PromptInputFooter className="flex-wrap gap-2">
          <PromptInputTools className="min-w-0 flex-1 flex-wrap">
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
            <ProviderPromptControls
              settings={modelSettings}
              busy={modelSettingsBusy}
              disabled={live || actionBusy}
              contextUsage={contextUsage}
              onChange={onModelSettingsChange}
              onReload={onModelSettingsReload}
            />
          </PromptInputTools>
          <PromptInputSubmit className="ml-auto" aria-label="전송" disabled={!canSend}>
            <SendIcon className="size-4" aria-hidden="true" />
            전송
          </PromptInputSubmit>
        </PromptInputFooter>
      </PromptInput>
      {hasStaleMentions && (
        <p role="alert" className="mt-1.5 text-xs text-destructive">
          현재 후보와 맞지 않는 맵 멘션을 제거하거나 다시 선택하세요.
        </p>
      )}
      {attachmentError !== null && (
        <p role="alert" className="mt-1.5 text-xs text-destructive">
          {attachmentError}
        </p>
      )}
    </div>
  );
}
