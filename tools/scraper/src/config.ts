import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export type BoardConfig = {
  name: string;
  source: string;
  outputFile: string;
  list:
    | { kind: "api-menu"; menuId: number }
    | { kind: "html"; urlTemplate: string };
  maxPages: number;
};

const srcDir = dirname(fileURLToPath(import.meta.url));
export const repoRoot = resolve(srcDir, "..", "..", "..");
export const corpusOutputDir = resolve(repoRoot, "ci", "corpus");

const cafeId = "17046257";

export const defaultDelayMs = 750;

export const boards: BoardConfig[] = [
  {
    name: "lua",
    source: "board_Lua자료실.jsonl",
    outputFile: "articles.jsonl",
    list: { kind: "api-menu", menuId: 229 },
    maxPages: 20
  },
  {
    name: "lectures",
    source: "board_강좌팁.jsonl",
    outputFile: "articles.jsonl",
    list: { kind: "api-menu", menuId: 15 },
    maxPages: 20
  },
  {
    name: "research",
    source: "board_연구칼럼.jsonl",
    outputFile: "articles.jsonl",
    list: { kind: "api-menu", menuId: 33 },
    maxPages: 20
  },
  {
    name: "utilities",
    source: "board_유틸리티툴.jsonl",
    outputFile: "articles.jsonl",
    list: { kind: "api-menu", menuId: 20 },
    maxPages: 20
  },
  {
    name: "qna",
    source: "board_질문답변.jsonl",
    outputFile: "articles.jsonl",
    list: { kind: "api-menu", menuId: 12 },
    maxPages: 20
  },
  {
    name: "cafebook",
    source: "cafebook.jsonl",
    outputFile: "cafebook.jsonl",
    list: {
      kind: "html",
      urlTemplate: "https://cafe.naver.com/edac/book5103106?page={page}"
    },
    maxPages: 20
  }
];

export function getBoards(names?: string[]): BoardConfig[] {
  if (!names || names.length === 0) {
    return boards;
  }

  const expandedNames = names.flatMap((name) =>
    name === "articles"
      ? boards.filter((board) => board.outputFile === "articles.jsonl").map((board) => board.name)
      : [name]
  );
  const requested = new Set(expandedNames);
  const selected = boards.filter((board) => requested.has(board.name));
  const missing = names.filter(
    (name) => name !== "articles" && !boards.some((board) => board.name === name)
  );

  if (missing.length > 0) {
    throw new Error(
      `Unknown board(s): ${missing.join(", ")}. Available boards: ${boards
        .map((board) => board.name)
        .join(", ")}`
    );
  }

  return selected;
}

export function renderTemplate(template: string, values: Record<string, string>): string {
  return template.replace(/\{([a-zA-Z0-9_]+)\}/g, (_, key: string) => {
    const value = values[key];
    if (value === undefined) {
      throw new Error(`Missing template value: ${key}`);
    }
    return encodeURIComponent(value);
  });
}
