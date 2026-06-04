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
 * edit buffer and every change flows out via `onChange`.
 */
import Editor from "@monaco-editor/react";
// Side-effect: bind Monaco to the local npm bundle (no CDN loader). Lives in
// THIS lazy module so the monaco bundle + workers are async-split with it.
import "@/editor/monaco";

export interface MonacoEditorProps {
  /** Current edit buffer (Apply source of truth). */
  value: string;
  onChange(next: string): void;
}

export default function MonacoEditor({ value, onChange }: MonacoEditorProps) {
  return (
    <Editor
      height="288px"
      defaultLanguage="plaintext"
      value={value}
      onChange={(v) => onChange(v ?? "")}
      theme="vs-dark"
      options={{
        minimap: { enabled: false },
        fontSize: 13,
        wordWrap: "on",
        scrollBeyondLastLine: false,
      }}
    />
  );
}
