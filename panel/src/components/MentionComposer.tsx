import {
  useEffect,
  useRef,
  useState,
  type ClipboardEventHandler,
  type RefObject,
} from "react";
import { FileTextIcon, MapPinnedIcon, ScanLineIcon, XIcon } from "lucide-react";

import {
  PromptInputBody,
  PromptInputTextarea,
} from "@/components/ai-elements/prompt-input";
import type {
  MentionInstance,
  MentionSearchRequest,
  MentionSearchResponse,
  MentionSnapshot,
  MentionSuggestion,
} from "@/lib/ipc";
import { formatAttachmentSize } from "@/lib/attachments";
import type { ProjectFileSuggestion } from "@/lib/projectFiles";
import { cn } from "@/lib/utils";

export const MAX_MENTIONS_PER_TURN = 16;
const SEARCH_LIMIT = 20;
const SEARCH_DEBOUNCE_MS = 120;
const LISTBOX_ID = "main-resource-mention-listbox";

interface ActiveFragment {
  start: number;
  end: number;
  query: string;
  key: string;
}

export interface MentionComposerProps {
  text: string;
  onTextChange(text: string): void;
  mentions: MentionInstance[];
  onMentionsChange(mentions: MentionInstance[]): void;
  search?(request: MentionSearchRequest): Promise<MentionSearchResponse>;
  /** Project-root files offered after the map resources; picking one attaches it. */
  fileSearch?(query: string): Promise<ProjectFileSuggestion[]>;
  onAttachFile?(file: ProjectFileSuggestion): void;
  projectIdentity: string;
  scopeIdentity: string;
  disabled?: boolean;
  placeholder?: string;
  textareaRef: RefObject<HTMLTextAreaElement | null>;
  onPaste?: ClipboardEventHandler<HTMLTextAreaElement>;
}

interface MentionChipsProps {
  mentions: readonly MentionInstance[];
  onRemove?(id: string): void;
  onChipRef?(id: string, node: HTMLDivElement | null): void;
  align?: "start" | "end";
}

let mentionSequence = 0;


function mentionIcon(kind: MentionSnapshot["kind"]) {
  switch (kind) {
    case "map.region":
      return <ScanLineIcon aria-hidden className="size-3.5" />;
    case "map.location":
      return <MapPinnedIcon aria-hidden className="size-3.5" />;
  }
}

export function MentionChips({
  mentions,
  onRemove,
  onChipRef,
  align = "start",
}: MentionChipsProps) {
  if (mentions.length === 0) return null;
  return (
    <div
      data-testid="mention-chips"
      className={cn(
        "flex max-w-full flex-wrap gap-1.5",
        align === "end" && "justify-end",
      )}
    >
      {mentions.map((instance) => (
        <div
          key={instance.id}
          ref={(node) => onChipRef?.(instance.id, node)}
          tabIndex={-1}
          data-mention-kind={instance.mention.kind}
          className={cn(
            "flex min-w-0 max-w-72 items-center gap-1 rounded-md border border-emerald-500/35 bg-emerald-500/10 px-2 py-1 text-xs text-emerald-100 focus-visible:outline-2 focus-visible:outline-ring",
            instance.stale &&
              "border-destructive/50 bg-destructive/10 text-destructive",
          )}
          title={instance.detail}
        >
          {mentionIcon(instance.mention.kind)}
          <span className="truncate font-medium">@{instance.label}</span>
          {instance.detail && (
            <span className="hidden truncate text-muted-foreground sm:inline">
              {instance.detail}
            </span>
          )}
          {instance.stale && <span className="shrink-0">만료됨</span>}
          {onRemove && (
            <button
              type="button"
              aria-label={`@${instance.label} 멘션 제거`}
              className="ml-0.5 flex size-6 shrink-0 cursor-pointer items-center justify-center rounded text-current/70 hover:bg-background/70 hover:text-current focus-visible:outline-2 focus-visible:outline-ring"
              onClick={() => onRemove(instance.id)}
            >
              <XIcon aria-hidden className="size-3" />
            </button>
          )}
        </div>
      ))}
    </div>
  );
}

