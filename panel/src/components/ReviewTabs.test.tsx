/**
 * Review area Tabs (features/03 ## Behaviors → Review):
 *   - preview tab: CodeBlock (escaped, lang label; display truncated at 1 MiB
 *     with a notice — Apply always sends full text).
 *   - diff tab: server unified diff with +/- coloring; HIDDEN in new-file mode.
 *   - edit tab: Monaco seeded from the code; the Monaco buffer is the SINGLE
 *     SOURCE OF TRUTH for Apply.
 *
 * Monaco is module-mocked with a <textarea> test double (the task's
 * "textarea test double"): the real editor needs a worker/DOM canvas happy-dom
 * can't supply, and the contract under test is only that edits flow to Apply.
 *
 * Contract (Step B implements `@/components/ReviewTabs`):
 *   export interface ReviewTabsProps {
 *     review: ReviewState;          // { code, lang, diff, diagnostics }
 *     newFileMode: boolean;
 *     editedCode: string;           // current Monaco buffer (Apply source of truth)
 *     onEditCode(next: string): void;
 *   }
 *   export function ReviewTabs(props: ReviewTabsProps): JSX.Element;
 *
 * Tabs are the shadcn Tabs primitive with values "preview" / "diff" / "edit",
 * trigger labels "미리보기" / "변경" / "편집"; preview default.
 */
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReviewState } from "@/state/store";
// `vi.mock` calls below are hoisted above every import by Vitest, so the
// @monaco-editor/react + @/editor/monaco doubles are in place before ReviewTabs
// (and its transitive Monaco imports) load.
import { ReviewTabs as ReviewTabsHarness } from "@/components/ReviewTabs";

// --- Monaco test double: a controlled textarea standing in for the editor. ---
vi.mock("@monaco-editor/react", () => ({
  __esModule: true,
  default: ({
    value,
    onChange,
  }: {
    value?: string;
    onChange?: (v: string | undefined) => void;
  }) => (
    <textarea
      aria-label="monaco"
      value={value ?? ""}
      onChange={(e) => onChange?.(e.target.value)}
    />
  ),
  // Some wrappers import a named { Editor }; provide both to be safe.
  Editor: ({
    value,
    onChange,
  }: {
    value?: string;
    onChange?: (v: string | undefined) => void;
  }) => (
    <textarea
      aria-label="monaco"
      value={value ?? ""}
      onChange={(e) => onChange?.(e.target.value)}
    />
  ),
  loader: { config: vi.fn() },
}));

// The Monaco wiring module has side effects (worker imports) that happy-dom
// can't run; stub it so importing ReviewTabs (which imports it) is harmless.
vi.mock("@/editor/monaco", () => ({ monaco: {} }));

function review(over: Partial<ReviewState> = {}): ReviewState {
  return {
    code: "function f() {}",
    lang: "eps",
    diff: "@@ -1 +1 @@\n-old\n+new",
    diagnostics: [],
    ...over,
  };
}

const noop = () => {};

describe("ReviewTabs — preview", () => {
  it("shows the language label", () => {
    render(
      <ReviewTabsHarness
        review={review({ lang: "eps" })}
        newFileMode={false}
        editedCode={review().code}
        onEditCode={noop}
      />,
    );
    expect(screen.getByText("eps")).toBeInTheDocument();
  });

  it("renders the code as text (escaped — no raw HTML injection)", () => {
    const malicious = "<img src=x onerror=alert(1)>";
    render(
      <ReviewTabsHarness
        review={review({ code: malicious })}
        newFileMode={false}
        editedCode={malicious}
        onEditCode={noop}
      />,
    );
    // The text content must contain the literal markup; no <img> element exists.
    expect(screen.getByTestId("preview-code")).toHaveTextContent(malicious);
    expect(
      document.querySelector("[data-testid='preview-code'] img"),
    ).toBeNull();
  });

  it("shows a truncation notice when code exceeds 1 MiB and slices the display", () => {
    const big = "a".repeat(1024 * 1024 + 10);
    render(
      <ReviewTabsHarness
        review={review({ code: big })}
        newFileMode={false}
        editedCode={big}
        onEditCode={noop}
      />,
    );
    expect(screen.getByTestId("preview-truncated-notice")).toBeInTheDocument();
    const previewText = screen.getByTestId("preview-code").textContent ?? "";
    expect(previewText.length).toBeLessThanOrEqual(1024 * 1024);
  });

  it("does NOT show the truncation notice for small code", () => {
    render(
      <ReviewTabsHarness
        review={review({ code: "small" })}
        newFileMode={false}
        editedCode="small"
        onEditCode={noop}
      />,
    );
    expect(
      screen.queryByTestId("preview-truncated-notice"),
    ).not.toBeInTheDocument();
  });
});

