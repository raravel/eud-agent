/**
 * Advisory diagnostics strip (features/03 ## Behaviors → Diagnostics):
 *   "advisory strip below the review area; dismissible; NEVER blocks Apply."
 * rules.md: epscript-lsp diagnostics are advisory only — they annotate, never
 * block apply; absence must not break the flow.
 *
 * Contract (Step B implements `@/components/DiagnosticsStrip`):
 *   export interface DiagnosticsStripProps {
 *     diagnostics: Diagnostic[];   // string | { message?/text?/severity?/line? }
 *     dismissed: boolean;
 *     onDismiss(): void;
 *   }
 *   export function DiagnosticsStrip(props): JSX.Element | null;
 *
 * Renders nothing when there are no diagnostics OR when dismissed. The dismiss
 * control has accessible name "닫기". The component owns ZERO Apply gating —
 * that lives in the ApplyBar (separate test) — so "never blocks Apply" is a
 * structural guarantee, asserted there.
 */
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { Diagnostic } from "@/lib/ipc";
import { DiagnosticsStrip } from "@/components/DiagnosticsStrip";

describe("DiagnosticsStrip", () => {
  it("renders nothing when there are no diagnostics", () => {
    const { container } = render(
      <DiagnosticsStrip diagnostics={[]} dismissed={false} onDismiss={() => {}} />,
    );
    expect(container).toBeEmptyDOMElement();
  });

  it("renders a string diagnostic's text", () => {
    render(
      <DiagnosticsStrip
        diagnostics={["unused variable foo"]}
        dismissed={false}
        onDismiss={() => {}}
      />,
    );
    expect(screen.getByText(/unused variable foo/)).toBeInTheDocument();
  });

  it("renders a structured diagnostic's message field", () => {
    const diags: Diagnostic[] = [{ message: "type mismatch", severity: "warning" }];
    render(
      <DiagnosticsStrip diagnostics={diags} dismissed={false} onDismiss={() => {}} />,
    );
    expect(screen.getByText(/type mismatch/)).toBeInTheDocument();
  });

  it("renders nothing when dismissed even with diagnostics present", () => {
    const { container } = render(
      <DiagnosticsStrip
        diagnostics={["something"]}
        dismissed={true}
        onDismiss={() => {}}
      />,
    );
    expect(container).toBeEmptyDOMElement();
  });

  it("invokes onDismiss when the close control is clicked", async () => {
    const user = userEvent.setup();
    const onDismiss = vi.fn();
    render(
      <DiagnosticsStrip
        diagnostics={["something"]}
        dismissed={false}
        onDismiss={onDismiss}
      />,
    );
    await user.click(screen.getByRole("button", { name: "닫기" }));
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });
});
