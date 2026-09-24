/**
 * Reference chunk text → readable markdown for the "참고 문서" document tab.
 *
 * The index stores plain text scraped from cafe posts and eud-book pages, not
 * markdown: a `제목: …` header the tab already shows, cafe page chrome lines,
 * `[[[CONTENT-ELEMENT-n]]]` placeholders for images/attachments that were not
 * indexed, and a `[댓글]` section of `- 작성자: 내용` lines. This keeps every line
 * break and indentation the text has, escapes markdown syntax so scraped text
 * never turns into accidental emphasis/HTML, leaves bare URLs clickable, and
 * turns the comment marker into a heading.
 */

/** Cafe page chrome that carries no article content. */
const NOISE_LINES = new Set([
  ">",
  "|",
  "카페 캘린더로 보내시겠습니까?",
  "이 멤버의 글을 탭에서 볼 수 있습니다.",
  "내PC 저장",
  "네이버 MYBOX 저장",
]);

const CAFE_CHROME_END = "카페 캘린더로 보내시겠습니까?";
const CHROME_SCAN_LINES = 40;

const CONTENT_ELEMENT = /\[\[\[CONTENT-ELEMENT-\d+\]\]\]/g;
const URL = /https?:\/\/[^\s<>()[\]]+/g;
const MARKDOWN_SYNTAX = /[\\`*_<>[\]|~#]/g;

function escapeText(text: string): string {
  return text.replace(MARKDOWN_SYNTAX, (char) => `\\${char}`);
}

/** Escape a line outside its URLs; keep leading indentation visible. */
function renderLine(line: string): string {
  const indent = /^[ \t]*/.exec(line)?.[0] ?? "";
  const body = line.slice(indent.length);
  let out = "";
  let last = 0;
  for (const match of body.matchAll(URL)) {
    out += escapeText(body.slice(last, match.index)) + match[0];
    last = match.index + match[0].length;
  }
  out += escapeText(body.slice(last));
  // A leading `-`/`=`/`+` or `1)` would make a setext heading, rule or list.
  out = out.replace(/^([-=+])/, "\\$1").replace(/^(\d+)([.)])/, "$1\\$2");
  const spaces = indent.replace(/\t/g, "    ").length;
  return "&nbsp;".repeat(spaces) + out;
}

export function referenceMarkdown(text: string, title: string): string {
  const lines = text.replace(/\r\n?/g, "\n").replace(CONTENT_ELEMENT, "").split("\n");

  // A cafe post opens with page chrome (board, title, author, view and comment
  // counts) that ends at the calendar prompt; the article starts after it.
  const chromeEnd = lines
    .slice(0, CHROME_SCAN_LINES)
    .findIndex((line) => line.trim() === CAFE_CHROME_END);
  // The document tab shows the title; drop the scraped `제목:` header (it is
  // sometimes repeated) and the bare title line some cafe pages repeat after it.
  let start = chromeEnd + 1;
  const titleText = title.trim();
  while (start < lines.length) {
    const line = lines[start].trim();
    if (line === "" || line.startsWith("제목:") || line === titleText) start += 1;
    else break;
  }

  const blocks: string[][] = [[]];
  for (const raw of lines.slice(start)) {
    const line = raw.trimEnd();
    if (NOISE_LINES.has(line.trim())) continue;
    if (line.trim() === "") {
      if (blocks[blocks.length - 1].length > 0) blocks.push([]);
      continue;
    }
    if (line.trim() === "[댓글]") {
      if (blocks[blocks.length - 1].length > 0) blocks.push([]);
      blocks[blocks.length - 1].push("## 댓글");
      blocks.push([]);
      continue;
    }
    // A list marker the text already has stays a list; everything else is text.
    const list = /^(\s*)([-*]|\d+\.) (.*)$/.exec(line);
    blocks[blocks.length - 1].push(
      list && list[1] === "" ? `${list[2] === "*" ? "-" : list[2]} ${renderLine(list[3])}` : renderLine(line),
    );
  }

  return blocks
    .filter((block) => block.length > 0)
    .map((block) =>
      block[0] === "## 댓글" ? block[0] : block.join("  \n"),
    )
    .join("\n\n");
}
