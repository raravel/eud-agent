import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export type BoardConfig = {
  name: string;
  cafeId: string;
  outputFile: string;
  listUrlTemplate: string;
  articleUrlTemplate: string;
  maxPages: number;
};

const srcDir = dirname(fileURLToPath(import.meta.url));
export const repoRoot = resolve(srcDir, "..", "..", "..");
export const corpusOutputDir = resolve(repoRoot, "ci", "corpus");

const cafeId = "17046257";

export const defaultDelayMs = 750;

export const boards: BoardConfig[] = [
  {
    name: "articles",
    cafeId,
    outputFile: "articles.jsonl",
    listUrlTemplate:
      "https://cafe.naver.com/ArticleList.nhn?search.clubid=17046257&search.boardtype=L&search.page={page}",
    articleUrlTemplate:
      "https://cafe.naver.com/f-e/cafes/17046257/articles/{id}",
    maxPages: 20
  },
  {
    name: "cafebook",
    cafeId,
    outputFile: "cafebook.jsonl",
    listUrlTemplate: "https://cafe.naver.com/edac/book5103106?page={page}",
    articleUrlTemplate: "https://cafe.naver.com/edac/book5103106/{id}",
    maxPages: 20
  },
  {
    name: "eud_book",
    cafeId,
    outputFile: "eud_book.jsonl",
    listUrlTemplate:
      "https://cafe.naver.com/ArticleList.nhn?search.clubid=17046257&search.query=eud%20book&search.page={page}",
    articleUrlTemplate:
      "https://cafe.naver.com/f-e/cafes/17046257/articles/{id}",
    maxPages: 20
  }
];

export function getBoards(names?: string[]): BoardConfig[] {
  if (!names || names.length === 0) {
    return boards;
  }

  const requested = new Set(names);
  const selected = boards.filter((board) => requested.has(board.name));
  const missing = names.filter((name) => !boards.some((board) => board.name === name));

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
