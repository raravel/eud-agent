import { fireEvent, render, screen } from "@testing-library/react";
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

    const option = screen.getByLabelText(/세밀하게/) as HTMLInputElement;
    fireEvent.click(option);
    expect(option.checked).toBe(true);

    fireEvent.change(screen.getByLabelText("기타 입력"), {
      target: { value: "설정을 직접 지정" },
    });
    expect(option.checked).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "답변 전달" }));

    expect(onSubmit).toHaveBeenCalledWith({
      mode: { answers: ["설정을 직접 지정"] },
    });
  });
});
