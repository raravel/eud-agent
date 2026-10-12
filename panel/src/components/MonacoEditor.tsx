/**
 * Monaco edit surface, isolated so it can be lazy-loaded.
 *
 * Importing `monaco-editor` (and the `@/editor/monaco` local-bundle wiring with
 * its `?worker` imports) is multi-MB. Keeping it behind `React.lazy` (see
 * ReviewTabs) puts ALL of that in a separate async chunk that loads only when
 * the edit tab is first opened — the eager entry stays small (dep-pruning
 * carry-forward: "Monaco stays lazy/worker-split").
 *
 * The Monaco buffer is the SINGLE SOURCE OF TRUTH for Apply: `value` is the
 * edit buffer and every change flows out via `onChange`. A `readOnly` surface
 * (the project file viewer) only highlights; it never emits changes.
 *
 * The editor always uses the panel-derived `eud-agent` theme registered by
 * `@/editor/monaco`, so Monaco surfaces share the page palette.
 */
import { useEffect, useState } from "react";
import Editor, { type OnMount } from "@monaco-editor/react";
// Side-effect: bind Monaco to the local npm bundle (no CDN loader) and register
// the panel theme. Lives in THIS lazy module so the monaco bundle + workers are
// async-split with it.
import "@/editor/monaco";
import { EUD_AGENT_THEME } from "@/editor/monacoTheme";
import type { FileLineTarget } from "@/lib/projectFiles";

export interface MonacoEditorProps {
  /** Current edit buffer (Apply source of truth). */
  value: string;
  /** Required unless `readOnly`; a read-only surface never calls it. */
  onChange?(next: string): void;
  /** Monaco language id. Defaults to plaintext for existing callers. */
  language?: string;
  /** CSS height of the editor surface. */
  height?: string | number;
  /** Highlight-only viewer: no edits, no hover/suggest from the language service. */
  readOnly?: boolean;
  /** Accessible name for the editor's text area. */
  ariaLabel?: string;
  /**
   * Line to reveal and select. A new object reveals again even for the same
   * line, so reopening a search match scrolls back to it.
   */
  revealLine?: FileLineTarget | null;
}

type MonacoInstance = Parameters<OnMount>[0];

export default function MonacoEditor({
  value,
  onChange,
  language = "plaintext",
  height = "288px",
  readOnly = false,
  ariaLabel,
  revealLine,
}: MonacoEditorProps) {
  const [editor, setEditor] = useState<MonacoInstance | null>(null);

  // The tab that asked for the reveal may have been hidden (0×0) until this
  // render; lay the editor out on the next frame before centring the line.
  useEffect(() => {
    if (!editor || !revealLine) return;
    const frame = requestAnimationFrame(() => {
      const model = editor.getModel();
      if (!model) return;
      const line = Math.min(Math.max(1, revealLine.line), model.getLineCount());
      editor.layout();
      editor.revealLineInCenter(line);
      editor.setSelection({
        startLineNumber: line,
        startColumn: 1,
        endLineNumber: line,
        endColumn: model.getLineMaxColumn(line),
      });
    });
    return () => cancelAnimationFrame(frame);
  }, [editor, revealLine]);

  return (
    <Editor
      height={height}
      defaultLanguage={language}
      value={value}
      onMount={setEditor}
      onChange={(v) => onChange?.(v ?? "")}
      theme={EUD_AGENT_THEME}
      options={{
        minimap: { enabled: false },
        automaticLayout: true,
        fontSize: 13,
        wordWrap: "on",
        scrollBeyondLastLine: false,
        readOnly,
        domReadOnly: readOnly,
        ariaLabel,
        ...(readOnly
          ? {
              hover: { enabled: false },
              quickSuggestions: false,
              readOnlyMessage: {
                value: "읽기 전용 파일입니다. 변경은 에이전트에게 요청하세요.",
              },
            }
          : {}),
      }}
    />
  );
}
