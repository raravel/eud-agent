/**
 * Main PromptInput contracts: store-driven send gating, reset/model controls,
 * `chat {text, attachments}`, and picker/drop/paste attachment drafts. The
 * instruction textarea, attachment input, and visible controls retain stable
 * Korean accessible names used below.
 */
import { describe, it, expect, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
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
const codexSettings = {
  models: [
    {
      model: "gpt-default",
      displayName: "GPT Default",
      description: "기본 모델",
      supportedReasoningEfforts: [
        { reasoningEffort: "medium", description: "균형" },
        { reasoningEffort: "high", description: "깊게 추론" },
      ],
      defaultReasoningEffort: "medium",
      isDefault: true,
    },
    {
      model: "gpt-fast",
      displayName: "GPT Fast",
      description: "빠른 모델",
      supportedReasoningEfforts: [
        { reasoningEffort: "low", description: "빠른 추론" },
      ],
      defaultReasoningEffort: "low",
      isDefault: false,
    },
  ],
  selectedModel: "gpt-default",
  selectedReasoningEffort: "medium",
};

describe("InstructionBox — textarea sizing", () => {
  it("grows with multiline input until the capped height, then scrolls", () => {
    render(<InstructionBox state={readyState()} onSend={noop} />);
    const textarea = screen.getByRole("combobox", { name: "지시 입력" });

    fireEvent.change(textarea, {
      target: {
        value: Array.from({ length: 80 }, (_, index) => `긴 입력 ${index + 1}`).join(
          "\n",
        ),
      },
    });

    expect(textarea).toHaveClass(
      "min-h-16",
      "max-h-48",
      "field-sizing-content",
      "overflow-y-auto",
    );
    expect(textarea).not.toHaveClass("h-16", "max-h-16", "field-sizing-fixed");
  });
});


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


describe("InstructionBox — persistent active-turn feedback", () => {
  it("shows the real activity stage and keeps a stop action beside the prompt", async () => {
    const user = userEvent.setup();
    const store = createPanelStore();
    store.wsOpen();
    store.applyList({ files: [] });
    store.chatSent();
    const onCancel = vi.fn();
    const view = render(
      <InstructionBox
        state={store.getState()}
        onSend={noop}
        onCancel={onCancel}
      />,
    );

    expect(screen.getByTestId("active-turn-status")).toHaveTextContent(
      "작업 준비 중",
    );

    store.agentEvent("reasoning", "확인합니다.");
    view.rerender(
      <InstructionBox
        state={store.getState()}
        onSend={noop}
        onCancel={onCancel}
      />,
    );
    expect(screen.getByTestId("active-turn-status")).toHaveTextContent("추론 중");

    store.agentEvent("tool_call", "search_docs");
    view.rerender(
      <InstructionBox
        state={store.getState()}
        onSend={noop}
        onCancel={onCancel}
      />,
    );
    expect(screen.getByTestId("active-turn-status")).toHaveTextContent(
      "도구 실행 중 · search_docs",
    );
    await user.click(screen.getByRole("button", { name: "작업 중단" }));
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it("restores a rewound message into the editable prompt", () => {
    render(
      <InstructionBox
        state={readyState()}
        onSend={noop}
        draft={{
          text: "수정할 요청",
          attachments: [
            {
              id: "text-1",
              name: "notes.eps",
              mime: "text/plain",
              kind: "text",
              size: 12,
            },
          ],
          mentions: [
            {
              id: "mention-region",
              label: "영역 A",
              mention: {
                kind: "map.region",
                version: 1,
                projectId: "project-a",
                sourceFileSha256: "a".repeat(64),
                mapWidth: 64,
                mapHeight: 64,
                selectionId: "region-a",
                selectionSnapshotHash: "b".repeat(64),
              },
            },
          ],
        }}
      />,
    );

    expect(screen.getByRole("combobox", { name: "지시 입력" })).toHaveValue(
      "수정할 요청",
    );
    expect(screen.getByText("notes.eps")).toBeInTheDocument();
    expect(screen.getByTestId("mention-chips")).toHaveTextContent("@영역 A");
  });
});

describe("InstructionBox — plan_review feedback channel (EUD-074)", () => {
  function planReviewState(): PanelState {
    const store = createPanelStore();
    store.wsOpen();
    store.applyStatus({ compiling: false, project: "MyMap" });
    store.applyList({
      files: [{ path: "main.eps", ftype: "CUIEps", settable: true }],
    });
    store.chatSent();
    store.planReceived("# 계획", 1);
    return store.getState(); // plan_review
  }

  it("keeps Send ENABLED during plan_review (the input is the feedback channel)", () => {
    render(<InstructionBox state={planReviewState()} onSend={noop} />);
    expect(screen.getByRole("button", { name: "전송" })).toBeEnabled();
  });

  it("switches the placeholder to the plan-feedback guidance", () => {
    render(<InstructionBox state={planReviewState()} onSend={noop} />);
    const ta = screen.getByRole("combobox", { name: "지시 입력" });
    expect(ta).toHaveAttribute(
      "placeholder",
      expect.stringContaining("계획"),
    );
  });
});

describe("InstructionBox — chat payload (v2)", () => {
  it("sends the trimmed instruction text as {text}", async () => {
    const user = userEvent.setup();
    const onSend = vi.fn<(p: ChatPayload) => void>();
    render(<InstructionBox state={readyState()} onSend={onSend} />);
    await user.type(screen.getByRole("combobox", { name: "지시 입력" }), "트리거 추가");
    await user.click(screen.getByRole("button", { name: "전송" }));
    expect(onSend).toHaveBeenCalledWith({
      text: "트리거 추가",
      attachments: [],
      mentions: [],
    });
  });

  it("does not send empty / whitespace-only text", async () => {
    const user = userEvent.setup();
    const onSend = vi.fn<(p: ChatPayload) => void>();
    render(<InstructionBox state={readyState()} onSend={onSend} />);
    await user.type(screen.getByRole("combobox", { name: "지시 입력" }), "   ");
    await user.click(screen.getByRole("button", { name: "전송" }));
    expect(onSend).not.toHaveBeenCalled();
  });
  it("sends a mention-only request with the opaque backend snapshot", async () => {
    const user = userEvent.setup();
    const onSend = vi.fn<(p: ChatPayload) => void>();
    const mention = {
      resourceKey: "map.region:region-a",
      kind: "map.region" as const,
      label: "영역 A",
      detail: "저장된 영역 · 사각형",
      mention: {
        kind: "map.region" as const,
        version: 1 as const,
        projectId: "project-a",
        sourceFileSha256: "a".repeat(64),
        mapWidth: 64,
        mapHeight: 64,
        selectionId: "region-a",
        selectionSnapshotHash: "b".repeat(64),
      },
    };
    render(
      <InstructionBox
        state={readyState()}
        onSend={onSend}
        onMentionSearch={async () => ({
          schema: "eud-mention-search/1",
          results: [mention],
          truncated: false,
        })}
      />,
    );

    await user.type(screen.getByRole("combobox", { name: "지시 입력" }), "@");
    await user.click(await screen.findByRole("option", { name: /@영역 A/ }));
    await user.click(screen.getByRole("button", { name: "전송" }));

    expect(onSend).toHaveBeenCalledWith({
      text: "",
      attachments: [],
      mentions: [
        expect.objectContaining({
          label: "영역 A",
          mention: mention.mention,
        }),
      ],
    });
  });
});


describe("InstructionBox — attachments", () => {
  const imageAttachment = {
    id: "image-1",
    name: "screenshot.png",
    mime: "image/png",
    kind: "image" as const,
    size: 4,
    previewUrl: "data:image/png;base64,iVBORw0KGgo=",
  };
  const audioAttachment = {
    id: "audio-1",
    name: "battle-theme.flac",
    mime: "audio/flac",
    kind: "audio" as const,
    size: 4 * 1024 * 1024,
  };

  it("stages a picked image, renders a removable chip, and sends it with the text", async () => {
    const user = userEvent.setup();
    const onSend = vi.fn<(p: ChatPayload) => void>();
    const onStageAttachment = vi.fn().mockResolvedValue(imageAttachment);
    const onDiscardAttachment = vi.fn().mockResolvedValue(undefined);
    render(
      <InstructionBox
        state={readyState()}
        onSend={onSend}
        onStageAttachment={onStageAttachment}
        onDiscardAttachment={onDiscardAttachment}
      />,
    );

    const file = new File(["png!"], "screenshot.png", { type: "image/png" });
    await user.upload(screen.getByLabelText("파일 첨부"), file);

    expect(onStageAttachment).toHaveBeenCalledWith(file);
    expect(screen.getByText("screenshot.png")).toBeInTheDocument();

    await user.type(screen.getByRole("combobox", { name: "지시 입력" }), "이 화면을 봐줘");
    await user.click(screen.getByRole("button", { name: "전송" }));
    expect(onSend).toHaveBeenCalledWith({
      text: "이 화면을 봐줘",
      attachments: [imageAttachment],
      mentions: [],
    });
  });

  it("allows an attachment-only message", async () => {
    const user = userEvent.setup();
    const onSend = vi.fn<(p: ChatPayload) => void>();
    render(
      <InstructionBox
        state={readyState()}
        onSend={onSend}
        onStageAttachment={vi.fn().mockResolvedValue(imageAttachment)}
      />,
    );

    await user.upload(
      screen.getByLabelText("파일 첨부"),
      new File(["png!"], "screenshot.png", { type: "image/png" }),
    );
    await user.click(screen.getByRole("button", { name: "전송" }));

    expect(onSend).toHaveBeenCalledWith({
      text: "",
      attachments: [imageAttachment],
      mentions: [],
    });
  });

  it("accepts and sends an attachment-only audio file without an image preview", async () => {
    const user = userEvent.setup();
    const onSend = vi.fn<(payload: ChatPayload) => void>();
    render(
      <InstructionBox
        state={readyState()}
        onSend={onSend}
        onStageAttachment={vi.fn().mockResolvedValue(audioAttachment)}
      />,
    );
    const picker = screen.getByLabelText("파일 첨부");
    expect(picker).toHaveAttribute("accept", expect.stringContaining("audio/*"));
    await user.upload(
      picker,
      new File(["fLaC"], "battle-theme.flac", { type: "audio/flac" }),
    );
    expect(screen.getByText("battle-theme.flac")).toBeInTheDocument();
    expect(screen.queryByRole("img")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "전송" }));
    expect(onSend).toHaveBeenCalledWith({
      text: "",
      attachments: [audioAttachment],
      mentions: [],
    });
  });

  it("rejects staged audio above the 128 MiB turn aggregate and discards it", async () => {
    const user = userEvent.setup();
    const onDiscardAttachment = vi.fn().mockResolvedValue(undefined);
    const staged = [1, 2, 3].map((index) => ({
      ...audioAttachment,
      id: `audio-${index}`,
      name: `audio-${index}.wav`,
      size: 64 * 1024 * 1024,
    }));
    const onStageAttachment = vi
      .fn()
      .mockResolvedValueOnce(staged[0])
      .mockResolvedValueOnce(staged[1])
      .mockResolvedValueOnce(staged[2]);
    render(
      <InstructionBox
        state={readyState()}
        onSend={noop}
        onStageAttachment={onStageAttachment}
        onDiscardAttachment={onDiscardAttachment}
      />,
    );
    await user.upload(screen.getByLabelText("파일 첨부"), [
      new File(["a"], "audio-1.wav", { type: "audio/wav" }),
      new File(["b"], "audio-2.wav", { type: "audio/wav" }),
      new File(["c"], "audio-3.wav", { type: "audio/wav" }),
    ]);
    expect(await screen.findByRole("alert")).toHaveTextContent("합계 128MB");
    expect(onDiscardAttachment).toHaveBeenCalledWith("audio-3");
    expect(screen.getByText("audio-1.wav")).toBeInTheDocument();
    expect(screen.getByText("audio-2.wav")).toBeInTheDocument();
    expect(screen.queryByText("audio-3.wav")).not.toBeInTheDocument();
  });

  it("accepts dropped files and pasted clipboard images", async () => {
    const stagedText = {
      id: "text-1",
      name: "notes.txt",
      mime: "text/plain",
      kind: "text" as const,
      size: 5,
    };
    const onStageAttachment = vi
      .fn()
      .mockResolvedValueOnce(stagedText)
      .mockResolvedValueOnce(imageAttachment);
    render(
      <InstructionBox
        state={readyState()}
        onSend={noop}
        onStageAttachment={onStageAttachment}
      />,
    );

    const dropFile = new File(["notes"], "notes.txt", { type: "text/plain" });
    fireEvent.drop(screen.getByTestId("prompt-drop-zone"), {
      dataTransfer: { files: [dropFile] },
    });
    await screen.findByText("notes.txt");

    const pastedImage = new File(["png!"], "clipboard.png", {
      type: "image/png",
    });
    fireEvent.paste(screen.getByRole("combobox", { name: "지시 입력" }), {
      clipboardData: { files: [pastedImage] },
    });
    await screen.findByText("screenshot.png");

    expect(onStageAttachment).toHaveBeenNthCalledWith(1, dropFile);
    expect(onStageAttachment).toHaveBeenNthCalledWith(2, pastedImage);
  });

  it("removes a staged attachment and deletes its draft", async () => {
    const user = userEvent.setup();
    const onDiscardAttachment = vi.fn().mockResolvedValue(undefined);
    render(
      <InstructionBox
        state={readyState()}
        onSend={noop}
        onStageAttachment={vi.fn().mockResolvedValue(imageAttachment)}
        onDiscardAttachment={onDiscardAttachment}
      />,
    );
    await user.upload(
      screen.getByLabelText("파일 첨부"),
      new File(["png!"], "screenshot.png", { type: "image/png" }),
    );

    await user.click(
      screen.getByRole("button", { name: "screenshot.png 첨부 제거" }),
    );

    expect(onDiscardAttachment).toHaveBeenCalledWith("image-1");
    expect(screen.queryByText("screenshot.png")).not.toBeInTheDocument();
  });
});

describe("InstructionBox — Codex model settings", () => {
  it("renders the current model and reasoning effort inline", () => {
    render(
      <InstructionBox
        state={readyState()}
        onSend={noop}
        codexSettings={codexSettings}
        onCodexSettingsChange={noop}
      />,
    );

    expect(
      screen.getByRole("combobox", { name: "Codex 모델" }),
    ).toHaveTextContent("GPT Default");
    expect(
      screen.getByRole("combobox", { name: "추론 단계" }),
    ).toHaveTextContent("추론 보통");
  });

  it("changes models with that model's default reasoning effort", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(
      <InstructionBox
        state={readyState()}
        onSend={noop}
        codexSettings={codexSettings}
        onCodexSettingsChange={onChange}
      />,
    );

    await user.click(screen.getByRole("combobox", { name: "Codex 모델" }));
    await user.click(screen.getByRole("option", { name: "GPT Fast" }));
    expect(onChange).toHaveBeenCalledWith("gpt-fast", "low");
  });

  it("changes reasoning effort without changing the selected model", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(
      <InstructionBox
        state={readyState()}
        onSend={noop}
        codexSettings={codexSettings}
        onCodexSettingsChange={onChange}
      />,
    );

    await user.click(screen.getByRole("combobox", { name: "추론 단계" }));
    await user.click(screen.getByRole("option", { name: "추론 높음" }));
    expect(onChange).toHaveBeenCalledWith("gpt-default", "high");
  });

  it("offers a retry control when the catalog could not be loaded", async () => {
    const user = userEvent.setup();
    const onReload = vi.fn();
    render(
      <InstructionBox
        state={readyState()}
        onSend={noop}
        codexSettings={null}
        onCodexSettingsReload={onReload}
      />,
    );

    await user.click(
      screen.getByRole("button", { name: "Codex 모델 다시 불러오기" }),
    );
    expect(onReload).toHaveBeenCalledTimes(1);
  });

  it("locks both selectors while a settings save is in flight", () => {
    render(
      <InstructionBox
        state={readyState()}
        onSend={noop}
        codexSettings={codexSettings}
        codexSettingsBusy
        onCodexSettingsChange={noop}
      />,
    );

    expect(screen.getByRole("combobox", { name: "Codex 모델" })).toBeDisabled();
    expect(screen.getByRole("combobox", { name: "추론 단계" })).toBeDisabled();
  });
});

