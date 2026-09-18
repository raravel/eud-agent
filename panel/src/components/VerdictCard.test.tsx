/**
 * Verifier verdict card: pass/fail title, summary, unmet list, attempt badge.
 */
import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { VerdictCard } from "@/components/VerdictCard";
import { ResearchCard } from "@/components/ResearchCard";
import userEvent from "@testing-library/user-event";

describe("VerdictCard", () => {
  it("renders a failed verdict with its unmet list and attempt counter", () => {
    render(
      <VerdictCard
        verdict={{
          verdict: "fail",
          summary: "빌드는 통과했지만 수용 기준이 미충족입니다.",
          path: ".eud-agent/workspace/verify/req-1.md",
          sha256: "b".repeat(64),
          unmet: ["모든 플레이어에게 미네랄 지급", "게임 시작 시 1회만 실행"],
        }}
        attempts={1}
      />,
    );
    const card = screen.getByRole("region", { name: "검증 결과" });
    expect(card).toHaveAttribute("data-verdict", "fail");
    expect(within(card).getByText("검증 실패")).toBeInTheDocument();
    expect(
      within(card).getByText("빌드는 통과했지만 수용 기준이 미충족입니다."),
    ).toBeInTheDocument();
    const unmet = within(card).getByRole("list", { name: "미충족 항목" });
    expect(within(unmet).getAllByRole("listitem").map((item) => item.textContent)).toEqual([
      "모든 플레이어에게 미네랄 지급",
      "게임 시작 시 1회만 실행",
    ]);
    expect(within(card).getByText("검증 1/2")).toBeInTheDocument();
  });

  it("renders a passed verdict without an unmet list", () => {
    render(
      <VerdictCard
        verdict={{
          verdict: "pass",
          summary: "모든 수용 기준을 충족합니다.",
          path: ".eud-agent/workspace/verify/req-1.md",
          sha256: "c".repeat(64),
          unmet: [],
        }}
      />,
    );
    const card = screen.getByRole("region", { name: "검증 결과" });
    expect(card).toHaveAttribute("data-verdict", "pass");
    expect(within(card).getByText("검증 통과")).toBeInTheDocument();
    expect(within(card).queryByRole("list", { name: "미충족 항목" })).toBeNull();
  });
});

describe("ResearchCard", () => {
  it("collapses by default and reveals the summary and path when expanded", async () => {
    render(
      <ResearchCard
        research={{
          path: ".eud-agent/workspace/research/req-1.md",
          sha256: "a".repeat(64),
          summary: "트리거 3개와 MainFile 구성을 확인했습니다.",
        }}
      />,
    );
    expect(screen.queryByText("트리거 3개와 MainFile 구성을 확인했습니다.")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "조사 결과 펼치기" }));
    expect(
      screen.getByText("트리거 3개와 MainFile 구성을 확인했습니다."),
    ).toBeInTheDocument();
    expect(
      screen.getByText(".eud-agent/workspace/research/req-1.md"),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "조사 결과 접기" })).toBeInTheDocument();
  });
});
