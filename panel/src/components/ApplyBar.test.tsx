/**
 * Apply bar (features/03 ## Behaviors → Apply):
 *   - SET  → apply {mode:"set",    target, code}  — code is the Monaco buffer.
 *   - NEWEPS → apply {mode:"neweps", target:<filename>, code} — filename validated.
 *   - Cancel returns to ready.
 *   - Buttons gated by the store (canSendSet / canSendNewEps) + applying phase.
 *   - Diagnostics NEVER block Apply (no diagnostics input to this component).
 *
 * Contract (Step B implements `@/components/ApplyBar`):
 *   export interface ApplyBarProps {
 *     state: PanelState;          // gating: canSendSet, canSendNewEps, phase
 *     editedCode: string;         // Monaco buffer — Apply SET/NEWEPS body
 *     newEpsName: string;         // raw NEWEPS filename (validated here)
 *     onApply(payload: ApplyPayload): void;  // assembled apply message body
 *     onCancel(): void;
 *   }
 *   export interface ApplyPayload { mode: "set" | "neweps"; target: string; code: string; }
 *   export function ApplyBar(props): JSX.Element;
 *
 * Button accessible names: "적용 (SET)", "새 파일로 적용 (NEWEPS)", "취소".
 */
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createPanelStore, type PanelState } from "@/state/store";
import { ApplyBar, type ApplyPayload } from "@/components/ApplyBar";

function reviewingSetState(): PanelState {
  const store = createPanelStore();
  store.wsOpen();
  store.applyStatus({ compiling: false, project: "MyMap" });
  store.applyList({
    files: [{ path: "main.eps", ftype: "CUIEps", settable: true }],
  });
  store.selectTarget("main.eps");
  store.instructSent();
  store.codeReceived({ code: "seed", lang: "eps", diff: "", diagnostics: [] });
  return store.getState(); // phase: reviewing, canSendSet: true
}

function reviewingNewFileState(): PanelState {
  const store = createPanelStore();
  store.wsOpen();
  store.applyStatus({ compiling: false, project: "MyMap" });
  store.applyList({ files: [] }); // empty-but-open project
  store.setNewFileMode(true);
  store.instructSent();
  store.codeReceived({ code: "seed", lang: "eps", diff: "", diagnostics: [] });
  return store.getState(); // phase: reviewing, canSendNewEps: true, canSendSet: false
}

const noop = () => {};

describe("ApplyBar — SET payload uses the Monaco buffer", () => {
  it("sends mode:set, the selected target, and the EDITED code (not the original)", async () => {
    const user = userEvent.setup();
    const onApply = vi.fn<(p: ApplyPayload) => void>();
    render(
      <ApplyBar
        state={reviewingSetState()}
        editedCode="EDITED-IN-MONACO"
        newEpsName=""
        onApply={onApply}
        onCancel={noop}
      />,
    );
    await user.click(screen.getByRole("button", { name: "적용 (SET)" }));
    expect(onApply).toHaveBeenCalledWith({
      mode: "set",
      target: "main.eps",
      code: "EDITED-IN-MONACO",
    });
  });
});

describe("ApplyBar — NEWEPS payload uses the validated filename", () => {
  it("sends mode:neweps with the trimmed filename as target and the edited code", async () => {
    const user = userEvent.setup();
    const onApply = vi.fn<(p: ApplyPayload) => void>();
    render(
      <ApplyBar
        state={reviewingNewFileState()}
        editedCode="NEW-CODE"
        newEpsName="  feature.eps  "
        onApply={onApply}
        onCancel={noop}
      />,
    );
    await user.click(
      screen.getByRole("button", { name: "새 파일로 적용 (NEWEPS)" }),
    );
    expect(onApply).toHaveBeenCalledWith({
      mode: "neweps",
      target: "feature.eps",
      code: "NEW-CODE",
    });
  });

  it("does NOT apply NEWEPS when the filename is invalid (path separator)", async () => {
    const user = userEvent.setup();
    const onApply = vi.fn<(p: ApplyPayload) => void>();
    render(
      <ApplyBar
        state={reviewingNewFileState()}
        editedCode="NEW-CODE"
        newEpsName="bad/name"
        onApply={onApply}
        onCancel={noop}
      />,
    );
    const btn = screen.getByRole("button", { name: "새 파일로 적용 (NEWEPS)" });
    await user.click(btn);
    expect(onApply).not.toHaveBeenCalled();
  });

  it("does NOT apply NEWEPS when the filename is empty", async () => {
    const user = userEvent.setup();
    const onApply = vi.fn<(p: ApplyPayload) => void>();
    render(
      <ApplyBar
        state={reviewingNewFileState()}
        editedCode="NEW-CODE"
        newEpsName="   "
        onApply={onApply}
        onCancel={noop}
      />,
    );
    await user.click(
      screen.getByRole("button", { name: "새 파일로 적용 (NEWEPS)" }),
    );
    expect(onApply).not.toHaveBeenCalled();
  });
});

describe("ApplyBar — gating", () => {
  it("disables the SET button when canSendSet is false (no settable target)", () => {
    render(
      <ApplyBar
        state={reviewingNewFileState()} // canSendSet false (empty project)
        editedCode="x"
        newEpsName=""
        onApply={noop}
        onCancel={noop}
      />,
    );
    expect(screen.getByRole("button", { name: "적용 (SET)" })).toBeDisabled();
  });

  it("disables both apply buttons while applying", () => {
    const store = createPanelStore();
    store.wsOpen();
    store.applyStatus({ compiling: false, project: "MyMap" });
    store.applyList({
      files: [{ path: "main.eps", ftype: "CUIEps", settable: true }],
    });
    store.selectTarget("main.eps");
    store.instructSent();
    store.codeReceived({ code: "seed", lang: "eps", diff: "", diagnostics: [] });
    store.applySent(); // phase: applying
    render(
      <ApplyBar
        state={store.getState()}
        editedCode="x"
        newEpsName="ok.eps"
        onApply={noop}
        onCancel={noop}
      />,
    );
    expect(screen.getByRole("button", { name: "적용 (SET)" })).toBeDisabled();
    expect(
      screen.getByRole("button", { name: "새 파일로 적용 (NEWEPS)" }),
    ).toBeDisabled();
  });
});

describe("ApplyBar — cancel", () => {
  it("invokes onCancel when the Cancel button is clicked", async () => {
    const user = userEvent.setup();
    const onCancel = vi.fn();
    render(
      <ApplyBar
        state={reviewingSetState()}
        editedCode="x"
        newEpsName=""
        onApply={noop}
        onCancel={onCancel}
      />,
    );
    await user.click(screen.getByRole("button", { name: "취소" }));
    expect(onCancel).toHaveBeenCalledTimes(1);
  });
});