describe("InstructionBox — session context usage", () => {
  it("shows current context and cumulative usage through the Context hover card", async () => {
    const user = userEvent.setup();
    const store = createPanelStore();
    store.wsOpen();
    store.applyList({ files: [] });
    store.contextUsageReceived({
      last: {
        inputTokens: 31_000,
        cachedInputTokens: 24_000,
        cacheWriteInputTokens: 0,
        outputTokens: 1_200,
        reasoningOutputTokens: 800,
        totalTokens: 32_200,
      },
      total: {
        inputTokens: 52_000,
        cachedInputTokens: 40_000,
        cacheWriteInputTokens: 600,
        outputTokens: 2_100,
        reasoningOutputTokens: 1_300,
        totalTokens: 54_100,
      },
      modelContextWindow: 128_000,
    });

    render(<InstructionBox state={store.getState()} onSend={noop} />);
    const trigger = screen.getByRole("button", {
      name: /컨텍스트 .* 사용/,
    });
    await user.hover(trigger);

    expect(await screen.findByText("현재 컨텍스트")).toBeInTheDocument();
    expect(screen.getByText("세션 누적")).toBeInTheDocument();
    expect(screen.getByText("캐시 입력")).toBeInTheDocument();
    expect(screen.queryByText(/Total cost/i)).not.toBeInTheDocument();
  });

  it("does not guess a percentage without a model context window", () => {
    const store = createPanelStore();
    store.contextUsageReceived({
      last: {
        inputTokens: 1,
        cachedInputTokens: 0,
        cacheWriteInputTokens: 0,
        outputTokens: 0,
        reasoningOutputTokens: 0,
        totalTokens: 1,
      },
      total: {
        inputTokens: 1,
        cachedInputTokens: 0,
        cacheWriteInputTokens: 0,
        outputTokens: 0,
        reasoningOutputTokens: 0,
        totalTokens: 1,
      },
      modelContextWindow: null,
    });

    render(<InstructionBox state={store.getState()} onSend={noop} />);

    expect(
      screen.queryByRole("button", { name: /컨텍스트 .* 사용/ }),
    ).not.toBeInTheDocument();
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
      <InstructionBox state={readyState()} onSend={noop} />,
    );
    const group = container.querySelector('[data-slot="input-group"]');
    expect(group).not.toBeNull();
    // Iterate direct children instead of `:scope >` (jsdom's selector engine
    // does not reliably support `:scope` with attribute compounds).
    const directAddon = Array.from(group!.children).find(
      (c) =>
        c.getAttribute("data-slot") === "input-group-addon" &&
        c.getAttribute("data-align") === "block-end",
    );
    expect(directAddon).not.toBeUndefined();
  });
});

