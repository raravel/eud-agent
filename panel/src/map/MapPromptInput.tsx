import { useTranslation } from "react-i18next";
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
  ImagePlusIcon,
  MapPinnedIcon,
  PaperclipIcon,
  SendIcon,
  CornerDownRightIcon,
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
  MAX_IMAGE_BYTES,
  MAX_TEXT_BYTES,
} from "@/lib/attachments";
import type { ProjectFileSuggestion } from "@/lib/projectFiles";
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
const FILE_SEARCH_DEBOUNCE_MS = 120;

type PromptSuggestion =
  | MapMentionSuggestion
  | {
      key: string;
      kind: "file";
      label: string;
      detail: string;
      file: ProjectFileSuggestion;
    };

/** A past user message restored into the prompt after a rewind. */
export interface MapPromptDraft {
  text: string;
  attachments: ChatAttachment[];
}

export interface MapPromptInputProps {
  turn: TurnState;
  /**
   * A request runs: Send then steers it (the request receives the message at
   * its next tool boundary) and the per-request toggles are off.
   */
  live: boolean;
  /** A question of the running request waits on its card; it is answered there. */
  askPending?: boolean;
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
  /** Project-root files offered after the map resources; picking one attaches it. */
  onProjectFileSearch?(query: string): Promise<ProjectFileSuggestion[]>;
  onReadProjectFile?(file: ProjectFileSuggestion): Promise<File>;
  /** `imagePlacement` is the one-message "사진 배치" toggle. */
  onSend(text: string, attachments: ChatAttachment[], imagePlacement: boolean): void;
  onCancel(): void;
  onStageAttachment?(file: File): Promise<ChatAttachment>;
  onDiscardAttachment?(id: string): Promise<void>;
  onModelSettingsChange?(
    model: string,
    reasoning: ReasoningSelection | undefined,
  ): void;
  onModelSettingsReload?(): void;
  /** Replaces the example request, e.g. where `@` mentions are unavailable. */
  placeholder?: string;
}

