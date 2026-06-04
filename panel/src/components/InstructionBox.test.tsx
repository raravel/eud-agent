/**
 * Instruction box (features/03 ## Behaviors → Instruct):
 *   instruct {instruction, target, useContext}; useContext checkbox default ON;
 *   Send disabled while working/applying/waiting and when the store gates it
 *   off (no settable picker selection / no project).
 *
 * INSTRUCT TARGET CONTRACT (features/02 orchestrator.py): instruct.target MUST
 * be an EXISTING settable file — the server GETs it for the diff stage. New-file
 * mode does NOT change instruct.target; the picker's selected settable file is
 * ALWAYS the instruct target. New-file creation happens only via apply
 * {mode:"neweps"} (the NEWEPS filename belongs to the ApplyBar tests). So
 * instruct is gated on canSendSet in BOTH modes, and an empty project (zero
 * settable files) cannot run instruct even in new-file mode.
 *
 * Contract (`@/components/InstructionBox`):
 *   export interface InstructionBoxProps {
 *     state: PanelState;            // gating: canSendSet, phase
 *     onSend(msg: InstructPayload): void;
 *   }
 *   export interface InstructPayload {
 *     instruction: string;
 *     target: string;               // ALWAYS the picker's selected settable file
 *     useContext: boolean;
 *   }
 *   export function InstructionBox(props): JSX.Element;
 *
 * The instruction textarea has accessible name "지시 입력"; the useContext
 * checkbox has accessible name "컨텍스트 사용"; the send button "전송".
 */
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createPanelStore, type PanelState } from "@/state/store";
import { InstructionBox, type InstructPayload } from "@/components/InstructionBox";

function readySetState(): PanelState {
  const store = createPanelStore();
  store.wsOpen();
  store.applyStatus({ compiling: false, project: "MyMap" });
  store.applyList({
    files: [{ path: "main.eps", ftype: "CUIEps", settable: true }],
  });
  store.selectTarget("main.eps");
  return store.getState(); // ready, canSendSet: true
}

const noop = () => {};

describe("InstructionBox — useContext checkbox", () => {
  it("defaults to ON (checked)", () => {
    render(<InstructionBox state={readySetState()} onSend={noop} />);
    expect(screen.getByRole("checkbox", { name: "컨텍스트 사용" })).toBeChecked();
  });
});

describe("InstructionBox — send gating", () => {
  it("disables Send when the store gates SET off (no settable target)", () => {
    const store = createPanelStore();
    store.wsOpen();
    store.applyStatus({ compiling: false, project: "MyMap" });
    store.applyList({ files: [] }); // empty-but-open project, SET mode
    render(<InstructionBox state={store.getState()} onSend={noop} />);
    expect(screen.getByRole("button", { name: "전송" })).toBeDisabled();
  });

  it("disables Send while working", () => {
    const store = createPanelStore();
    store.wsOpen();
    store.applyStatus({ compiling: false, project: "MyMap" });
    store.applyList({
      files: [{ path: "main.eps", ftype: "CUIEps", settable: true }],
    });
    store.selectTarget("main.eps");
    store.instructSent(); // working
    render(<InstructionBox state={store.getState()} onSend={noop} />);
    expect(screen.getByRole("button", { name: "전송" })).toBeDisabled();
  });

  it("enables Send in SET mode with a settable target selected and ready", () => {
    render(<InstructionBox state={readySetState()} onSend={noop} />);
    expect(screen.getByRole("button", { name: "전송" })).toBeEnabled();
  });

  it("disables Send in new-file mode with an EMPTY project (no diff target)", () => {
    // New-file mode does NOT relax the instruct target requirement: with zero
    // settable files there is no file for the server's diff stage to GET, so
    // instruct cannot run at all (gated on canSendSet, not canSendNewEps).
    const store = createPanelStore();
    store.wsOpen();
    store.applyStatus({ compiling: false, project: "MyMap" });
    store.applyList({ files: [] });
    store.setNewFileMode(true);
    render(<InstructionBox state={store.getState()} onSend={noop} />);
    expect(screen.getByRole("button", { name: "전송" })).toBeDisabled();
  });
});

describe("InstructionBox — instruct payload", () => {
  it("sends instruction text, the selected target, and useContext=true by default", async () => {
    const user = userEvent.setup();
    const onSend = vi.fn<(p: InstructPayload) => void>();
    render(<InstructionBox state={readySetState()} onSend={onSend} />);
    await user.type(screen.getByRole("textbox", { name: "지시 입력" }), "트리거 추가");
    await user.click(screen.getByRole("button", { name: "전송" }));
    expect(onSend).toHaveBeenCalledWith({
      instruction: "트리거 추가",
      target: "main.eps",
      useContext: true,
    });
  });

  it("sends useContext=false after unchecking the checkbox", async () => {
    const user = userEvent.setup();
    const onSend = vi.fn<(p: InstructPayload) => void>();
    render(<InstructionBox state={readySetState()} onSend={onSend} />);
    await user.click(screen.getByRole("checkbox", { name: "컨텍스트 사용" }));
    await user.type(screen.getByRole("textbox", { name: "지시 입력" }), "x");
    await user.click(screen.getByRole("button", { name: "전송" }));
    expect(onSend).toHaveBeenCalledWith(
      expect.objectContaining({ useContext: false }),
    );
  });

  it("sends the PICKER selection as target even in new-file mode (not the NEWEPS name)", async () => {
    // The bug fix: instruct.target must be the existing settable picker file so
    // the server's diff stage can GET it. New-file mode only affects apply.
    const user = userEvent.setup();
    const onSend = vi.fn<(p: InstructPayload) => void>();
    const store = createPanelStore();
    store.wsOpen();
    store.applyStatus({ compiling: false, project: "MyMap" });
    store.applyList({
      files: [{ path: "main.eps", ftype: "CUIEps", settable: true }],
    });
    store.selectTarget("main.eps");
    store.setNewFileMode(true); // new-file mode ON, but a picker file is selected
    render(<InstructionBox state={store.getState()} onSend={onSend} />);
    await user.type(screen.getByRole("textbox", { name: "지시 입력" }), "새 기능");
    await user.click(screen.getByRole("button", { name: "전송" }));
    expect(onSend).toHaveBeenCalledWith(
      expect.objectContaining({ target: "main.eps" }),
    );
  });
});
