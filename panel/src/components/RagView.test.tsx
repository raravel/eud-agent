import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { RagSearchResponse } from "@/lib/ipc";

import { RagView } from "./RagView";

const RESPONSE: RagSearchResponse = {
  query: "chatEvent",
  indexSize: 1234,
  semanticReady: false,
  semanticUnavailable: false,
  hits: [
    {
      id: "000000000000002a",
      title: "채팅 감지 강좌",
      url: "https://cafe.example/42",
      part: [2, 3],
      tier: "lecture",
      matchKind: "lexical",
      score: 3,
      text: "제목: 채팅 감지 강좌\n\nchatEvent 는 채팅을 감지합니다.",
    },
  ],
};

function search(query: string) {
  fireEvent.change(screen.getByLabelText("참고 문서 검색어"), { target: { value: query } });
  fireEvent.click(screen.getByRole("button", { name: "참고 문서 검색" }));
}

describe("RagView", () => {
  it("searches the trimmed query and shows each hit with its tier, match kind and part", async () => {
    const onSearch = vi.fn().mockResolvedValue(RESPONSE);
    render(<RagView onSearch={onSearch} onOpen={vi.fn()} />);

    search("  chatEvent ");

    expect(onSearch).toHaveBeenCalledWith("chatEvent");
    expect(await screen.findByText("채팅 감지 강좌")).toBeInTheDocument();
    expect(screen.getByText("강좌")).toBeInTheDocument();
    expect(screen.getByText("어휘 일치")).toBeInTheDocument();
    expect(screen.getByText("조각 2/3")).toBeInTheDocument();
    expect(screen.getByText("chatEvent 는 채팅을 감지합니다.")).toBeInTheDocument();
    expect(screen.getByText(/1,234개 조각 중 1개 결과/)).toBeInTheDocument();
    expect(screen.getByText(/어휘 일치 결과만 표시합니다/)).toBeInTheDocument();
  });

  it("says semantic search is unavailable when the embedding runtime did not load", async () => {
    const onSearch = vi.fn().mockResolvedValue({ ...RESPONSE, semanticUnavailable: true });
    render(<RagView onSearch={onSearch} onOpen={vi.fn()} />);

    search("chatEvent");

    expect(await screen.findByText(/의미 검색을 쓸 수 없어 키워드 일치 결과만/)).toBeInTheDocument();
    expect(screen.queryByText(/의미 검색 모델 준비 중/)).not.toBeInTheDocument();
  });

  it("opens the selected hit as a document and marks the active one", async () => {
    const onOpen = vi.fn();
    const { rerender } = render(
      <RagView onSearch={vi.fn().mockResolvedValue(RESPONSE)} onOpen={onOpen} />,
    );
    search("chatEvent");

    fireEvent.click(await screen.findByRole("button", { name: "채팅 감지 강좌 문서 열기" }));
    expect(onOpen).toHaveBeenCalledWith(RESPONSE.hits[0]);

    rerender(
      <RagView
        onSearch={vi.fn().mockResolvedValue(RESPONSE)}
        onOpen={onOpen}
        activeId="000000000000002a"
      />,
    );
    expect(screen.getByRole("button", { name: "채팅 감지 강좌 문서 열기" })).toHaveAttribute(
      "aria-current",
      "true",
    );
  });

  it("explains a missing index and a failed search with a recovery action", async () => {
    const onSearch = vi
      .fn()
      .mockResolvedValueOnce({ ...RESPONSE, indexSize: 0, hits: [] })
      .mockRejectedValueOnce(new Error("boom"));
    render(<RagView onSearch={onSearch} onOpen={vi.fn()} />);

    search("유닛");
    expect(await screen.findByText(/RAG 자산을 받아 주세요/)).toBeInTheDocument();

    search("유닛 생성");
    await waitFor(() =>
      expect(screen.getByText(/다시 시도해 주세요. \(boom\)/)).toBeInTheDocument(),
    );
  });
});
