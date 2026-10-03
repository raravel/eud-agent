export interface AskDraft {
  readonly selected: Readonly<Record<string, readonly string[]>>;
  readonly custom: Readonly<Record<string, string>>;
  readonly activeQuestionIndex: number;
  readonly collapsed: boolean;
}

export const EMPTY_ASK_DRAFT: AskDraft = {
  selected: {},
  custom: {},
  activeQuestionIndex: 0,
  collapsed: false,
};
