import { Check, ChevronLeft, ChevronRight, CircleHelp, LoaderCircle, Send } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";

import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import type { AskAnswer, AskQuestion } from "@/lib/ipc";
import { cn } from "@/lib/utils";

export interface AskCardProps {
  requestId: string;
  questions: AskQuestion[];
  submitting: boolean;
  onSubmit(answers: Record<string, AskAnswer>): void;
}

export function AskCard({
  requestId,
  questions,
  submitting,
  onSubmit,
}: AskCardProps) {
  const cardRef = useRef<HTMLElement>(null);
  const tabRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const [selected, setSelected] = useState<Record<string, string[]>>({});
  const [custom, setCustom] = useState<Record<string, string>>({});
  const [activeQuestionIndex, setActiveQuestionIndex] = useState(0);

  useEffect(() => {
    setSelected({});
    setCustom({});
    setActiveQuestionIndex(0);
    cardRef.current?.focus();
  }, [requestId]);

  const answeredCount = useMemo(
    () =>
      questions.filter(
        (question) =>
          (selected[question.id]?.length ?? 0) > 0 ||
          (custom[question.id]?.trim().length ?? 0) > 0,
      ).length,
    [custom, questions, selected],
  );
  const complete = questions.length > 0 && answeredCount === questions.length;
  const activeQuestion = questions[activeQuestionIndex] ?? questions[0];
  const activeQuestionAnswered = activeQuestion
    ? (selected[activeQuestion.id]?.length ?? 0) > 0 ||
      (custom[activeQuestion.id]?.trim().length ?? 0) > 0
    : false;

  const toggleOption = (question: AskQuestion, label: string) => {
    if (submitting) return;
    setSelected((current) => {
      const values = current[question.id] ?? [];
      const next = question.multi
        ? values.includes(label)
          ? values.filter((value) => value !== label)
          : [...values, label]
        : [label];
      return { ...current, [question.id]: next };
    });
    if (!question.multi) {
      setCustom((current) => ({ ...current, [question.id]: "" }));
    }
  };

  const updateCustom = (question: AskQuestion, value: string) => {
    setCustom((current) => ({ ...current, [question.id]: value }));
    if (!question.multi && value.length > 0) {
      setSelected((current) => ({ ...current, [question.id]: [] }));
    }
  };

  const goToQuestion = (index: number, moveFocus = false) => {
    if (submitting || index < 0 || index >= questions.length) return;
    setActiveQuestionIndex(index);
    if (moveFocus) {
      tabRefs.current[index]?.focus();
    }
  };

  const handleTabKeyDown = (
    event: React.KeyboardEvent<HTMLButtonElement>,
    questionIndex: number,
  ) => {
    let nextIndex: number;
    switch (event.key) {
      case "ArrowLeft":
        nextIndex = (questionIndex - 1 + questions.length) % questions.length;
        break;
      case "ArrowRight":
        nextIndex = (questionIndex + 1) % questions.length;
        break;
      case "Home":
        nextIndex = 0;
        break;
      case "End":
        nextIndex = questions.length - 1;
        break;
      default:
        return;
    }
    event.preventDefault();
    goToQuestion(nextIndex, true);
  };

  const handleSubmit = (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!complete || submitting) return;
    const answers = Object.fromEntries(
      questions.map((question) => {
        const direct = custom[question.id]?.trim();
        const values = question.multi
          ? [...(selected[question.id] ?? []), ...(direct ? [direct] : [])]
          : direct
            ? [direct]
            : (selected[question.id] ?? []).slice(0, 1);
        return [question.id, { answers: values }];
      }),
    );
    onSubmit(answers);
  };

  return (
    <section
      ref={cardRef}
      tabIndex={-1}
      aria-label="AI 질문"
      className="shrink-0 border-t border-border bg-card/35 px-4 py-3 outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
    >
      <form
        onSubmit={handleSubmit}
        className="mx-auto flex w-full max-w-3xl flex-col gap-4 rounded-xl border border-primary/25 bg-background/80 p-4 shadow-sm"
      >
        <header className="flex items-start gap-3">
          <span className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary">
            <CircleHelp className="size-5" aria-hidden="true" />
          </span>
          <div className="min-w-0 flex-1">
            <h2 className="text-sm font-semibold text-foreground">확인이 필요합니다</h2>
            <p className="mt-0.5 text-xs leading-5 text-muted-foreground">
              답변하면 AI가 같은 작업을 이어서 진행합니다.
            </p>
          </div>
          <span className="rounded-full bg-muted px-2 py-1 text-[11px] tabular-nums text-muted-foreground">
            {questions.length}개 질문
          </span>
        </header>

        {questions.length > 1 && (
          <div
            role="tablist"
            aria-label="질문 목록"
            aria-orientation="horizontal"
            className="grid grid-flow-col auto-cols-fr gap-1 rounded-lg bg-muted/60 p-1"
          >
            {questions.map((question, questionIndex) => {
              const answered =
                (selected[question.id]?.length ?? 0) > 0 ||
                (custom[question.id]?.trim().length ?? 0) > 0;
              const active = questionIndex === activeQuestionIndex;
              const tabLabel = question.header?.trim() || `질문 ${questionIndex + 1}`;
              const tabId = `${requestId}-${question.id}-tab`;
              const panelId = `${requestId}-${question.id}-panel`;
              return (
                <button
                  key={question.id}
                  ref={(element) => {
                    tabRefs.current[questionIndex] = element;
                  }}
                  id={tabId}
                  type="button"
                  role="tab"
                  aria-controls={panelId}
                  aria-selected={active}
                  aria-label={`${tabLabel}, ${questionIndex + 1}번 질문, ${
                    answered ? "답변 완료" : "답변 필요"
                  }`}
                  tabIndex={active ? 0 : -1}
                  disabled={submitting}
                  onClick={() => goToQuestion(questionIndex)}
                  onKeyDown={(event) => handleTabKeyDown(event, questionIndex)}
                  className={cn(
                    "flex min-h-11 min-w-0 items-center justify-center gap-1.5 rounded-md px-2 text-xs font-medium outline-none transition-colors",
                    "focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1",
                    "disabled:cursor-not-allowed disabled:opacity-50",
                    active
                      ? "bg-background text-foreground shadow-sm"
                      : "text-muted-foreground hover:bg-background/65 hover:text-foreground",
                  )}
                >
                  <span
                    className={cn(
                      "flex size-5 shrink-0 items-center justify-center rounded-full text-[11px] tabular-nums",
                      answered
                        ? "bg-primary/15 text-primary"
                        : "bg-background/70 text-muted-foreground",
                    )}
                    aria-hidden="true"
                  >
                    {answered ? <Check className="size-3.5" /> : questionIndex + 1}
                  </span>
                  <span className="truncate">{tabLabel}</span>
                </button>
              );
            })}
          </div>
        )}

        {activeQuestion && (
          <div
            id={`${requestId}-${activeQuestion.id}-panel`}
            role={questions.length > 1 ? "tabpanel" : undefined}
            aria-labelledby={
              questions.length > 1 ? `${requestId}-${activeQuestion.id}-tab` : undefined
            }
            className="min-w-0"
          >
            <fieldset className="min-w-0 space-y-2.5">
              <legend className="w-full text-sm font-medium leading-6 text-foreground">
                <span className="mr-2 text-xs tabular-nums text-muted-foreground">
                  {activeQuestionIndex + 1}.
                </span>
                {activeQuestion.header && (
                  <span className="mr-2 rounded bg-muted px-1.5 py-0.5 text-[11px] font-medium text-muted-foreground">
                    {activeQuestion.header}
                  </span>
                )}
                {activeQuestion.question}
              </legend>

              {(activeQuestion.options?.length ?? 0) > 0 && (
                <div className="grid gap-2 sm:grid-cols-2">
                  {activeQuestion.options?.map((option, optionIndex) => {
                    const inputId = `${requestId}-${activeQuestion.id}-${optionIndex}`;
                    const checked = (selected[activeQuestion.id] ?? []).includes(option.label);
                    return (
                      <div key={option.label} className="relative min-w-0">
                        <input
                          id={inputId}
                          type={activeQuestion.multi ? "checkbox" : "radio"}
                          name={`ask-${requestId}-${activeQuestion.id}`}
                          checked={checked}
                          disabled={submitting}
                          onChange={() => toggleOption(activeQuestion, option.label)}
                          className="peer sr-only"
                        />
                        <label
                          htmlFor={inputId}
                          className={cn(
                            "flex min-h-11 cursor-pointer flex-col justify-center rounded-lg border border-border bg-card/45 px-3 py-2 text-left transition-colors",
                            "hover:border-primary/45 hover:bg-primary/5 peer-focus-visible:ring-2 peer-focus-visible:ring-ring peer-focus-visible:ring-offset-2",
                            "peer-disabled:cursor-not-allowed peer-disabled:opacity-50",
                            checked && "border-primary bg-primary/10",
                          )}
                        >
                          <span className="text-sm font-medium text-foreground">
                            {option.label}
                          </span>
                          {option.description && (
                            <span className="mt-0.5 text-xs leading-5 text-muted-foreground">
                              {option.description}
                            </span>
                          )}
                        </label>
                      </div>
                    );
                  })}
                </div>
              )}

              <label className="block">
                <span className="mb-1.5 block text-xs font-medium text-muted-foreground">
                  {(activeQuestion.options?.length ?? 0) > 0 ? "기타 입력" : "답변"}
                </span>
                <Textarea
                  value={custom[activeQuestion.id] ?? ""}
                  disabled={submitting}
                  onChange={(event) => updateCustom(activeQuestion, event.target.value)}
                  rows={2}
                  placeholder={
                    (activeQuestion.options?.length ?? 0) > 0
                      ? activeQuestion.multi
                        ? "선택 항목과 함께 전달할 내용을 입력하세요."
                        : "선택지 대신 직접 입력할 수 있습니다."
                      : "답변을 입력하세요."
                  }
                  className="min-h-20 resize-y bg-background"
                />
              </label>
            </fieldset>
          </div>
        )}

        <div className="flex flex-wrap items-center justify-between gap-3 border-t border-border pt-3">
          <div className="flex min-w-0 items-center gap-2">
            {questions.length > 1 && (
              <Button
                type="button"
                variant="outline"
                className="h-11 gap-1.5"
                disabled={activeQuestionIndex === 0 || submitting}
                onClick={() => goToQuestion(activeQuestionIndex - 1)}
              >
                <ChevronLeft className="size-4" aria-hidden="true" />
                이전
              </Button>
            )}
            <p className="text-xs text-muted-foreground" aria-live="polite">
              {complete
                ? "모든 질문에 답했습니다."
                : questions.length > 1
                  ? `${answeredCount}/${questions.length} 답변 완료`
                  : "답변해 주세요."}
            </p>
          </div>
          {questions.length > 1 && activeQuestionIndex < questions.length - 1 ? (
            <Button
              type="button"
              className="h-11 gap-1.5"
              disabled={!activeQuestionAnswered || submitting}
              onClick={() => goToQuestion(activeQuestionIndex + 1)}
            >
              다음 질문
              <ChevronRight className="size-4" aria-hidden="true" />
            </Button>
          ) : (
            <Button type="submit" className="h-11 gap-2" disabled={!complete || submitting}>
              {submitting ? (
                <LoaderCircle
                  className="size-4 animate-spin motion-reduce:animate-none"
                  aria-hidden="true"
                />
              ) : (
                <Send className="size-4" aria-hidden="true" />
              )}
              {submitting ? "전달 중" : "답변 전달"}
            </Button>
          )}
        </div>
      </form>
    </section>
  );
}