type ComposerOption =
  | { type: "mention"; key: string; suggestion: MentionSuggestion }
  | { type: "file"; key: string; file: ProjectFileSuggestion };

function fileDirectory(path: string): string {
  const slash = path.lastIndexOf("/");
  return slash < 0 ? "프로젝트 루트" : path.slice(0, slash);
}

function fileLabel(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

export function MentionComposer({
  text,
  onTextChange,
  mentions,
  onMentionsChange,
  search,
  fileSearch,
  onAttachFile,
  projectIdentity,
  scopeIdentity,
  disabled = false,
  placeholder,
  textareaRef,
  onPaste,
}: MentionComposerProps) {
  const [fragment, setFragment] = useState<ActiveFragment | null>(null);
  const [results, setResults] = useState<ComposerOption[]>([]);
  // Fragment key the current results answer; a completion never uses the
  // results of an older query.
  const [resultsKey, setResultsKey] = useState<string | null>(null);
  const [activeIndex, setActiveIndex] = useState(0);
  const [loading, setLoading] = useState(false);
  const [composing, setComposing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const composingRef = useRef(false);
  // Tab/Enter pressed while the IME was composing or the search was loading:
  // complete once the committed query has its results.
  const pendingCompletionRef = useRef(false);
  const dismissedFragmentRef = useRef<string | null>(null);
  const searchSequenceRef = useRef(0);
  const chipRefs = useRef(new Map<string, HTMLDivElement>());
  const identityRef = useRef({ projectIdentity, scopeIdentity });
  const searchable = search !== undefined || fileSearch !== undefined;

  useEffect(() => {
    const previous = identityRef.current;
    identityRef.current = { projectIdentity, scopeIdentity };
    if (previous.scopeIdentity !== scopeIdentity) {
      if (mentions.length > 0) onMentionsChange([]);
      setFragment(null);
      setResults([]);
      setError(null);
      return;
    }
    if (
      previous.projectIdentity !== projectIdentity &&
      mentions.length > 0
    ) {
      onMentionsChange(
        mentions.map((instance) => ({ ...instance, stale: true })),
      );
      setFragment(null);
      setResults([]);
      setError("프로젝트가 변경되어 선택한 멘션이 만료되었습니다. 제거한 뒤 다시 검색해 주세요.");
    }
  }, [mentions, onMentionsChange, projectIdentity, scopeIdentity]);

  // The query follows the textarea even while an IME composes (a Korean
  // query is composing for as long as its last syllable is being typed).
  useEffect(() => {
    if (fragment === null || disabled || !searchable) {
      searchSequenceRef.current += 1;
      setLoading(false);
      if (fragment === null) {
        setResults([]);
        setResultsKey(null);
        pendingCompletionRef.current = false;
      }
      return;
    }
    const sequence = ++searchSequenceRef.current;
    setLoading(true);
    setError(null);
    setActiveIndex(0);
    const timeout = window.setTimeout(() => {
      void Promise.allSettled([
        search?.({ query: fragment.query, limit: SEARCH_LIMIT }) ??
          Promise.resolve(null),
        fileSearch?.(fragment.query) ?? Promise.resolve(null),
      ]).then(([mentionResult, fileResult]) => {
        if (searchSequenceRef.current !== sequence) return;
        const next: ComposerOption[] = [];
        if (mentionResult.status === "fulfilled" && mentionResult.value !== null) {
          for (const suggestion of mentionResult.value.results) {
            next.push({ type: "mention", key: suggestion.resourceKey, suggestion });
          }
        }
        if (fileResult.status === "fulfilled" && fileResult.value !== null) {
          for (const file of fileResult.value) {
            next.push({ type: "file", key: `project.file:${file.path}`, file });
          }
        }
        const failure = [mentionResult, fileResult].find(
          (result) => result.status === "rejected",
        );
        setResults(next);
        setResultsKey(fragment.key);
        setLoading(false);
        setError(
          next.length === 0 && failure?.status === "rejected"
            ? `멘션 검색에 실패했습니다: ${String(failure.reason)}`
            : null,
        );
      });
    }, SEARCH_DEBOUNCE_MS);
    return () => window.clearTimeout(timeout);
  }, [disabled, fragment, search, fileSearch, searchable]);

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

  function removeFragment(active: ActiveFragment) {
    onTextChange(text.slice(0, active.start) + text.slice(active.end));
    setFragment(null);
    setResults([]);
    setResultsKey(null);
    const caret = active.start;
    requestAnimationFrame(() => {
      const textarea = textareaRef.current;
      textarea?.focus();
      textarea?.setSelectionRange(caret, caret);
    });
  }

  function selectOption(option: ComposerOption) {
    if (fragment === null) return;
    if (option.type === "file") {
      setError(null);
      removeFragment(fragment);
      onAttachFile?.(option.file);
      return;
    }
    const suggestion = option.suggestion;
    const encodedSuggestion = JSON.stringify(suggestion.mention);
    const existing = mentions.find(
      (instance) => JSON.stringify(instance.mention) === encodedSuggestion,
    );
    if (existing !== undefined) {
      setError(`@${suggestion.label} 멘션은 이미 선택되어 있습니다.`);
      setFragment(null);
      setResults([]);
      requestAnimationFrame(() => chipRefs.current.get(existing.id)?.focus());
      return;
    }
    if (mentions.length >= MAX_MENTIONS_PER_TURN) {
      setError(`한 메시지에는 멘션을 최대 ${MAX_MENTIONS_PER_TURN}개까지 사용할 수 있습니다.`);
      return;
    }

    mentionSequence += 1;
    const nextInstance: MentionInstance = {
      id: `mention-${Date.now().toString(36)}-${mentionSequence.toString(36)}`,
      label: suggestion.label,
      ...(suggestion.detail ? { detail: suggestion.detail } : {}),
      mention: suggestion.mention,
    };
    onMentionsChange([...mentions, nextInstance]);
    setError(null);
    removeFragment(fragment);
  }

  const resultsCurrent = fragment !== null && resultsKey === fragment.key;

  useEffect(() => {
    if (!pendingCompletionRef.current || composing || loading) return;
    if (fragment === null) {
      pendingCompletionRef.current = false;
      return;
    }
    if (!resultsCurrent) return;
    pendingCompletionRef.current = false;
    const option = results[activeIndex];
    if (option !== undefined) selectOption(option);
  });

  const listboxOpen = fragment !== null && searchable && !disabled;
  const activeOption = results[activeIndex];

  return (
    <>
      {mentions.length > 0 && (
        <div
          data-align="block-start"
          className="w-full border-b border-border/70 px-3 py-2"
        >
          <MentionChips
            mentions={mentions}
            onRemove={(id) =>
              onMentionsChange(
                mentions.filter((instance) => instance.id !== id),
              )
            }
            onChipRef={(id, node) => {
              if (node === null) chipRefs.current.delete(id);
              else chipRefs.current.set(id, node);
            }}
          />
        </div>
      )}
      <PromptInputBody>
        <PromptInputTextarea
          ref={textareaRef}
          role="combobox"
          aria-autocomplete="list"
          aria-expanded={listboxOpen}
          aria-controls={listboxOpen ? LISTBOX_ID : undefined}
          aria-activedescendant={
            listboxOpen && activeOption
              ? `${LISTBOX_ID}-option-${activeIndex}`
              : undefined
          }
          aria-label="지시 입력"
          value={text}
          onChange={(event) => {
            onTextChange(event.target.value);
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
              updateFragment(
                event.currentTarget.value,
                event.currentTarget.selectionStart,
              );
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
            if ((event.key === "Tab" && !event.shiftKey) || event.key === "Enter") {
              if (event.key === "Enter" && event.shiftKey) return;
              event.preventDefault();
              if (imeKey || loading || !resultsCurrent) {
                pendingCompletionRef.current = true;
              } else if (activeOption !== undefined) {
                selectOption(activeOption);
              }
              return;
            }
            if (imeKey) return;
            if (event.key === "ArrowDown") {
              event.preventDefault();
              setActiveIndex((index) =>
                results.length === 0 ? 0 : (index + 1) % results.length,
              );
            } else if (event.key === "ArrowUp") {
              event.preventDefault();
              setActiveIndex((index) =>
                results.length === 0
                  ? 0
                  : (index - 1 + results.length) % results.length,
              );
            } else if (event.key === "Escape") {
              event.preventDefault();
              pendingCompletionRef.current = false;
              dismissedFragmentRef.current = fragment?.key ?? null;
              setFragment(null);
              setResults([]);
            }
          }}
          onCompositionStart={() => {
            composingRef.current = true;
            setComposing(true);
          }}
          onCompositionEnd={(event) => {
            composingRef.current = false;
            setComposing(false);
            updateFragment(
              event.currentTarget.value,
              event.currentTarget.selectionStart,
            );
          }}
          onPaste={onPaste}
          placeholder={placeholder}
          disabled={disabled}
        />
      </PromptInputBody>
      {listboxOpen && (
        <div
          id={LISTBOX_ID}
          role="listbox"
          aria-label="리소스 멘션 검색 결과"
          className="mx-2 mb-1 max-h-56 w-[calc(100%-1rem)] overflow-y-auto rounded-md border border-border bg-popover p-1 text-popover-foreground shadow-lg"
        >
          {loading && results.length === 0 ? (
            <p role="status" className="px-2 py-2 text-xs text-muted-foreground">
              멘션 검색 중…
            </p>
          ) : error !== null ? (
            <p role="alert" className="px-2 py-2 text-xs text-destructive">
              {error}
            </p>
          ) : results.length === 0 ? (
            <p role="status" className="px-2 py-2 text-xs text-muted-foreground">
              검색 결과가 없습니다.
            </p>
          ) : (
            results.map((option, index) => {
              const label =
                option.type === "mention"
                  ? option.suggestion.label
                  : fileLabel(option.file.path);
              const detail =
                option.type === "mention"
                  ? option.suggestion.detail
                  : `${fileDirectory(option.file.path)} · ${formatAttachmentSize(option.file.size)} · 첨부`;
              return (
                <div
                  id={`${LISTBOX_ID}-option-${index}`}
                  key={option.key}
                  role="option"
                  aria-selected={index === activeIndex}
                  data-mention-kind={
                    option.type === "mention" ? option.suggestion.kind : "project.file"
                  }
                  className={cn(
                    "flex cursor-pointer items-start gap-2 rounded px-2 py-2 text-sm",
                    index === activeIndex ? "bg-accent" : "hover:bg-accent/60",
                  )}
                  onMouseDown={(event) => event.preventDefault()}
                  onMouseEnter={() => setActiveIndex(index)}
                  onClick={() => selectOption(option)}
                >
                  <span className="mt-0.5 shrink-0 text-emerald-400">
                    {option.type === "mention" ? (
                      mentionIcon(option.suggestion.kind)
                    ) : (
                      <FileTextIcon aria-hidden className="size-3.5" />
                    )}
                  </span>
                  <span className="min-w-0">
                    <span className="block truncate font-medium">@{label}</span>
                    {detail && (
                      <span className="block truncate text-xs text-muted-foreground">
                        {detail}
                      </span>
                    )}
                  </span>
                </div>
              );
            })
          )}
        </div>
      )}
      {error !== null && !listboxOpen && (
        <p role="alert" className="w-full px-3 pb-1 text-xs text-destructive">
          {error}
        </p>
      )}
    </>
  );
}

export function activeMentionFragment(
  value: string,
  caret: number,
): ActiveFragment | null {
  if (caret < 0 || caret > value.length) return null;
  const lineStart = value.lastIndexOf("\n", Math.max(caret - 1, 0)) + 1;
  for (let index = caret - 1; index >= lineStart; index -= 1) {
    if (value[index] !== "@") continue;
    const previous = index === 0 ? "" : value[index - 1];
    if (previous !== "" && !/[\s(\[{"'“‘,;:!?]/u.test(previous)) continue;
    const query = value.slice(index + 1, caret);
    if (query.includes("@")) return null;
    return {
      start: index,
      end: caret,
      query,
      key: `${index}:${caret}:${query}`,
    };
  }
  return null;
}
