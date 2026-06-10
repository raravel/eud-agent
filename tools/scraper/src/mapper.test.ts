import { describe, expect, it } from "vitest";
import { postToCorpusRow } from "./mapper.js";

describe("postToCorpusRow", () => {
  it("maps a parsed Naver Cafe post to the corpus JSONL row schema", () => {
    const row = postToCorpusRow({
      id: "141465",
      title: "채팅출력utf8.lua (수정3)",
      url: "https://cafe.naver.com/f-e/cafes/17046257/articles/141465",
      board: "Lua자료실",
      contentHtml: `
        <article>
          <p>기존 글은 구버전 카페글 에디터에서 작성했습니다.</p>
          <pre>print_utf8(line, offset, string)</pre>
        </article>
      `
    });

    expect(row).toEqual({
      title: "채팅출력utf8.lua (수정3)",
      content:
        "기존 글은 구버전 카페글 에디터에서 작성했습니다.\n\nprint_utf8(line, offset, string)",
      url: "https://cafe.naver.com/f-e/cafes/17046257/articles/141465",
      source: "board_Lua자료실.jsonl"
    });
  });

  it("preserves line breaks and indentation inside preformatted blocks", () => {
    const row = postToCorpusRow({
      id: "126461",
      title: "Multiline code",
      url: "https://cafe.naver.com/edac/book5103106/126461",
      board: "cafebook",
      contentHtml: `
        <article>
          <p>Example:</p>
          <pre>
function afterTriggerExec() {
  const value = 1;
}
          </pre>
        </article>
      `
    });

    expect(row.content).toBe(
      "Example:\n\nfunction afterTriggerExec() {\n  const value = 1;\n}"
    );
  });
});