export function MapPromptInput({
  turn,
  live,
  askPending = false,
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
  onProjectFileSearch,
  onReadProjectFile,
  onSend,
  onCancel,
  onStageAttachment,
  onDiscardAttachment,
  onModelSettingsChange,
  onModelSettingsReload,
  placeholder,
}: MapPromptInputProps) {
  const { t } = useTranslation();
  const [text, setText] = useState("");
  const [attachments, setAttachments] = useState<ChatAttachment[]>([]);
  const [imagePlacement, setImagePlacement] = useState(false);
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
  const [composing, setComposing] = useState(false);
  const [fileResults, setFileResults] = useState<{
    key: string;
    files: ProjectFileSuggestion[];
  } | null>(null);
  const composingRef = useRef(false);
  // Tab/Enter pressed while the IME was composing or files were loading:
  // complete once the committed query has its results.
  const pendingCompletionRef = useRef(false);
  const dismissedFragmentRef = useRef<string | null>(null);

  useEffect(() => {
    setText("");
    setFragment(null);
    setImagePlacement(false);
  }, [draftScope]);

  useEffect(() => {
    if (draft === undefined || draft === null) return;
    setText(draft.text);
    setAttachments([...draft.attachments]);
    setImagePlacement(false);
    setAttachmentError(null);
    setFragment(null);
    requestAnimationFrame(() => textarea.current?.focus());
  }, [draft]);

  const fileSearch =
    onProjectFileSearch !== undefined &&
    onReadProjectFile !== undefined &&
    onStageAttachment !== undefined
      ? onProjectFileSearch
      : undefined;
  const mentionSourcesEnabled =
    onLocationMention !== undefined ||
    onRegionMention !== undefined ||
    fileSearch !== undefined;
  const listboxOpen = fragment !== null && mentionSourcesEnabled && !actionBusy;
  const filesCurrent =
    fileSearch === undefined ||
    (fragment !== null && fileResults?.key === fragment.key);
  const suggestions = useMemo<PromptSuggestion[]>(() => {
    if (!listboxOpen || fragment === null) return [];
    const map: PromptSuggestion[] =
      onLocationMention !== undefined || onRegionMention !== undefined
        ? mapMentionSuggestions(fragment.query, selections, locations)
        : [];
    const files: PromptSuggestion[] =
      fileResults?.key === fragment.key
        ? fileResults.files.map((file) => ({
            key: `file:${file.path}`,
            kind: "file" as const,
            label: file.path.slice(file.path.lastIndexOf("/") + 1),
            detail: t("map.prompt.fileDetail", { folder: file.path.includes("/") ? file.path.slice(0, file.path.lastIndexOf("/")) : t("map.prompt.projectRoot"), size: formatAttachmentSize(file.size) }),
            file,
          }))
        : [];
    return [...map, ...files];
  }, [
    fileResults,
    fragment,
    listboxOpen,
    locations,
    onLocationMention,
    onRegionMention,
    selections,
    t,
  ]);
  const activeSuggestion = suggestions[activeIndex];

  useEffect(() => {
    setActiveIndex(0);
  }, [fragment?.key]);

  // The query follows the textarea even while an IME composes.
  useEffect(() => {
    if (fragment === null || fileSearch === undefined || actionBusy) return;
    let current = true;
    const timeout = window.setTimeout(() => {
      fileSearch(fragment.query)
        .catch(() => [])
        .then((files) => {
          if (current) setFileResults({ key: fragment.key, files });
        });
    }, FILE_SEARCH_DEBOUNCE_MS);
    return () => {
      current = false;
      window.clearTimeout(timeout);
    };
  }, [actionBusy, fileSearch, fragment]);

  function updateFragment(value: string, caret: number | null) {
    if (caret === null) return;
    const next = activeMentionFragment(value, caret);
    if (next?.key === dismissedFragmentRef.current) {
      setFragment(null);
      return;
    }
    dismissedFragmentRef.current = null;
    setFragment((current) => (next?.key === current?.key ? current : next));
  }

  function selectSuggestion(suggestion: PromptSuggestion) {
    if (fragment === null) return;
    if (suggestion.kind === "location") onLocationMention?.(suggestion.location);
    else if (suggestion.kind === "region") onRegionMention?.(suggestion.selection);
    else void attachProjectFile(suggestion.file);
    const caret = fragment.start;
    setText(text.slice(0, fragment.start) + text.slice(fragment.end));
    setFragment(null);
    requestAnimationFrame(() => {
      textarea.current?.focus();
      textarea.current?.setSelectionRange(caret, caret);
    });
  }

  useEffect(() => {
    if (!pendingCompletionRef.current || composing) return;
    if (fragment === null) {
      pendingCompletionRef.current = false;
      return;
    }
    if (!filesCurrent) return;
    pendingCompletionRef.current = false;
    const suggestion = suggestions[activeIndex];
    if (suggestion !== undefined) selectSuggestion(suggestion);
  });

  const attachmentInputDisabled =
    actionBusy || staging || onStageAttachment === undefined;
  const canSend =
    !(live && askPending) &&
    !actionBusy &&
    !staging &&
    !hasStaleMentions &&
    (text.trim().length > 0 || mentionCount > 0 || attachments.length > 0);
  const imagePlacementAvailable =
    !live && !actionBusy && attachments.some((attachment) => attachment.kind === "image");
  const imagePlacementOn = imagePlacement && imagePlacementAvailable;

  async function stageFiles(source: FileList | readonly File[]) {
    if (
      onStageAttachment === undefined ||
      stagingRef.current ||
      actionBusy
    ) {
      return;
    }
    const available = MAX_ATTACHMENTS_PER_TURN - attachments.length;
    const files = Array.from(source).slice(0, Math.max(available, 0));
    if (files.length === 0) {
      setAttachmentError(
        t("map.prompt.maxAttachments", { max: MAX_ATTACHMENTS_PER_TURN }),
      );
      return;
    }
    const omitted = Array.from(source).length - files.length;
    setAttachmentError(
      omitted > 0
        ? t("map.prompt.attachedOnlyMax", { max: MAX_ATTACHMENTS_PER_TURN })
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
            t("map.prompt.textLimit"),
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
    if (file.size > MAX_IMAGE_BYTES) {
      setAttachmentError(t("map.prompt.fileTooLarge", { path: file.path }));
      return;
    }
    try {
      await stageFiles([await onReadProjectFile(file)]);
    } catch (error) {
      setAttachmentError(
        t("map.prompt.attachFailed", { path: file.path, reason: attachmentErrorMessage(error) }),
      );
    }
  }

  async function removeAttachment(attachment: ChatAttachment) {
    const remaining = attachments.filter((candidate) => candidate.id !== attachment.id);
    setAttachments((current) =>
      current.filter((candidate) => candidate.id !== attachment.id),
    );
    if (!remaining.some((candidate) => candidate.kind === "image")) {
      setImagePlacement(false);
    }
    try {
      await onDiscardAttachment?.(attachment.id);
    } catch (error) {
      setAttachmentError(attachmentErrorMessage(error));
    }
  }

  function handleSend() {
    if (!canSend || stagingRef.current) return;
    onSend(text, attachments, imagePlacementOn);
    setText("");
    setFragment(null);
    setAttachments([]);
    setImagePlacement(false);
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
          {t("map.prompt.dropToAttach")}
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
                    aria-label={t("map.prompt.removeAttachment", { name: attachment.name })}
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
            aria-label={t("map.prompt.inputAria")}
            value={text}
            disabled={actionBusy}
            placeholder={
              live
                ? t("map.prompt.steerPlaceholder")
                : (placeholder ?? t("map.prompt.placeholder"))
            }
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
              if (!listboxOpen) return;
              // WebKit ends the composition before the keydown of the key that
              // committed it, so keyCode 229 still marks an IME keystroke.
              const imeKey =
                composingRef.current ||
                event.nativeEvent.isComposing ||
                event.nativeEvent.keyCode === 229;
              if (
                (event.key === "Tab" || event.key === "Enter") &&
                !event.shiftKey
              ) {
                event.preventDefault();
                if (imeKey || !filesCurrent) {
                  pendingCompletionRef.current = true;
                } else if (activeSuggestion !== undefined) {
                  selectSuggestion(activeSuggestion);
                }
                return;
              }
              if (imeKey) return;
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
              } else if (event.key === "Escape") {
                event.preventDefault();
                pendingCompletionRef.current = false;
                dismissedFragmentRef.current = fragment?.key ?? null;
                setFragment(null);
              }
            }}
            onCompositionStart={() => {
              composingRef.current = true;
              setComposing(true);
            }}
            onCompositionEnd={(event) => {
              composingRef.current = false;
              setComposing(false);
              updateFragment(event.currentTarget.value, event.currentTarget.selectionStart);
            }}
            onPaste={handlePaste}
          />
        </PromptInputBody>
        {listboxOpen && (
          <div
            id={MENTION_LISTBOX_ID}
            role="listbox"
            aria-label={t("map.prompt.mentionResults")}
            className="mx-2 mb-1 max-h-56 w-[calc(100%-1rem)] overflow-y-auto rounded-md border border-border bg-popover p-1 text-popover-foreground shadow-lg"
          >
            {suggestions.length === 0 ? (
              <p role="status" className="px-2 py-2 text-xs text-muted-foreground">
                {!filesCurrent
                  ? t("map.prompt.searching")
                  : selections.length === 0 && locations.length === 0 && fileSearch === undefined
                    ? t("map.prompt.nothingToMention")
                    : fileSearch === undefined
                      ? t("map.prompt.noMatchAreaLocation")
                      : t("map.prompt.noMatchAll")}
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
                    ) : suggestion.kind === "location" ? (
                      <FocusIcon aria-hidden className="size-3.5" />
                    ) : (
                      <FileTextIcon aria-hidden className="size-3.5" />
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
                  aria-label={t("map.prompt.attachFile")}
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
                  aria-label={t("map.prompt.attach")}
                  disabled={attachmentInputDisabled}
                  onClick={() => fileInput.current?.click()}
                >
                  {staging ? (
                    <LoaderCircleIcon className="size-4 animate-spin motion-reduce:animate-none" />
                  ) : (
                    <PaperclipIcon className="size-4" />
                  )}
                  {t("map.prompt.attach")}
                </PromptInputButton>
              </>
            )}
            <PromptInputButton
              type="button"
              variant="ghost"
              className={
                imagePlacementOn
                  ? "bg-primary/15 text-primary hover:bg-primary/25 hover:text-primary"
                  : undefined
              }
              aria-pressed={imagePlacementOn}
              aria-label={t("map.prompt.imagePlacement")}
              disabled={!imagePlacementAvailable}
              title={
                imagePlacementAvailable
                  ? t("map.prompt.imagePlacementOn")
                  : t("map.prompt.imagePlacementOff")
              }
              onClick={() => setImagePlacement((value) => !value)}
            >
              <ImagePlusIcon className="size-4" />
              {t("map.prompt.imagePlacement")}
            </PromptInputButton>
            <ProviderPromptControls
              settings={modelSettings}
              busy={modelSettingsBusy}
              disabled={live || actionBusy}
              contextUsage={contextUsage}
              onChange={onModelSettingsChange}
              onReload={onModelSettingsReload}
            />
          </PromptInputTools>
          <PromptInputSubmit
            className="ml-auto"
            aria-label={live ? t("map.prompt.sendToAi") : t("map.prompt.send")}
            disabled={!canSend}
          >
            {live ? (
              <CornerDownRightIcon className="size-4" aria-hidden="true" />
            ) : (
              <SendIcon className="size-4" aria-hidden="true" />
            )}
            {live ? t("map.prompt.deliver") : t("map.prompt.send")}
          </PromptInputSubmit>
        </PromptInputFooter>
      </PromptInput>
      {hasStaleMentions && (
        <p role="alert" className="mt-1.5 text-xs text-destructive">
          {t("map.prompt.staleMentions")}
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