describe("ReviewTabs — diff tab", () => {
  it("is available (renders the diff trigger) in SET mode", () => {
    render(
      <ReviewTabsHarness
        review={review()}
        newFileMode={false}
        editedCode={review().code}
        onEditCode={noop}
      />,
    );
    expect(screen.getByRole("tab", { name: "변경" })).toBeInTheDocument();
  });

  it("is HIDDEN in new-file mode", () => {
    render(
      <ReviewTabsHarness
        review={review()}
        newFileMode={true}
        editedCode={review().code}
        onEditCode={noop}
      />,
    );
    expect(screen.queryByRole("tab", { name: "변경" })).not.toBeInTheDocument();
  });

  it("colors +/- lines when the diff tab is active", async () => {
    const user = userEvent.setup();
    render(
      <ReviewTabsHarness
        review={review({ diff: "@@ -1 +1 @@\n-removed\n+added" })}
        newFileMode={false}
        editedCode={review().code}
        onEditCode={noop}
      />,
    );
    await user.click(screen.getByRole("tab", { name: "변경" }));
    expect(screen.getByTestId("diff-line-add")).toHaveTextContent("+added");
    expect(screen.getByTestId("diff-line-del")).toHaveTextContent("-removed");
  });
});

describe("ReviewTabs — tab switching", () => {
  it("shows the preview panel by default and the edit panel after switching", async () => {
    const user = userEvent.setup();
    render(
      <ReviewTabsHarness
        review={review()}
        newFileMode={false}
        editedCode={review().code}
        onEditCode={noop}
      />,
    );
    // Preview is the default active panel.
    expect(screen.getByTestId("preview-code")).toBeVisible();
    // Switch to edit → the Monaco double appears (lazy-loaded → findBy awaits it).
    await user.click(screen.getByRole("tab", { name: "편집" }));
    expect(await screen.findByLabelText("monaco")).toBeInTheDocument();
  });
});

describe("ReviewTabs — Monaco buffer is the apply source of truth", () => {
  it("routes edits through onEditCode (the buffer Apply will send)", async () => {
    const user = userEvent.setup();
    const onEditCode = vi.fn();
    render(
      <ReviewTabsHarness
        review={review({ code: "seed" })}
        newFileMode={false}
        editedCode="seed"
        onEditCode={onEditCode}
      />,
    );
    await user.click(screen.getByRole("tab", { name: "편집" }));
    const editor = await screen.findByLabelText("monaco");
    await user.type(editor, "!");
    expect(onEditCode).toHaveBeenCalled();
  });

  it("seeds the editor with the editedCode buffer", async () => {
    const user = userEvent.setup();
    render(
      <ReviewTabsHarness
        review={review({ code: "original" })}
        newFileMode={false}
        editedCode="edited-buffer"
        onEditCode={noop}
      />,
    );
    await user.click(screen.getByRole("tab", { name: "편집" }));
    expect(await screen.findByLabelText("monaco")).toHaveValue("edited-buffer");
  });
});
