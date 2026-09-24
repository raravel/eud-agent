/**
 * Project history view — what replaced request changeset review.
 *
 * Contract (`@/components/GitHistoryView`):
 *   export interface GitHistoryViewProps {
 *     revision: number;                       // App's cue to re-read the log
 *     open: boolean;
 *     onOpenChange(open: boolean): void;
 *     loadLog(): Promise<CommitSummary[]>;     // git_log
 *     loadDetail(sha: string): Promise<CommitDetail>;  // git_commit_detail
 *     revert(sha: string): Promise<CommitRecord>;      // git_revert
 *   }
 *
 * The view owns loading, selection and the revert confirmation; the App only
 * supplies the three commands.
 */
import { describe, it, expect, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { GitHistoryView } from "@/components/GitHistoryView";
import type { CommitDetail, CommitSummary } from "@/lib/ipc";

const commits: CommitSummary[] = [
  { sha: "a".repeat(40), subject: "마린 체력 조정", timestamp: 1_700_000_000 },
  { sha: "b".repeat(40), subject: "시작 지점 추가", timestamp: 1_699_000_000 },
];

const detail: CommitDetail = {
  sha: "a".repeat(40),
  subject: "마린 체력 조정",
  body: "session: s-1\nrequest: r-1",
  timestamp: 1_700_000_000,
  files: [
    {
      path: "src/main.eps",
      insertions: 1,
      deletions: 1,
      binary: false,
      patch: [
        "diff --git a/src/main.eps b/src/main.eps",
        "--- a/src/main.eps",
        "+++ b/src/main.eps",
        "@@ -1,2 +1,2 @@",
        " const keep = 1;",
        "-const old = 2;",
        "+const next = 3;",
      ].join("\n"),
    },
    {
      path: "maps/source.scx",
      insertions: 0,
      deletions: 0,
      binary: true,
      omitted: "바이너리 파일이라 내용 비교를 표시하지 않습니다.",
    },
  ],
};

function renderView(overrides: Partial<Parameters<typeof GitHistoryView>[0]> = {}) {
  const props = {
    revision: 0,
    open: true,
    onOpenChange: vi.fn(),
    loadLog: vi.fn(async () => commits),
    loadDetail: vi.fn(async () => detail),
    revert: vi.fn(async () => ({
      sha: "c".repeat(40),
      subject: 'Revert "마린 체력 조정"',
      files: 1,
    })),
    ...overrides,
  };
  render(<GitHistoryView {...props} />);
  return props;
}

describe("GitHistoryView", () => {
  it("lists the commits newest-first and opens the newest one", async () => {
    renderView();

    expect(await screen.findAllByText("마린 체력 조정")).not.toHaveLength(0);
    const rows = screen.getAllByRole("listitem");
    expect(within(rows[0]).getByText("마린 체력 조정")).toBeInTheDocument();
    expect(within(rows[1]).getByText("시작 지점 추가")).toBeInTheDocument();
    // The newest commit is selected without a click, so the panel is never a
    // list with nothing shown beside it.
    expect(
      await screen.findByTestId("commit-file-src/main.eps"),
    ).toBeInTheDocument();
  });

  it("renders a patch with a +/- gutter, not color alone", async () => {
    renderView();

    const file = await screen.findByTestId("commit-file-src/main.eps");
    expect(within(file).getByText("+1")).toBeInTheDocument();
    expect(within(file).getByText("-1")).toBeInTheDocument();

    const added = file.querySelector('[data-diff="add"]');
    const removed = file.querySelector('[data-diff="del"]');
    expect(added?.textContent).toBe("+const next = 3;");
    expect(removed?.textContent).toBe("-const old = 2;");
    // The git preamble is dropped; the hunk header survives.
    expect(file.textContent).not.toContain("diff --git");
    expect(file.querySelector('[data-diff="hunk"]')?.textContent).toContain(
      "@@ -1,2 +1,2 @@",
    );
  });

  it("shows the core's Korean reason when a file carries no patch", async () => {
    renderView();

    const binary = await screen.findByTestId("commit-file-maps/source.scx");
    expect(binary).toHaveTextContent(
      "바이너리 파일이라 내용 비교를 표시하지 않습니다.",
    );
    expect(binary.querySelector('[data-diff="add"]')).toBeNull();
  });

  it("shows the session and request trace quietly", async () => {
    renderView();
    expect(await screen.findByText("s-1")).toBeInTheDocument();
    expect(screen.getByText("r-1")).toBeInTheDocument();
  });

  it("confirms a revert, says it records an inverse commit, and refreshes", async () => {
    const props = renderView();
    await screen.findByRole("button", { name: "마린 체력 조정 되돌리기" });

    await userEvent.click(
      screen.getByRole("button", { name: "마린 체력 조정 되돌리기" }),
    );
    const dialog = await screen.findByRole("dialog");
    // The confirmation has to say history is added to, not erased.
    expect(dialog).toHaveTextContent("취소하는 새 커밋을 남깁니다");
    expect(props.revert).not.toHaveBeenCalled();

    await userEvent.click(within(dialog).getByRole("button", { name: "되돌리기" }));
    await waitFor(() =>
      expect(props.revert).toHaveBeenCalledWith("a".repeat(40)),
    );
    // The log is re-read so the inverse commit appears.
    await waitFor(() => expect(props.loadLog).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("cancelling the confirmation reverts nothing", async () => {
    const props = renderView();
    await screen.findByRole("button", { name: "마린 체력 조정 되돌리기" });

    await userEvent.click(
      screen.getByRole("button", { name: "시작 지점 추가 되돌리기" }),
    );
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "취소" }));

    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(props.revert).not.toHaveBeenCalled();
  });

  it("keeps the dialog open and shows the backend's Korean refusal", async () => {
    const props = renderView({
      revert: vi.fn(async () => {
        throw new Error(
          "저장하지 않은 변경이 남아 있어 되돌릴 수 없습니다. 먼저 현재 상태를 커밋하거나 되돌리세요.",
        );
      }),
    });
    await screen.findByRole("button", { name: "마린 체력 조정 되돌리기" });

    await userEvent.click(
      screen.getByRole("button", { name: "마린 체력 조정 되돌리기" }),
    );
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "되돌리기" }));

    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      "저장하지 않은 변경이 남아 있어 되돌릴 수 없습니다.",
    );
    expect(props.loadLog).toHaveBeenCalledTimes(1);
  });

  it("says so when the project has no commits yet", async () => {
    renderView({ loadLog: vi.fn(async () => []) });
    expect(
      await screen.findByText("아직 기록된 변경이 없습니다."),
    ).toBeInTheDocument();
  });

  it("surfaces a log failure instead of an empty list", async () => {
    renderView({
      loadLog: vi.fn(async () => {
        throw new Error("변경 기록을 읽지 못했습니다.");
      }),
    });
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "변경 기록을 읽지 못했습니다.",
    );
  });
});
