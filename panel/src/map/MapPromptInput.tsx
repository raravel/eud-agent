import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type ClipboardEvent,
  type DragEvent,
} from "react";
import {
  FileTextIcon,
  FocusIcon,
  ImageIcon,
  LoaderCircleIcon,
  MapPinnedIcon,
  PaperclipIcon,
  SendIcon,
  XIcon,
} from "lucide-react";

import { AgentTurnStatus } from "@/components/AgentTurnStatus";
import { activeMentionFragment } from "@/components/MentionComposer";
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
import { cn } from "@/lib/utils";
import type { TurnState } from "@/state/store";
import {
  mapMentionSuggestions,
  type MapMentionSuggestion,
} from "./mapMentionSuggest";
import type { MapLocation, SavedSelection } from "./mapProtocol";

const MENTION_LISTBOX_ID = "map-mention-listbox";

/** A past user message restored into the prompt after a rewind. */
export interface MapPromptDraft {
  text: string;
  attachments: ChatAttachment[];
}

export interface MapPromptInputProps {
  turn: TurnState;
  live: boolean;
  actionBusy?: boolean;
  mentionCount: number;
  hasStaleMentions: boolean;
  draftScope: string;
  /** Applied once per object identity; later typing is not controlled by it. */
  draft?: MapPromptDraft | null;
  contextUsage?: ContextUsage | null;
  modelSettings?: SessionModelSettings | null;
  modelSettingsBusy?: boolean;
  /** `@` completion sources; picking one adds a tray chip through the callbacks below. */
  locations?: MapLocation[];
  selections?: SavedSelection[];
  onLocationMention?(location: MapLocation): void;
  onRegionMention?(selection: SavedSelection): void;
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
  draft,
  contextUsage,
  modelSettings,
  modelSettingsBusy = false,
  locations = [],
  selections = [],
  onLocationMention,
  onRegionMention,
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
  const [fragment, setFragment] =
    useState<ReturnType<typeof activeMentionFragment>>(null);
  const [activeIndex, setActiveIndex] = useState(0);
  const stagingRef = useRef(false);
  const dragDepth = useRef(0);
  const fileInput = useRef<HTMLInputElement>(null);
  const textarea = useRef<HTMLTextAreaElement>(null);
  const composingRef = useRef(false);
  const dismissedFragmentRef = useRef<string | null>(null);

  useEffect(() => {
    setText("");
    setFragment(null);
  }, [draftScope]);

  useEffect(() => {
    if (draft === undefined || draft === null) return;
    setText(draft.text);
    setAttachments([...draft.attachments]);
    setAttachmentError(null);
    setFragment(null);
    requestAnimationFrame(() => textarea.current?.focus());
  }, [draft]);

  const mentionSourcesEnabled =
    onLocationMention !== undefined || onRegionMention !== undefined;
  const listboxOpen = fragment !== null && mentionSourcesEnabled && !actionBusy;
  const suggestions = useMemo(
    () =>
      listboxOpen && fragment !== null
        ? mapMentionSuggestions(fragment.query, selections, locations)
        : [],
    [fragment, listboxOpen, locations, selections],
  );
  const activeSuggestion = suggestions[activeIndex];

  useEffect(() => {
    setActiveIndex(0);
  }, [fragment?.key]);

  function updateFragment(value: string, caret: number | null) {
    if (composingRef.current || caret === null) return;
    const next = activeMentionFragment(value, caret);
    if (next?.key === dismissedFragmentRef.current) {
      setFragment(null);
      return;
    }
    dismissedFragmentRef.current = null;
    setFragment((current) => (next?.key === current?.key ? current : next));
  }

