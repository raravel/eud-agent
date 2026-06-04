/**
 * Target picker (features/03 ## Behaviors → Target picker):
 *   - Select fed by the store's `files`.
 *   - non-settable (GUI) options disabled, tooltip "읽기 전용 파일 형식".
 *   - refresh button.
 *   - no project (hasProject false) → placeholder "프로젝트를 열어주세요".
 *   - new-file toggle + NEWEPS filename input with inline validation error.
 *
 * Contract (Step B implements `@/components/TargetPicker`):
 *   export interface TargetPickerProps {
 *     state: PanelState;
 *     newEpsName: string;
 *     onSelectTarget(path: string): void;
 *     onToggleNewFile(on: boolean): void;
 *     onRefresh(): void;
 *     onChangeNewEpsName(name: string): void;
 *   }
 *   export function TargetPicker(props: TargetPickerProps): JSX.Element;
 *
 * The Select options carry `data-testid="target-option-<path>"` and the GUI
 * ones are disabled (aria-disabled / [data-disabled]). The new-file checkbox
 * has accessible name "새 파일"; the NEWEPS input has accessible name
 * "새 파일 이름".
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createPanelStore, type PanelState } from "@/state/store";
import { TargetPicker } from "@/components/TargetPicker";

function stateWithFiles(): PanelState {
  const store = createPanelStore();
  store.wsOpen();
  store.applyStatus({ compiling: false, project: "MyMap" });
  store.applyList({
    files: [
      { path: "main.eps", ftype: "CUIEps", settable: true },
      { path: "scene.tdf", ftype: "GUI", settable: false },
      { path: "extra.eps", ftype: "CUIEps", settable: true },
    ],
  });
  return store.getState();
}

function noProjectState(): PanelState {
  const store = createPanelStore();
  store.wsOpen();
  // bridge "ERROR: no project" → error{message}; store clears hasProject.
  store.errorReceived("ERROR: no project");
  return store.getState();
}

const noop = () => {};

describe("TargetPicker — file options", () => {
  it("renders one option per file from store state", () => {
    render(
      <TargetPicker
        state={stateWithFiles()}
        newEpsName=""
        onSelectTarget={noop}
        onToggleNewFile={noop}
        onRefresh={noop}
        onChangeNewEpsName={noop}
      />,
    );
    expect(screen.getByTestId("target-option-main.eps")).toBeInTheDocument();
    expect(screen.getByTestId("target-option-scene.tdf")).toBeInTheDocument();
    expect(screen.getByTestId("target-option-extra.eps")).toBeInTheDocument();
  });

  it("disables non-settable (GUI) options", () => {
    render(
      <TargetPicker
        state={stateWithFiles()}
        newEpsName=""
        onSelectTarget={noop}
        onToggleNewFile={noop}
        onRefresh={noop}
        onChangeNewEpsName={noop}
      />,
    );
    const gui = screen.getByTestId("target-option-scene.tdf");
    expect(gui).toHaveAttribute("aria-disabled", "true");
    // a settable option is not disabled
    const ok = screen.getByTestId("target-option-main.eps");
    expect(ok).not.toHaveAttribute("aria-disabled", "true");
  });

  it("shows the read-only tooltip text on the GUI option", () => {
    render(
      <TargetPicker
        state={stateWithFiles()}
        newEpsName=""
        onSelectTarget={noop}
        onToggleNewFile={noop}
        onRefresh={noop}
        onChangeNewEpsName={noop}
      />,
    );
    const gui = screen.getByTestId("target-option-scene.tdf");
    expect(gui).toHaveAttribute("title", "읽기 전용 파일 형식");
  });

  it("invokes onSelectTarget with the chosen path", async () => {
    const user = userEvent.setup();
    const onSelectTarget = vi.fn();
    render(
      <TargetPicker
        state={stateWithFiles()}
        newEpsName=""
        onSelectTarget={onSelectTarget}
        onToggleNewFile={noop}
        onRefresh={noop}
        onChangeNewEpsName={noop}
      />,
    );
    await user.click(screen.getByTestId("target-option-extra.eps"));
    expect(onSelectTarget).toHaveBeenCalledWith("extra.eps");
  });

  it("does NOT select a disabled GUI option on click", async () => {
    const user = userEvent.setup();
    const onSelectTarget = vi.fn();
    render(
      <TargetPicker
        state={stateWithFiles()}
        newEpsName=""
        onSelectTarget={onSelectTarget}
        onToggleNewFile={noop}
        onRefresh={noop}
        onChangeNewEpsName={noop}
      />,
    );
    await user.click(screen.getByTestId("target-option-scene.tdf"));
    expect(onSelectTarget).not.toHaveBeenCalled();
  });
});

describe("TargetPicker — no project", () => {
  it("shows the '프로젝트를 열어주세요' placeholder when no project is open", () => {
    render(
      <TargetPicker
        state={noProjectState()}
        newEpsName=""
        onSelectTarget={noop}
        onToggleNewFile={noop}
        onRefresh={noop}
        onChangeNewEpsName={noop}
      />,
    );
    expect(screen.getByText("프로젝트를 열어주세요")).toBeInTheDocument();
  });
});

describe("TargetPicker — refresh", () => {
  it("invokes onRefresh when the refresh button is clicked", async () => {
    const user = userEvent.setup();
    const onRefresh = vi.fn();
    render(
      <TargetPicker
        state={stateWithFiles()}
        newEpsName=""
        onSelectTarget={noop}
        onToggleNewFile={noop}
        onRefresh={onRefresh}
        onChangeNewEpsName={noop}
      />,
    );
    await user.click(screen.getByRole("button", { name: "새로고침" }));
    expect(onRefresh).toHaveBeenCalledTimes(1);
  });
});

describe("TargetPicker — new-file mode", () => {
  let state: PanelState;
  beforeEach(() => {
    const store = createPanelStore();
    store.wsOpen();
    store.applyStatus({ compiling: false, project: "MyMap" });
    store.applyList({ files: [] });
    store.setNewFileMode(true);
    state = store.getState();
  });

  it("renders the NEWEPS filename input when new-file mode is on", () => {
    render(
      <TargetPicker
        state={state}
        newEpsName=""
        onSelectTarget={noop}
        onToggleNewFile={noop}
        onRefresh={noop}
        onChangeNewEpsName={noop}
      />,
    );
    expect(screen.getByLabelText("새 파일 이름")).toBeInTheDocument();
  });

  it("shows an inline validation error for a name with a path separator", () => {
    render(
      <TargetPicker
        state={state}
        newEpsName="bad/name"
        onSelectTarget={noop}
        onToggleNewFile={noop}
        onRefresh={noop}
        onChangeNewEpsName={noop}
      />,
    );
    expect(
      screen.getByText(/경로 구분자/),
    ).toBeInTheDocument();
  });

  it("shows no validation error for a valid name", () => {
    render(
      <TargetPicker
        state={state}
        newEpsName="good"
        onSelectTarget={noop}
        onToggleNewFile={noop}
        onRefresh={noop}
        onChangeNewEpsName={noop}
      />,
    );
    expect(screen.queryByText(/경로 구분자/)).not.toBeInTheDocument();
    expect(screen.queryByText("파일 이름을 입력하세요.")).not.toBeInTheDocument();
  });

  it("propagates typed characters via onChangeNewEpsName", async () => {
    const user = userEvent.setup();
    const onChangeNewEpsName = vi.fn();
    render(
      <TargetPicker
        state={state}
        newEpsName=""
        onSelectTarget={noop}
        onToggleNewFile={noop}
        onRefresh={noop}
        onChangeNewEpsName={onChangeNewEpsName}
      />,
    );
    await user.type(screen.getByLabelText("새 파일 이름"), "x");
    expect(onChangeNewEpsName).toHaveBeenCalled();
  });
});
