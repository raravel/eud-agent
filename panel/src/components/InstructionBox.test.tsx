/**
 * Instruction box v2 (features/06 ## Behaviors → Send gating):
 *   chat {text}; Send disabled while busy (turn in flight / plan awaiting a
 *   decision / compiling) and when the store gates it off (no project). The
 *   settable-target requirement is GONE — the agent picks files/targets, so
 *   there is no picker selection and no useContext toggle. Gate on `canSend`.
 *
 * Contract (`@/components/InstructionBox`):
 *   export interface InstructionBoxProps {
 *     state: PanelState;            // gating: canSend
 *     onSend(msg: ChatPayload): void;
 *   }
 *   export interface ChatPayload { text: string; }
 *
 * The instruction textarea has accessible name "지시 입력"; the send button "전송".
 */
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createPanelStore, type PanelState } from "@/state/store";
import { InstructionBox, type ChatPayload } from "@/components/InstructionBox";

function readyState(): PanelState {
  const store = createPanelStore();
  store.wsOpen();
  store.applyStatus({ compiling: false, project: "MyMap" });
  store.applyList({
    files: [{ path: "main.eps", ftype: "CUIEps", settable: true }],
  });
  return store.getState(); // ready, canSend: true
}

const noop = () => {};

describe("InstructionBox — send gating (v2)", () => {
  it("enables Send when connected with an open project (ready)", () => {
    render(<InstructionBox state={readyState()} onSend={noop} />);
    expect(screen.getByRole("button", { name: "전송" })).toBeEnabled();
  });

  it("enables Send for an empty-but-open project (no settable-target gate)", () => {
    const store = createPanelStore();
    store.wsOpen();
    store.applyStatus({ compiling: false, project: "MyMap" });
    store.applyList({ files: [] }); // zero files, still open
    render(<InstructionBox state={store.getState()} onSend={noop} />);
    expect(screen.getByRole("button", { name: "전송" })).toBeEnabled();
  });

  it("disables Send when no project is open", () => {
    const store = createPanelStore();
    store.wsOpen();
    store.applyList({ error: "no project" });
    render(<InstructionBox state={store.getState()} onSend={noop} />);
    expect(screen.getByRole("button", { name: "전송" })).toBeDisabled();
  });

  it("disables Send while busy (thinking)", () => {
    const store = createPanelStore();
    store.wsOpen();
    store.applyList({
      files: [{ path: "main.eps", ftype: "CUIEps", settable: true }],
    });
    store.chatSent(); // thinking
    render(<InstructionBox state={store.getState()} onSend={noop} />);
    expect(screen.getByRole("button", { name: "전송" })).toBeDisabled();
  });

  it("disables Send while the editor is compiling", () => {
    const store = createPanelStore();
    store.wsOpen();
    store.applyList({
      files: [{ path: "main.eps", ftype: "CUIEps", settable: true }],
    });
    store.applyStatus({ compiling: true, project: "MyMap" });
    render(<InstructionBox state={store.getState()} onSend={noop} />);
    expect(screen.getByRole("button", { name: "전송" })).toBeDisabled();
  });
});

describe("InstructionBox — 새 대화 reset control (EUD-065)", () => {
  it("renders a [새 대화] button", () => {
    render(<InstructionBox state={readyState()} onSend={noop} onReset={noop} />);
    expect(screen.getByRole("button", { name: "새 대화" })).toBeInTheDocument();
  });

  it("calls onReset when [새 대화] is clicked", async () => {
    const user = userEvent.setup();
    const onReset = vi.fn();
    render(
      <InstructionBox state={readyState()} onSend={noop} onReset={onReset} />,
    );
    await user.click(screen.getByRole("button", { name: "새 대화" }));
    expect(onReset).toHaveBeenCalledTimes(1);
  });

  it("disables [새 대화] while a turn is in flight", () => {
    const store = createPanelStore();
    store.wsOpen();
    store.applyList({
      files: [{ path: "main.eps", ftype: "CUIEps", settable: true }],
    });
    store.chatSent(); // thinking — a turn is in flight
    render(
      <InstructionBox state={store.getState()} onSend={noop} onReset={noop} />,
    );
    expect(screen.getByRole("button", { name: "새 대화" })).toBeDisabled();
  });
});

describe("InstructionBox — chat payload (v2)", () => {
  it("sends the trimmed instruction text as {text}", async () => {
    const user = userEvent.setup();
    const onSend = vi.fn<(p: ChatPayload) => void>();
    render(<InstructionBox state={readyState()} onSend={onSend} />);
    await user.type(screen.getByRole("textbox", { name: "지시 입력" }), "트리거 추가");
    await user.click(screen.getByRole("button", { name: "전송" }));
    expect(onSend).toHaveBeenCalledWith({ text: "트리거 추가" });
  });

  it("does not send empty / whitespace-only text", async () => {
    const user = userEvent.setup();
    const onSend = vi.fn<(p: ChatPayload) => void>();
    render(<InstructionBox state={readyState()} onSend={onSend} />);
    await user.type(screen.getByRole("textbox", { name: "지시 입력" }), "   ");
    await user.click(screen.getByRole("button", { name: "전송" }));
    expect(onSend).not.toHaveBeenCalled();
  });
});

describe("InstructionBox — InputGroup composition (EUD-066 layout contract)", () => {
  // The InputGroup column layout depends on CSS `:has(> ...)` DIRECT-child
  // selectors (`has-[>[data-align=block-end]]:flex-col` / `:h-auto` in
  // ui/input-group.tsx). `:has(>)` is DOM-structural — a footer nested inside
  // the `display: contents` PromptInputBody does NOT match, leaving the group
  // a fixed-height flex ROW: the textarea collapses to ~24px and renders its
  // placeholder vertically (live defect, EUD-066). jsdom cannot do layout, so
  // this pins the DOM STRUCTURE the selectors require.
  it("renders the footer addon as a DIRECT child of the input group", () => {
    const { container } = render(
      <InstructionBox state={readyState()} onSend={noop} onReset={noop} />,
    );
    const group = container.querySelector('[data-slot="input-group"]');
    expect(group).not.toBeNull();
    const directAddon = group!.querySelector(
      ':scope > [data-slot="input-group-addon"][data-align="block-end"]',
    );
    expect(directAddon).not.toBeNull();
  });
});
