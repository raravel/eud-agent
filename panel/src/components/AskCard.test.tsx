import { act, fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { AskCard } from "@/components/AskCard";

const questions = [
  {
    id: "mode",
    header: "방식",
    question: "어떤 방식을 사용할까요?",
    multi: false,
    options: [
      { label: "빠르게", description: "기본 설정을 사용합니다." },
      { label: "세밀하게", description: "세부 설정을 확인합니다." },
    ],
  },
  {
    id: "features",
    question: "필요한 항목을 고르세요.",
    multi: true,
    options: [{ label: "로그" }, { label: "알림" }],
  },
];

describe("AskCard", () => {
  it("shows the remaining answer time and the text fallback when a wait is bounded", () => {
    vi.useFakeTimers();
    try {
      vi.setSystemTime(new Date("2026-09-18T00:00:00Z"));
      render(
        <AskCard
          requestId="ask-timed"
          questions={questions}
          submitting={false}
          waitSeconds={240}
          receivedAt={Date.now() - 30_000}
          onSubmit={vi.fn()}
        />,
      );
      expect(screen.getByText("남은 시간 3:30")).toBeInTheDocument();
      expect(screen.getByText(/다음 메시지로 답할 수 있습니다/)).toBeInTheDocument();
      act(() => {
        vi.advanceTimersByTime(60_000);
      });
      expect(screen.getByText("남은 시간 2:30")).toBeInTheDocument();
      act(() => {
        vi.advanceTimersByTime(200_000);
      });
      expect(screen.getByText("시간 초과")).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it("says an unanswered browser confirmation is refused and takes no free text", () => {
    const onSubmit = vi.fn();
    render(
      <AskCard
        requestId="ask-browser"
        kind="browser_confirm"
        questions={[
          {
            id: "browser_confirm",
            header: "브라우저 확인",
            question: '에이전트가 "결제하기" 를 누르려고 합니다.',
            multi: false,
            options: [
              { label: "허용", description: "이번 동작을 진행합니다." },
              { label: "거부", description: "진행하지 않고 에이전트에게 알립니다." },
            ],
          },
        ]}
        submitting={false}
        waitSeconds={200}
        receivedAt={Date.now()}
        onSubmit={onSubmit}
      />,
    );
    expect(screen.getByText(/시간 안에 답하지 않으면 거부로 처리됩니다/)).toBeInTheDocument();
    expect(screen.queryByText(/다음 메시지로 답할 수 있습니다/)).not.toBeInTheDocument();
    expect(screen.queryByLabelText("기타 입력")).not.toBeInTheDocument();
    expect(screen.getAllByRole("radio")).toHaveLength(2);

    fireEvent.click(screen.getByLabelText(/거부/));
    expect(screen.getByLabelText(/거부/)).toBeChecked();
    fireEvent.click(screen.getByRole("button", { name: "답변 전달" }));
    expect(onSubmit).toHaveBeenCalledWith({ browser_confirm: { answers: ["거부"] } });
  });

  it("asks before an agent-instruction write like a browser confirmation", () => {
    const onSubmit = vi.fn();
    render(
      <AskCard
        requestId="ask-instruction"
        kind="instruction_write"
        questions={[
          {
            id: "instruction_write",
            header: "지시 파일 수정 확인",
            question: "에이전트가 에이전트 지시 파일을 쓰려고 합니다(새 파일 만들기). 파일: `AGENTS.md`",
            multi: false,
            preview: "내용 (총 12바이트)\n# 빌드 규칙",
            options: [
              { label: "허용", description: "이 파일을 씁니다." },
              { label: "거부", description: "쓰지 않고 에이전트에게 알립니다." },
            ],
          },
        ]}
        submitting={false}
        waitSeconds={200}
        receivedAt={Date.now()}
        onSubmit={onSubmit}
      />,
    );
    expect(screen.getByText(/허용하면 에이전트가 이 파일을 씁니다/)).toBeInTheDocument();
    // The card shows what would be written, as text.
    expect(screen.getByText("쓰려는 내용")).toBeInTheDocument();
    expect(screen.getByText(/# 빌드 규칙/)).toBeInTheDocument();
    expect(screen.getByText(/총 12바이트/)).toBeInTheDocument();
    expect(screen.getByText(/시간 안에 답하지 않으면 거부로 처리됩니다/)).toBeInTheDocument();
    expect(screen.queryByLabelText("기타 입력")).not.toBeInTheDocument();
    expect(screen.getAllByRole("radio")).toHaveLength(2);
    fireEvent.click(screen.getByLabelText(/허용/));
    fireEvent.click(screen.getByRole("button", { name: "답변 전달" }));
    expect(onSubmit).toHaveBeenCalledWith({ instruction_write: { answers: ["허용"] } });
  });

  it("answers one tab at a time and preserves answers while revisiting earlier questions", () => {
    const onSubmit = vi.fn();
    render(
      <AskCard
        requestId="ask-1"
        questions={questions}
        submitting={false}
        onSubmit={onSubmit}
      />,
    );

    expect(screen.getByRole("tablist", { name: "질문 목록" })).toBeInTheDocument();
    expect(screen.getByText("어떤 방식을 사용할까요?")).toBeInTheDocument();
    expect(screen.queryByText("필요한 항목을 고르세요.")).not.toBeInTheDocument();
    fireEvent.keyDown(screen.getByRole("tab", { name: /방식/ }), {
      key: "ArrowRight",
    });
    expect(screen.getByText("필요한 항목을 고르세요.")).toBeInTheDocument();
    fireEvent.keyDown(screen.getByRole("tab", { name: /질문 2/ }), {
      key: "Home",
    });
    expect(screen.getByText("어떤 방식을 사용할까요?")).toBeInTheDocument();

    const next = screen.getByRole("button", { name: "다음 질문" });
    expect(next).toBeDisabled();
    fireEvent.click(screen.getByLabelText(/빠르게/));
    expect(next).toBeEnabled();
    fireEvent.click(next);

    expect(screen.getByText("필요한 항목을 고르세요.")).toBeInTheDocument();
    fireEvent.click(screen.getByLabelText("로그"));
    fireEvent.change(screen.getByLabelText("기타 입력"), {
      target: { value: "진행률" },
    });

    fireEvent.click(screen.getByRole("tab", { name: /방식/ }));
    expect(screen.getByLabelText(/빠르게/)).toBeChecked();
    fireEvent.click(screen.getByLabelText(/세밀하게/));

    fireEvent.click(screen.getByRole("tab", { name: /질문 2/ }));
    expect(screen.getByLabelText("로그")).toBeChecked();
    expect(screen.getByLabelText("기타 입력")).toHaveValue("진행률");

    const submit = screen.getByRole("button", { name: "답변 전달" });
    expect(submit).toBeEnabled();
    fireEvent.click(submit);

    expect(onSubmit).toHaveBeenCalledWith({
      mode: { answers: ["세밀하게"] },
      features: { answers: ["로그", "진행률"] },
    });
  });

  it("uses direct input instead of a selected option for a single-choice question", () => {
    const onSubmit = vi.fn();
    render(
      <AskCard
        requestId="ask-2"
        questions={[questions[0]]}
        submitting={false}
        onSubmit={onSubmit}
      />,
    );

    const option = screen.getByLabelText(/세밀하게/);
    fireEvent.click(option);
    expect(option).toBeChecked();

    fireEvent.change(screen.getByLabelText("기타 입력"), {
      target: { value: "설정을 직접 지정" },
    });
    expect(option).not.toBeChecked();
    fireEvent.click(screen.getByRole("button", { name: "답변 전달" }));

    expect(onSubmit).toHaveBeenCalledWith({
      mode: { answers: ["설정을 직접 지정"] },
    });
  });

  it("collapses to its header and keeps the draft answer when expanded again", () => {
    const onSubmit = vi.fn();
    render(
      <AskCard
        requestId="ask-3"
        questions={[questions[0]]}
        submitting={false}
        onSubmit={onSubmit}
      />,
    );

    fireEvent.change(screen.getByLabelText("기타 입력"), {
      target: { value: "직접 답변" },
    });
    const collapse = screen.getByRole("button", { name: "질문 접기" });
    expect(collapse).toHaveAttribute("aria-expanded", "true");
    fireEvent.click(collapse);

    expect(screen.queryByLabelText("기타 입력")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "답변 전달" })).not.toBeInTheDocument();
    expect(screen.getByText("어떤 방식을 사용할까요?")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "질문 펼치기" }));
    expect(screen.getByLabelText("기타 입력")).toHaveValue("직접 답변");
    fireEvent.click(screen.getByRole("button", { name: "답변 전달" }));
    expect(onSubmit).toHaveBeenCalledWith({ mode: { answers: ["직접 답변"] } });
  });
});
