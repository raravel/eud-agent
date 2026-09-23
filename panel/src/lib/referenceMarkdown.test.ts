import { describe, expect, it } from "vitest";

import { referenceMarkdown } from "./referenceMarkdown";

describe("referenceMarkdown", () => {
  it("drops the scraped title header, cafe chrome and attachment placeholders", () => {
    const text = [
      "제목: 쿠션 실험",
      "",
      "제목: 쿠션 실험",
      "",
      "쿠션 실험",
      "",
      "이 멤버의 글을 탭에서 볼 수 있습니다.",
      ">",
      "본문 첫 줄[[[CONTENT-ELEMENT-0]]]",
      "본문 둘째 줄",
    ].join("\n");

    expect(referenceMarkdown(text, "쿠션 실험")).toBe("본문 첫 줄  \n본문 둘째 줄");
  });

  it("keeps line breaks and indentation and escapes markdown syntax outside URLs", () => {
    const text = "function f() {\n    var __addr__ = 1; // *x*\n}\n원문 https://blog.example/a_b_c";

    expect(referenceMarkdown(text, "t")).toBe(
      [
        "function f() {",
        "&nbsp;&nbsp;&nbsp;&nbsp;var \\_\\_addr\\_\\_ = 1; // \\*x\\*",
        "}",
        "원문 https://blog.example/a_b_c",
      ].join("  \n"),
    );
  });

  it("skips the cafe page chrome and keeps rule-like lines as text", () => {
    const text = [
      "제목: 실험",
      "",
      "맵 제작 연구/칼럼",
      "실험",
      "xvnkr",
      "1,046",
      "카페 캘린더로 보내시겠습니까?",
      "출처",
      "---",
      "= 결과 =",
      "1) 첫째",
    ].join("\n");

    expect(referenceMarkdown(text, "실험")).toBe(
      ["출처", "\\---", "\\= 결과 =", "1\\) 첫째"].join("  \n"),
    );
  });

  it("turns the comment marker into a heading over a comment list", () => {
    const text = "본문\n\n[댓글]\n- 작성자: 감사합니다 <3\n- 방문자: 좋아요";

    expect(referenceMarkdown(text, "t")).toBe(
      "본문\n\n## 댓글\n\n- 작성자: 감사합니다 \\<3  \n- 방문자: 좋아요",
    );
  });
});
