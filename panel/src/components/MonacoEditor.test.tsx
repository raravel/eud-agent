import { render, waitFor } from "@testing-library/react";
import { useEffect } from "react";
import { describe, expect, it, vi } from "vitest";

import MonacoEditor from "./MonacoEditor";

// The real editor needs a browser canvas and workers; the double hands the
// component a fake editor instance on mount, as `@monaco-editor/react` does.
const fakeEditor = {
  getModel: () => ({
    getLineCount: () => 40,
    getLineMaxColumn: (line: number) => line + 10,
  }),
  layout: vi.fn(),
  revealLineInCenter: vi.fn(),
  setSelection: vi.fn(),
};

vi.mock("@/editor/monaco", () => ({}));
vi.mock("@/editor/monacoTheme", () => ({ EUD_AGENT_THEME: "eud-agent" }));
vi.mock("@monaco-editor/react", () => ({
  default: function Editor({ onMount }: { onMount?: (editor: unknown) => void }) {
    useEffect(() => {
      onMount?.(fakeEditor);
    }, [onMount]);
    return null;
  },
}));

describe("MonacoEditor", () => {
  it("reveals and selects the target line, again for a new target on the same line", async () => {
    const view = render(<MonacoEditor value="x" readOnly revealLine={{ line: 12 }} />);
    // Deferred a frame, after laying out a tab that was just hidden.
    expect(fakeEditor.revealLineInCenter).not.toHaveBeenCalled();
    await waitFor(() => expect(fakeEditor.revealLineInCenter).toHaveBeenLastCalledWith(12));
    expect(fakeEditor.layout).toHaveBeenCalled();
    expect(fakeEditor.setSelection).toHaveBeenLastCalledWith({
      startLineNumber: 12,
      startColumn: 1,
      endLineNumber: 12,
      endColumn: 22,
    });

    view.rerender(<MonacoEditor value="x" readOnly revealLine={{ line: 12 }} />);
    await waitFor(() => expect(fakeEditor.revealLineInCenter).toHaveBeenCalledTimes(2));

    // A line past the end of the file reveals the last line.
    view.rerender(<MonacoEditor value="x" readOnly revealLine={{ line: 99 }} />);
    await waitFor(() => expect(fakeEditor.revealLineInCenter).toHaveBeenLastCalledWith(40));
  });

  it("leaves the view alone without a target", async () => {
    fakeEditor.revealLineInCenter.mockClear();
    render(<MonacoEditor value="x" readOnly />);
    await new Promise((resolve) => requestAnimationFrame(resolve));
    expect(fakeEditor.revealLineInCenter).not.toHaveBeenCalled();
  });
});
