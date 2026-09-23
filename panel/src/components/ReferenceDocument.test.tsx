import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { RagArticle } from "@/lib/ipc";

import { ReferenceDocument } from "./ReferenceDocument";

const ARTICLE: RagArticle = {
  id: "000000000000002a",
  title: "채팅 감지 강좌",
  url: "https://cafe.example/42",
  tier: "lecture",
  parts: 3,
  complete: true,
  text: "제목: 채팅 감지 강좌\n\n본문 https://blog.example/post 참고\n\n[댓글]\n- 작성자: 감사합니다",
};

describe("ReferenceDocument", () => {
  it("renders the joined article as markdown with its source header", () => {
    render(
      <ReferenceDocument title="채팅 감지 강좌" article={ARTICLE} loading={false} error={null} onOpenLink={vi.fn()} />,
    );

    expect(screen.getByRole("heading", { level: 1, name: "채팅 감지 강좌" })).toBeInTheDocument();
    expect(screen.getByText("강좌")).toBeInTheDocument();
    expect(screen.getByText("조각 3개 합침")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "댓글" })).toBeInTheDocument();
    expect(screen.getByText("작성자: 감사합니다")).toBeInTheDocument();
    expect(screen.queryByText(/제목:/)).not.toBeInTheDocument();
  });

  it("opens the original and every body link in the browser", () => {
    const onOpenLink = vi.fn();
    render(
      <ReferenceDocument title="채팅 감지 강좌" article={ARTICLE} loading={false} error={null} onOpenLink={onOpenLink} />,
    );

    fireEvent.click(screen.getByRole("button", { name: "채팅 감지 강좌 원문 열기" }));
    expect(onOpenLink).toHaveBeenLastCalledWith("https://cafe.example/42");

    fireEvent.click(screen.getByRole("link", { name: "https://blog.example/post" }));
    expect(onOpenLink).toHaveBeenLastCalledWith("https://blog.example/post");
  });

  it("says when only the searched chunk could be shown", () => {
    render(
      <ReferenceDocument
        title="t"
        article={{ ...ARTICLE, parts: 1, complete: false }}
        loading={false}
        error={null}
        onOpenLink={vi.fn()}
      />,
    );

    expect(screen.getByRole("status")).toHaveTextContent("원문에서 확인해 주세요");
    expect(screen.queryByText("조각 3개 합침")).not.toBeInTheDocument();
  });
});