  function selectSuggestion(suggestion: MapMentionSuggestion) {
    if (composingRef.current || fragment === null) return;
    if (suggestion.kind === "location") onLocationMention?.(suggestion.location);
    else onRegionMention?.(suggestion.selection);
    const caret = fragment.start;
    setText(text.slice(0, fragment.start) + text.slice(fragment.end));
    setFragment(null);
    requestAnimationFrame(() => {
      textarea.current?.focus();
      textarea.current?.setSelectionRange(caret, caret);
    });
  }

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
    setFragment(null);
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
            ref={textarea}
            role="combobox"
            aria-autocomplete="list"
            aria-expanded={listboxOpen}
            aria-controls={listboxOpen ? MENTION_LISTBOX_ID : undefined}
            aria-activedescendant={
              listboxOpen && activeSuggestion !== undefined
                ? `${MENTION_LISTBOX_ID}-option-${activeIndex}`
                : undefined
            }
            aria-label="맵 요청 입력"
            value={text}
            disabled={actionBusy}
            placeholder="예: @target 영역 안에 P5 벙커 2개와 어울리는 정글 지형을 구성해줘"
            onChange={(event) => {
              setText(event.target.value);
              updateFragment(event.target.value, event.target.selectionStart);
            }}
            onClick={(event) =>
              updateFragment(event.currentTarget.value, event.currentTarget.selectionStart)
            }
            onSelect={(event) =>
              updateFragment(event.currentTarget.value, event.currentTarget.selectionStart)
            }
            onKeyUp={(event) => {
              if (event.key !== "Escape") {
                updateFragment(event.currentTarget.value, event.currentTarget.selectionStart);
              }
            }}
            onKeyDown={(event) => {
              if (composingRef.current || event.nativeEvent.isComposing || !listboxOpen) {
                return;
              }
              if (event.key === "ArrowDown") {
                event.preventDefault();
                setActiveIndex((index) =>
                  suggestions.length === 0 ? 0 : (index + 1) % suggestions.length,
                );
              } else if (event.key === "ArrowUp") {
                event.preventDefault();
                setActiveIndex((index) =>
                  suggestions.length === 0
                    ? 0
                    : (index - 1 + suggestions.length) % suggestions.length,
                );
              } else if (event.key === "Enter") {
                event.preventDefault();
                if (activeSuggestion !== undefined) selectSuggestion(activeSuggestion);
              } else if (event.key === "Escape") {
                event.preventDefault();
                dismissedFragmentRef.current = fragment?.key ?? null;
                setFragment(null);
              }
            }}
            onCompositionStart={() => {
              composingRef.current = true;
              setFragment(null);
            }}
            onCompositionEnd={(event) => {
              composingRef.current = false;
              updateFragment(event.currentTarget.value, event.currentTarget.selectionStart);
            }}
            onPaste={handlePaste}
          />
        </PromptInputBody>
        {listboxOpen && (
          <div
            id={MENTION_LISTBOX_ID}
            role="listbox"
            aria-label="맵 멘션 검색 결과"
            className="mx-2 mb-1 max-h-56 w-[calc(100%-1rem)] overflow-y-auto rounded-md border border-border bg-popover p-1 text-popover-foreground shadow-lg"
          >
            {suggestions.length === 0 ? (
              <p role="status" className="px-2 py-2 text-xs text-muted-foreground">
                {selections.length === 0 && locations.length === 0
                  ? "멘션할 저장 영역이나 로케이션이 없습니다. 캔버스에서 영역을 선택해 저장하세요."
                  : "일치하는 저장 영역이나 로케이션이 없습니다."}
              </p>
            ) : (
              suggestions.map((suggestion, index) => (
                <div
                  id={`${MENTION_LISTBOX_ID}-option-${index}`}
                  key={suggestion.key}
                  role="option"
                  aria-selected={index === activeIndex}
                  data-mention-kind={suggestion.kind}
                  className={cn(
                    "flex cursor-pointer items-start gap-2 rounded px-2 py-2 text-sm",
                    index === activeIndex ? "bg-accent" : "hover:bg-accent/60",
                  )}
                  onMouseDown={(event) => event.preventDefault()}
                  onMouseEnter={() => setActiveIndex(index)}
                  onClick={() => selectSuggestion(suggestion)}
                >
                  <span className="mt-0.5 shrink-0 text-emerald-400">
                    {suggestion.kind === "region" ? (
                      <MapPinnedIcon aria-hidden className="size-3.5" />
                    ) : (
                      <FocusIcon aria-hidden className="size-3.5" />
                    )}
                  </span>
                  <span className="min-w-0">
                    <span className="block truncate font-medium">@{suggestion.label}</span>
                    <span className="block truncate text-xs text-muted-foreground">
                      {suggestion.detail}
                    </span>
                  </span>
                </div>
              ))
            )}
          </div>
        )}
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
