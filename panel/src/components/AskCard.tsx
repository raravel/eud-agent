import { CircleHelp, LoaderCircle, Send } from "lucide-react";
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
  const [selected, setSelected] = useState<Record<string, string[]>>({});
  const [custom, setCustom] = useState<Record<string, string>>({});

  useEffect(() => {
    setSelected({});
    setCustom({});
    cardRef.current?.focus();
  }, [requestId]);

  const complete = useMemo(
    () =>
      questions.every(
        (question) =>
          (selected[question.id]?.length ?? 0) > 0 ||
          (custom[question.id]?.trim().length ?? 0) > 0,
      ),
    [custom, questions, selected],
  );

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

        <div className="grid gap-4">
          {questions.map((question, questionIndex) => {
            const values = selected[question.id] ?? [];
            const options = question.options ?? [];
            return (
              <fieldset key={question.id} className="min-w-0 space-y-2.5">
                <legend className="w-full text-sm font-medium leading-6 text-foreground">
                  <span className="mr-2 text-xs tabular-nums text-muted-foreground">
                    {questionIndex + 1}.
                  </span>
                  {question.header && (
                    <span className="mr-2 rounded bg-muted px-1.5 py-0.5 text-[11px] font-medium text-muted-foreground">
                      {question.header}
                    </span>
                  )}
                  {question.question}
                </legend>

                {options.length > 0 && (
                  <div className="grid gap-2 sm:grid-cols-2">
                    {options.map((option, optionIndex) => {
                      const inputId = `${requestId}-${question.id}-${optionIndex}`;
                      const checked = values.includes(option.label);
                      return (
                        <div key={option.label} className="relative min-w-0">
                          <input
                            id={inputId}
                            type={question.multi ? "checkbox" : "radio"}
                            name={`ask-${requestId}-${question.id}`}
                            checked={checked}
                            disabled={submitting}
                            onChange={() => toggleOption(question, option.label)}
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
                    {options.length > 0 ? "기타 입력" : "답변"}
                  </span>
                  <Textarea
                    value={custom[question.id] ?? ""}
                    disabled={submitting}
                    onChange={(event) => updateCustom(question, event.target.value)}
                    rows={2}
                    placeholder={
                      options.length > 0
                        ? question.multi
                          ? "선택 항목과 함께 전달할 내용을 입력하세요."
                          : "선택지 대신 직접 입력할 수 있습니다."
                        : "답변을 입력하세요."
                    }
                    className="min-h-20 resize-y bg-background"
                  />
                </label>
              </fieldset>
            );
          })}
        </div>

        <div className="flex items-center justify-between gap-3 border-t border-border pt-3">
          <p className="text-xs text-muted-foreground" aria-live="polite">
            {complete ? "모든 질문에 답했습니다." : "모든 질문에 답해 주세요."}
          </p>
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
        </div>
      </form>
    </section>
  );
}
