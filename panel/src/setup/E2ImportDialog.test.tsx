import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { preflightE2Project } from "@/lib/projectImport";
import { E2ImportDialog, type E2ImportDialogProps } from "@/setup/E2ImportDialog";
import type {
  E2ImportCompleted,
  E2ImportProgress,
  E2PreflightResult,
  E2RelinkNeed,
} from "@/lib/projectImport";
import type { E2ImportEventMessage } from "@/lib/protocol";

const destination = { path: "C:\\Work\\Imported", empty: true };
const readyPreflight: E2PreflightResult = {
  state: "ready",
  fingerprint: "fingerprint-ready",
  sourceKind: "e2s",
  features: ["grp"],
  warnings: [],
};
const completed: E2ImportCompleted = {
  state: "completed",
  importId: "real-import-id",
  fingerprint: "fingerprint-ready",
  projectOpened: true,
  projectPath: "C:\\Work\\Imported",
};
const mapNeed: E2RelinkNeed = {
  id: "map",
  kind: "inputMap",
  label: "원본 맵",
  originalPath: "C:\\Legacy\\map.scx",
  candidates: [],
  extensions: ["scx"],
};
const resourceNeed: E2RelinkNeed = {
  id: "resource",
  kind: "resource",
  label: "필요한 리소스",
  candidates: [],
  extensions: ["wav"],
};

function importEvent(result: E2ImportEventMessage["result"]): E2ImportEventMessage {
  return { type: "e2_import", result };
}

function deferred<T>() {
  let resolve: (value: T) => void = () => {};
  const promise = new Promise<T>((nextResolve) => {
    resolve = nextResolve;
  });
  return { promise, resolve };
}

function renderDialog(overrides: Partial<E2ImportDialogProps> = {}) {
  let messageHandler: ((message: E2ImportEventMessage) => void) | undefined;
  const props: E2ImportDialogProps = {
    open: true,
    onOpenChange: vi.fn(),
    pickSource: vi.fn(async () => "C:\\Legacy\\sample.e2s"),
    pickDestination: vi.fn(async () => destination),
    pickRelink: vi.fn(async (need: E2RelinkNeed) => `C:\\Found\\${need.id}.bin`),
    preflight: vi.fn(async () => readyPreflight),
    importProject: vi.fn(async () => completed),
    cancelImport: vi.fn(async () => ({ state: "cancelled", importId: "real-import-id", fingerprint: "fingerprint-ready" })),
    subscribe: vi.fn((handler: (message: E2ImportEventMessage) => void) => {
      messageHandler = handler;
      return () => {};
    }),
    onImported: vi.fn(),
    ...overrides,
  };
  render(<E2ImportDialog {...props} />);
  return {
    props,
    emit(message: E2ImportEventMessage) {
      if (messageHandler === undefined) throw new Error("e2 import listener was not registered");
      messageHandler(message);
    },
  };
}

async function chooseSourceAndDestination() {
  await userEvent.click(screen.getByRole("button", { name: "E2S/E2P 선택" }));
  await userEvent.click(screen.getByRole("button", { name: "새 작업 폴더 선택" }));
}