describe("InstructionBox — RAG warmup gate", () => {
  // While the RAG model loads the store gates canSend off; the box must also
  // disable the TEXTAREA (no typing before the model is ready — user decision)
  // and explain why via the placeholder.
  function loadingState(): PanelState {
    const store = createPanelStore();
    store.wsOpen();
    store.applyList({
      files: [{ path: "main.eps", ftype: "CUIEps", settable: true }],
    });
    store.ragWarmupChanged("loading");
    return store.getState();
  }

  it("disables Send + the textarea with a guide placeholder while loading", () => {
    render(<InstructionBox state={loadingState()} onSend={noop} />);
    expect(screen.getByRole("button", { name: "전송" })).toBeDisabled();
    const textarea = screen.getByRole("combobox", { name: "지시 입력" });
    expect(textarea).toBeDisabled();
    expect(textarea).toHaveAttribute(
      "placeholder",
      expect.stringContaining("RAG 모델 준비 중"),
    );
  });

  it("re-enables once warmup completes", () => {
    const store = createPanelStore();
    store.wsOpen();
    store.applyList({
      files: [{ path: "main.eps", ftype: "CUIEps", settable: true }],
    });
    store.ragWarmupChanged("loading");
    store.ragWarmupChanged("ready");
    render(<InstructionBox state={store.getState()} onSend={noop} />);
    expect(screen.getByRole("button", { name: "전송" })).toBeEnabled();
    expect(screen.getByRole("combobox", { name: "지시 입력" })).toBeEnabled();
  });
});
