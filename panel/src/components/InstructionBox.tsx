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
  RefreshCwIcon,
  SendIcon,
  SquareIcon,
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
import { Shimmer } from "@/components/ai-elements/shimmer";
import {
  Context,
  ContextContent,
  ContextContentBody,
  ContextContentHeader,
  ContextTrigger,
} from "@/components/ai-elements/context";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { PanelState } from "@/state/store";
import type {
  ChatAttachment,
  CodexModelSettings,
  ContextUsage,
} from "@/lib/ipc";
import {
  attachmentErrorMessage,
  formatAttachmentSize,
  MAX_ATTACHMENTS_PER_TURN,
  MAX_TEXT_BYTES,
} from "@/lib/attachments";
const REASONING_LABELS: Readonly<Record<string, string>> = {
  none: "추론 없음",
  minimal: "추론 최소",
  low: "추론 낮음",
  medium: "추론 보통",
  high: "추론 높음",
  xhigh: "추론 매우 높음",
  ultra: "추론 최고",
};
const CONTEXT_TOKEN_FORMATTER = new Intl.NumberFormat("ko-KR", {
  notation: "compact",
  maximumFractionDigits: 1,
});

function ContextUsageDetails({ usage }: { usage: ContextUsage }) {
  const rows = [
    ["입력", usage.total.inputTokens],
    ["캐시 입력", usage.total.cachedInputTokens],
    ["출력", usage.total.outputTokens],
    ["추론", usage.total.reasoningOutputTokens],
  ] as const;

  return (
    <div className="space-y-2 text-xs">
      <div className="flex items-center justify-between gap-3 font-medium">
        <span>세션 누적</span>
        <span className="font-mono tabular-nums">
          {CONTEXT_TOKEN_FORMATTER.format(usage.total.totalTokens)}
        </span>
      </div>
      <dl className="space-y-1.5">
        {rows.map(([label, tokens]) => (
          <div
            key={label}
            className="flex items-center justify-between gap-3 text-muted-foreground"
          >
            <dt>{label}</dt>
            <dd className="font-mono tabular-nums">
              {CONTEXT_TOKEN_FORMATTER.format(tokens)}
            </dd>
          </div>
        ))}
      </dl>
    </div>
  );
}

function turnActivityLabel(state: PanelState): string {
  for (let index = state.turn.tools.length - 1; index >= 0; index -= 1) {
    const tool = state.turn.tools[index];
    if (tool.state === "running") return `도구 실행 중 · ${tool.name}`;
  }
  if (state.turn.answerStarted) return "응답 작성 중";
  if (state.turn.tools.length > 0) return "도구 결과 확인 중";
  if (state.turn.reasoning.length > 0) return "추론 중";
  return "작업 준비 중";
}


export interface ChatPayload {
  text: string;
  attachments: ChatAttachment[];
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

  useEffect(() => {
    if (draft === undefined || draft === null) return;
    setInstruction(draft.text);
    setAttachments([...draft.attachments]);
    setAttachmentError(null);
    textareaRef.current?.focus();
  }, [draft]);

  // Send gating v2: the store's single `canSend` selector (connected &&
  // hasProject && !busy). Empty text is valid only when a staged attachment exists.
  const canSend = state.canSend && !actionBusy;
  const turnInFlight = state.phase === "thinking";
  const ragLoading = state.rag === "loading";
  const editorDisconnected = !state.editorConnected;
  const selectedModel = codexSettings?.models.find(
    (model) => model.model === codexSettings.selectedModel,
  );
  const codexSettingsDisabled =
    turnInFlight ||
    actionBusy ||
    codexSettingsBusy ||
    onCodexSettingsChange === undefined;
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
    onSend({ text, attachments });
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
        <div
          data-testid="active-turn-status"
          role="status"
          aria-live="polite"
          aria-atomic="true"
          className="mb-2 flex min-h-11 items-center justify-between gap-3 rounded-lg border border-border bg-card/95 px-3 shadow-sm"
        >
          <div className="flex min-w-0 items-center gap-2 text-sm">
            <span
              aria-hidden
              className="size-2 shrink-0 animate-pulse rounded-full bg-emerald-400 motion-reduce:animate-none"
            />
            <Shimmer className="truncate">{turnActivityLabel(state)}</Shimmer>
          </div>
          {onCancel && (
            <button
              type="button"
              aria-label="작업 중단"
              disabled={actionBusy}
              onClick={onCancel}
              className="flex min-h-11 shrink-0 cursor-pointer items-center gap-1.5 rounded-md px-3 text-sm text-muted-foreground transition-colors duration-200 hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-default disabled:opacity-50"
            >
              <SquareIcon aria-hidden className="size-3.5 fill-current" />
              중단
            </button>
          )}
        </div>
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
            {codexSettings && selectedModel ? (
              <>
                <Select
                  value={codexSettings.selectedModel}
                  disabled={codexSettingsDisabled}
                  onValueChange={(modelId) => {
                    const model = codexSettings.models.find(
                      (candidate) => candidate.model === modelId,
                    );
                    if (model) {
                      onCodexSettingsChange?.(
                        model.model,
                        model.defaultReasoningEffort,
                      );
                    }
                  }}
                >
                  <SelectTrigger
                    size="sm"
                    aria-label="Codex 모델"
                    className="max-w-44"
                  >
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent position="popper">
                    {codexSettings.models.map((model) => (
                      <SelectItem
                        key={model.model}
                        value={model.model}
                        title={model.description}
                      >
                        {model.displayName}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                <Select
                  value={codexSettings.selectedReasoningEffort}
                  disabled={codexSettingsDisabled}
                  onValueChange={(reasoningEffort) =>
                    onCodexSettingsChange?.(
                      codexSettings.selectedModel,
                      reasoningEffort,
                    )
                  }
                >
                  <SelectTrigger
                    size="sm"
                    aria-label="추론 단계"
                    className="max-w-32"
                  >
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent position="popper">
                    {selectedModel.supportedReasoningEfforts.map((option) => (
                      <SelectItem
                        key={option.reasoningEffort}
                        value={option.reasoningEffort}
                        title={option.description}
                      >
                        {REASONING_LABELS[option.reasoningEffort] ??
                          option.reasoningEffort}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </>
            ) : onCodexSettingsReload ? (
              <PromptInputButton
                type="button"
                aria-label="Codex 모델 다시 불러오기"
                disabled={codexSettingsBusy}
                onClick={onCodexSettingsReload}
              >
                <RefreshCwIcon
                  className={`size-4 ${codexSettingsBusy ? "animate-spin" : ""}`}
                />
                {codexSettingsBusy
                  ? "모델 불러오는 중…"
                  : "모델 다시 불러오기"}
              </PromptInputButton>
            ) : null}
            {state.contextUsage?.modelContextWindow !== null &&
              state.contextUsage?.modelContextWindow !== undefined &&
              state.contextUsage.modelContextWindow > 0 && (
                <Context
                  usedTokens={Math.max(0, state.contextUsage.last.totalTokens)}
                  maxTokens={state.contextUsage.modelContextWindow}
                >
                  <ContextTrigger />
                  <ContextContent side="top" align="end">
                    <ContextContentHeader />
                    <ContextContentBody>
                      <ContextUsageDetails usage={state.contextUsage} />
                    </ContextContentBody>
                  </ContextContent>
                </Context>
              )}
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