describe("E2ImportDialog", () => {
  it("enables import after the real transport decodes a Rust Ready payload", async () => {
    const invoke = vi.fn(async () => ({
      state: "ready",
      fingerprint: "fingerprint-from-rust",
      sourceKind: "e2p",
      features: ["projectMap", "buttonsBgm"],
      warnings: [{
        code: "invalid_resource",
        severity: "warning",
        message: "one resource needs attention",
        feature: "buttonsBgm",
      }],
    }));
    const preflight = vi.fn((request) => preflightE2Project(request, invoke));
    const { props } = renderDialog({ preflight });

    await chooseSourceAndDestination();

    expect(invoke).toHaveBeenCalledWith("setup_e2_preflight", {
      request: {
        sourcePath: "C:\\Legacy\\sample.e2s",
        destinationPath: "C:\\Work\\Imported",
        relinks: [],
      },
    });
    expect(screen.getByRole("button", { name: "선택한 위치로 가져오기" })).toBeEnabled();
    expect(props.preflight).toHaveBeenCalledTimes(1);
  });

  it("starts only after the ordered source, destination, and preflight steps succeed", async () => {
    const callOrder: string[] = [];
    const importProject = vi.fn(async () => {
      callOrder.push("import");
      return completed;
    });
    const { props } = renderDialog({
      subscribe: (handler) => {
        callOrder.push("subscribe");
        void handler;
        return () => {};
      },
      importProject,
    });

    expect(screen.getByRole("heading", { name: "E2 프로젝트 가져오기" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "선택한 위치로 가져오기" })).toBeDisabled();

    await chooseSourceAndDestination();
    expect(props.preflight).toHaveBeenCalledWith({
      sourcePath: "C:\\Legacy\\sample.e2s",
      destinationPath: "C:\\Work\\Imported",
      relinks: [],
    });

    await userEvent.click(screen.getByRole("button", { name: "선택한 위치로 가져오기" }));

    expect(callOrder).toEqual(["subscribe", "import"]);
    expect(props.importProject).toHaveBeenCalledWith({
      sourcePath: "C:\\Legacy\\sample.e2s",
      destinationPath: "C:\\Work\\Imported",
      fingerprint: "fingerprint-ready",
      relinks: [],
    });
    expect(props.onImported).toHaveBeenCalledWith(completed);
    expect(props.onOpenChange).toHaveBeenCalledWith(false);
  });

  it("returns to idle when either native picker is cancelled", async () => {
    const sourcePicker = vi.fn()
      .mockResolvedValueOnce(null)
      .mockResolvedValueOnce("C:\\Legacy\\sample.e2p");
    const destinationPicker = vi.fn(async () => null);
    const { props } = renderDialog({
      pickSource: sourcePicker,
      pickDestination: destinationPicker,
    });

    await userEvent.click(screen.getByRole("button", { name: "E2S/E2P 선택" }));
    expect(screen.getByRole("button", { name: "새 작업 폴더 선택" })).toBeDisabled();
    expect(props.preflight).not.toHaveBeenCalled();

    await userEvent.click(screen.getByRole("button", { name: "E2S/E2P 선택" }));
    await userEvent.click(screen.getByRole("button", { name: "새 작업 폴더 선택" }));
    expect(destinationPicker).toHaveBeenCalledTimes(1);
    expect(props.preflight).not.toHaveBeenCalled();
  });

  it("relinks and completes", async () => {
    const afterMap: E2PreflightResult = {
      state: "needsRelink",
      fingerprint: "fingerprint-map",
      needs: [resourceNeed],
    };
    const afterResource: E2PreflightResult = {
      ...readyPreflight,
      fingerprint: "fingerprint-final",
    };
    const finalCompletion: E2ImportCompleted = { ...completed, fingerprint: "fingerprint-final" };
    const { props } = renderDialog({
      preflight: vi.fn()
        .mockResolvedValueOnce({ state: "needsRelink", fingerprint: "fingerprint-initial", needs: [mapNeed, resourceNeed] })
        .mockResolvedValueOnce(afterMap)
        .mockResolvedValueOnce(afterResource),
      importProject: vi.fn(async () => finalCompletion),
    });

    await chooseSourceAndDestination();
    await userEvent.click(screen.getByRole("button", { name: "원본 맵 다시 연결" }));
    await userEvent.click(screen.getByRole("button", { name: "리소스 다시 연결" }));
    await userEvent.click(screen.getByRole("button", { name: "선택한 위치로 가져오기" }));

    expect(props.preflight).toHaveBeenLastCalledWith({
      sourcePath: "C:\\Legacy\\sample.e2s",
      destinationPath: "C:\\Work\\Imported",
      relinks: [
        { needId: "map", path: "C:\\Found\\map.bin" },
        { needId: "resource", path: "C:\\Found\\resource.bin" },
      ],
    });
    expect(props.importProject).toHaveBeenCalledWith({
      sourcePath: "C:\\Legacy\\sample.e2s",
      destinationPath: "C:\\Work\\Imported",
      fingerprint: "fingerprint-final",
      relinks: [
        { needId: "map", path: "C:\\Found\\map.bin" },
        { needId: "resource", path: "C:\\Found\\resource.bin" },
      ],
    });
    expect(props.onImported).toHaveBeenCalledWith(finalCompletion);
  });

  it.each([
    ["active SCDB", "active_scdb_unsupported", "SCDB active", "활성 SCDB 데이터"],
    ["effective GRP", "active_grp_unsupported", "GRP active", "실제로 사용 중인 GRP"],
    ["unsupported version", "unsupported_version", "version unsupported", "지원하는 EUD Editor 2 버전"],
    ["malformed input", "malformed_input", "input malformed", "파일 구조를 읽을 수 없습니다"],
    ["unverified ChatWrite", "active_chatwrite_unverified", "ChatWrite active", "ChatWrite 동작"],
  ])("shows %s as an actionable fatal diagnostic", async (_name, code, message, recovery) => {
    const fatal: E2PreflightResult = {
      state: "fatal",
      issues: [{ code, severity: "fatal", message }],
    };
    const { props } = renderDialog({ preflight: vi.fn(async () => fatal) });
    await chooseSourceAndDestination();
    expect(screen.getByRole("alert")).toHaveTextContent(recovery);
    await userEvent.click(screen.getByText("오류 상세"));
    expect(screen.getByText(`${code}: ${message}`)).toBeVisible();
    expect(props.onImported).not.toHaveBeenCalled();
  });

  it("keeps inert GRP nonfatal during preflight", async () => {
    const { props } = renderDialog();
    await chooseSourceAndDestination();
    expect(screen.getByText("연결 파일을 확인했습니다. 가져오기를 시작할 수 있습니다.")).toBeVisible();
    expect(props.onImported).not.toHaveBeenCalled();
  });

  it.each([
    ["unsupported version", "unsupported_version", "version unsupported", "지원하는 EUD Editor 2 버전"],
    ["malformed input", "malformed_input", "input malformed", "파일 구조를 읽을 수 없습니다"],
    ["unverified ChatWrite", "active_chatwrite_unverified", "ChatWrite active", "ChatWrite 동작"],
  ])("reports a fingerprint-less %s import fatal and returns to a closable idle state", async (_name, code, message, recovery) => {
    const pending = deferred<unknown>();
    const { props } = renderDialog({ importProject: vi.fn(() => pending.promise) });
    await chooseSourceAndDestination();
    await userEvent.click(screen.getByRole("button", { name: "선택한 위치로 가져오기" }));
    await waitFor(() => expect(props.subscribe).toHaveBeenCalled());

    pending.resolve({ state: "fatal", issues: [{ code, severity: "fatal", message }] });
    await pending.promise;
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent(recovery));
    expect(screen.getByRole("dialog")).toHaveAttribute("aria-busy", "false");
    expect(props.onImported).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "취소" }));
    expect(props.onOpenChange).toHaveBeenCalledWith(false);
  });

  it("ignores stale fingerprints and wrong import IDs before activating the matching completed event", async () => {
    const pending = deferred<unknown>();
    const { props, emit } = renderDialog({ importProject: vi.fn(() => pending.promise) });
    await chooseSourceAndDestination();
    await userEvent.click(screen.getByRole("button", { name: "선택한 위치로 가져오기" }));
    await waitFor(() => expect(props.subscribe).toHaveBeenCalled());

    const progress: E2ImportProgress = {
      state: "progress",
      importId: "real-import-id",
      fingerprint: "fingerprint-ready",
      phase: "materializing",
      completed: 2,
      total: 5,
      message: "파일을 만드는 중",
    };
    emit(importEvent(progress));
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("2 / 5"));

    emit(importEvent({ ...completed, fingerprint: "stale-fingerprint" }));
    expect(props.onImported).not.toHaveBeenCalled();
    emit(importEvent({ ...completed, importId: "wrong-import-id" }));
    expect(props.onImported).not.toHaveBeenCalled();
    emit(importEvent(completed));
    expect(props.onImported).toHaveBeenCalledTimes(1);
    pending.resolve(completed);
  });

  it("does not activate a cancelled attempt when its late completion settles after a new import begins", async () => {
    const firstPending = deferred<unknown>();
    const secondPending = deferred<unknown>();
    const { props, emit } = renderDialog({
      importProject: vi.fn()
        .mockImplementationOnce(() => firstPending.promise)
        .mockImplementationOnce(() => secondPending.promise),
      cancelImport: vi.fn(async () => ({ state: "cancelled", importId: "real-import-id", fingerprint: "fingerprint-ready" })),
    });
    await chooseSourceAndDestination();
    await userEvent.click(screen.getByRole("button", { name: "선택한 위치로 가져오기" }));
    await waitFor(() => expect(props.subscribe).toHaveBeenCalled());
    emit(importEvent({
      state: "progress",
      importId: "real-import-id",
      fingerprint: "fingerprint-ready",
      phase: "opening",
      completed: 4,
      total: 5,
      message: "프로젝트를 여는 중",
    }));

    await waitFor(() => expect(screen.getByRole("button", { name: "가져오기 취소" })).toBeEnabled());
    await userEvent.click(screen.getByRole("button", { name: "가져오기 취소" }));
    expect(props.cancelImport).toHaveBeenCalledWith({ importId: "real-import-id" });
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("취소했습니다"));
    await userEvent.click(screen.getByRole("button", { name: "다른 E2S/E2P 선택" }));
    await userEvent.click(screen.getByRole("button", { name: "새 작업 폴더 선택" }));
    await userEvent.click(screen.getByRole("button", { name: "선택한 위치로 가져오기" }));
    await waitFor(() => expect(props.importProject).toHaveBeenCalledTimes(2));

    firstPending.resolve(completed);
    await firstPending.promise;
    await waitFor(() => expect(props.onImported).not.toHaveBeenCalled());
    expect(screen.getByRole("dialog")).toHaveAttribute("aria-busy", "true");
    secondPending.resolve({ state: "cancelled", importId: "second-import-id", fingerprint: "fingerprint-ready" });
  });

  it("activates exactly once when the matching terminal event and invocation result race", async () => {
    const pending = deferred<unknown>();
    const { props, emit } = renderDialog({ importProject: vi.fn(() => pending.promise) });
    await chooseSourceAndDestination();
    await userEvent.click(screen.getByRole("button", { name: "선택한 위치로 가져오기" }));
    await waitFor(() => expect(props.subscribe).toHaveBeenCalled());

    emit(importEvent(completed));
    expect(props.onImported).toHaveBeenCalledTimes(1);
    pending.resolve(completed);
    await pending.promise;
    await waitFor(() => expect(props.onImported).toHaveBeenCalledTimes(1));
  });

  it("rejects false completion and keeps keyboard controls and busy status accessible", async () => {
    const { props } = renderDialog({
      importProject: vi.fn(async () => ({
        state: "completed",
        importId: "real-import-id",
        fingerprint: "fingerprint-ready",
        projectOpened: false,
        projectPath: "C:\\Work\\Imported",
      })),
    });
    const user = userEvent.setup();
    await user.tab();
    expect(screen.getByRole("dialog")).toHaveFocus();
    expect(screen.getByRole("button", { name: "E2S/E2P 선택" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "새 작업 폴더 선택" })).toBeDisabled();
    await chooseSourceAndDestination();

    await user.click(screen.getByRole("button", { name: "선택한 위치로 가져오기" }));

    expect(screen.getByRole("alert")).toHaveTextContent("가져오기 명령을 실행하지 못했습니다");
    expect(screen.getByRole("dialog")).toHaveAttribute("aria-busy", "false");
    expect(props.onImported).not.toHaveBeenCalled();
  });
});
