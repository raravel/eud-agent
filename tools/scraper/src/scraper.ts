import { join } from "node:path";
import { load } from "cheerio";
import {
  type CorpusJsonRow,
  readCorpusRows,
  serializeCorpusRow,
  writeCorpusJsonlAtomic
} from "./corpusWriter.js";
import {
  type BoardConfig,
  corpusOutputDir,
  defaultDelayMs,
  getBoards,
  renderTemplate
} from "./config.js";
import type { ParsedPost } from "./mapper.js";
import { postToCorpusRow } from "./mapper.js";
import type { NaverClient } from "./naverClient.js";

const cafeId = "17046257";
const articlePageSize = 50;

export type ScrapeOptions = {
  client: NaverClient;
  boards?: string[];
  delayMs?: number;
  dryRun?: boolean;
  limit?: number;
};

export type ScrapeSummary = {
  board: string;
  outputPath: string;
  fetched: number;
  skipped: number;
  totalRows: number;
};

type ArticleRef = {
  id: string;
  url: string;
  title?: string;
};

type ArticleCollection = {
  refs: ArticleRef[];
  skipped: number;
};

type OutputState = {
  outputPath: string;
  rows: CorpusJsonRow[];
  existingIds: Set<string>;
  newRows: CorpusJsonRow[];
};

type BoardListResponse = {
  result?: {
    articleList?: Array<{
      type?: string;
      item?: {
        articleId?: number;
        menuId?: number;
        subject?: string;
      };
    }>;
  };
};

type ArticleResponse = {
  result?: {
    article?: {
      id?: number;
      subject?: string;
      contentHtml?: string;
      isReadable?: boolean;
      menu?: {
        id?: number;
        name?: string;
      };
    };
    comments?: {
      items?: Array<{
        content?: string;
        writer?: { nick?: string };
      }>;
    };
  };
};

export async function scrape(options: ScrapeOptions): Promise<ScrapeSummary[]> {
  const selectedBoards = getBoards(options.boards);
  const outputStates = new Map<string, OutputState>();
  const summaries: ScrapeSummary[] = [];
  const dryRun = options.dryRun ?? false;
  const delayMs = options.delayMs ?? defaultDelayMs;
  const limit = dryRun && options.limit === undefined ? 3 : options.limit;

  for (const board of selectedBoards) {
    const outputPath = join(corpusOutputDir, board.outputFile);
    let state = outputStates.get(outputPath);
    if (!state) {
      const rows = await readCorpusRows(outputPath);
      state = {
        outputPath,
        rows,
        existingIds: dryRun ? new Set<string>() : collectExistingIds(rows),
        newRows: []
      };
      outputStates.set(outputPath, state);
    }

    const collection = await collectArticleRefs(
      options.client,
      board,
      state.existingIds,
      delayMs,
      limit
    );
    const boardRows: CorpusJsonRow[] = [];

    for (const articleRef of collection.refs) {
      await sleep(delayMs);
      const parsed = await fetchPost(options.client, board, articleRef);
      const row = postToCorpusRow(parsed);
      boardRows.push(row);
      state.newRows.push(row);
      state.rows.push(row);
      state.existingIds.add(parsed.id);
    }

    summaries.push({
      board: board.name,
      outputPath,
      fetched: boardRows.length,
      skipped: collection.skipped,
      totalRows: state.rows.length
    });
  }

  if (dryRun) {
    for (const state of outputStates.values()) {
      for (const row of state.newRows) {
        console.log(serializeCorpusRow(row));
      }
    }
  } else {
    for (const state of outputStates.values()) {
      await writeCorpusJsonlAtomic(state.outputPath, sortRowsByPostId(state.rows));
    }
  }

  return summaries;
}

async function collectArticleRefs(
  client: NaverClient,
  board: BoardConfig,
  existingIds: Set<string>,
  delayMs: number,
  limit?: number
): Promise<ArticleCollection> {
  const list = board.list;
  if (list.kind === "api-menu") {
    return collectMenuArticleRefs(
      client,
      board,
      list.menuId,
      existingIds,
      delayMs,
      limit
    );
  }
  return collectHtmlArticleRefs(
    client,
    board,
    list.urlTemplate,
    existingIds,
    delayMs,
    limit
  );
}

async function collectMenuArticleRefs(
  client: NaverClient,
  board: BoardConfig,
  menuId: number,
  existingIds: Set<string>,
  delayMs: number,
  limit?: number
): Promise<ArticleCollection> {
  const refs = new Map<string, ArticleRef>();
  let skipped = 0;

  for (let page = 1; page <= board.maxPages; page += 1) {
    if (limit !== undefined && refs.size >= limit) {
      break;
    }
    if (page > 1) {
      await sleep(delayMs);
    }

    const url =
      `https://apis.naver.com/cafe-web/cafe-boardlist-api/v1/cafes/${cafeId}` +
      `/menus/${menuId}/articles?page=${page}&pageSize=${articlePageSize}` +
      "&sortBy=TIME&viewType=L";
    const payload = await client.fetchJson<BoardListResponse>(url);
    const items = payload.result?.articleList ?? [];
    if (items.length === 0) {
      break;
    }

    let newOnPage = 0;
    for (const wrapper of items) {
      const item = wrapper.item;
      if (
        wrapper.type !== "ARTICLE" ||
        !item?.articleId ||
        item.menuId !== menuId
      ) {
        continue;
      }

      const id = String(item.articleId);
      if (existingIds.has(id)) {
        skipped += 1;
        continue;
      }
      if (!refs.has(id)) {
        refs.set(id, {
          id,
          title: item.subject?.trim(),
          url: canonicalArticleUrl(id)
        });
        newOnPage += 1;
      }
      if (limit !== undefined && refs.size >= limit) {
        break;
      }
    }

    if (newOnPage === 0) {
      break;
    }
  }

  return { refs: [...refs.values()].sort(compareArticleRefs), skipped };
}

async function collectHtmlArticleRefs(
  client: NaverClient,
  board: BoardConfig,
  urlTemplate: string,
  existingIds: Set<string>,
  delayMs: number,
  limit?: number
): Promise<ArticleCollection> {
  const refs = new Map<string, ArticleRef>();
  let skipped = 0;

  for (let page = 1; page <= board.maxPages; page += 1) {
    if (limit !== undefined && refs.size >= limit) {
      break;
    }
    if (page > 1) {
      await sleep(delayMs);
    }

    const url = renderTemplate(urlTemplate, { page: String(page) });
    const pageRefs = parseArticleListHtml(await client.fetchText(url));
    if (pageRefs.length === 0) {
      break;
    }

    let newOnPage = 0;
    for (const ref of pageRefs) {
      if (existingIds.has(ref.id)) {
        skipped += 1;
        continue;
      }
      if (!refs.has(ref.id)) {
        refs.set(ref.id, ref);
        newOnPage += 1;
      }
      if (limit !== undefined && refs.size >= limit) {
        break;
      }
    }

    if (newOnPage === 0) {
      break;
    }
  }

  return { refs: [...refs.values()].sort(compareArticleRefs), skipped };
}

function parseArticleListHtml(html: string): ArticleRef[] {
  const $ = load(html);
  const refs = new Map<string, ArticleRef>();

  $("a[href]").each((_, element) => {
    const anchor = $(element);
    const href = anchor.attr("href");
    if (!href) {
      return;
    }

    const id = extractArticleId(href);
    if (!id) {
      return;
    }

    const title = anchor.text().replace(/\s+/g, " ").trim();
    refs.set(id, {
      id,
      url: canonicalArticleUrl(id),
      title: title.length > 0 ? title : undefined
    });
  });

  return [...refs.values()].sort(compareArticleRefs);
}

async function fetchPost(
  client: NaverClient,
  board: BoardConfig,
  ref: ArticleRef
): Promise<ParsedPost> {
  const url =
    `https://article.cafe.naver.com/gw/v4/cafes/${cafeId}/articles/${ref.id}` +
    "?query=&useCafeId=true&requestFrom=A";
  const payload = await client.fetchJson<ArticleResponse>(url);
  const article = payload.result?.article;

  if (!article || article.isReadable === false || !article.contentHtml) {
    throw new Error(`Naver article ${ref.id} is missing or not readable`);
  }
  if (board.list.kind === "api-menu" && article.menu?.id !== board.list.menuId) {
    throw new Error(
      `Naver article ${ref.id} moved from menu ${board.list.menuId} to ${article.menu?.id ?? "unknown"}`
    );
  }

  return {
    id: String(article.id ?? ref.id),
    title: article.subject?.trim() || ref.title || ref.id,
    url: canonicalArticleUrl(ref.id),
    source: board.source,
    contentHtml: article.contentHtml,
    comments: renderComments(payload.result?.comments?.items ?? [])
  };
}

function renderComments(
  comments: Array<{ content?: string; writer?: { nick?: string } }>
): string | undefined {
  const lines: string[] = [];
  for (const comment of comments) {
    const content = comment.content?.replace(/\s+/g, " ").trim();
    if (!content) {
      continue;
    }
    const nick = comment.writer?.nick?.trim();
    lines.push(nick ? `- ${nick}: ${content}` : `- ${content}`);
  }
  return lines.length > 0 ? lines.join("\n") : undefined;
}

function canonicalArticleUrl(id: string): string {
  return `https://cafe.naver.com/f-e/cafes/${cafeId}/articles/${id}`;
}

function extractArticleId(value: string): string | undefined {
  const decoded = value.replace(/&amp;/g, "&");
  const match =
    decoded.match(/\/articles\/(\d+)/i) ??
    decoded.match(/[?&]articleid=(\d+)/i) ??
    decoded.match(/[?&]articleId=(\d+)/) ??
    decoded.match(/\/book\d+\/(\d+)/i);
  return match?.[1];
}

function collectExistingIds(rows: CorpusJsonRow[]): Set<string> {
  const ids = new Set<string>();

  for (const row of rows) {
    if (typeof row.id === "string" && row.id.trim().length > 0) {
      ids.add(row.id.trim());
    }
    if (typeof row.url === "string") {
      const id = extractArticleId(row.url);
      if (id) {
        ids.add(id);
      }
    }
  }

  return ids;
}

function sortRowsByPostId(rows: CorpusJsonRow[]): CorpusJsonRow[] {
  return [...rows].sort((left, right) => {
    const leftId = rowNumericId(left);
    const rightId = rowNumericId(right);
    if (leftId !== rightId) {
      return leftId - rightId;
    }

    const sourceOrder = String(left.source ?? "").localeCompare(
      String(right.source ?? ""),
      "ko"
    );
    if (sourceOrder !== 0) {
      return sourceOrder;
    }
    return String(left.title ?? "").localeCompare(String(right.title ?? ""), "ko");
  });
}

function rowNumericId(row: CorpusJsonRow): number {
  if (typeof row.id === "string") {
    const parsed = Number.parseInt(row.id, 10);
    if (Number.isFinite(parsed)) {
      return parsed;
    }
  }
  if (typeof row.url === "string") {
    const id = extractArticleId(row.url);
    if (id) {
      return Number.parseInt(id, 10);
    }
  }
  return Number.MAX_SAFE_INTEGER;
}

function compareArticleRefs(left: ArticleRef, right: ArticleRef): number {
  return Number.parseInt(left.id, 10) - Number.parseInt(right.id, 10);
}

function sleep(ms: number): Promise<void> {
  if (ms <= 0) {
    return Promise.resolve();
  }

  const { promise, resolve } = Promise.withResolvers<void>();
  setTimeout(resolve, ms);
  return promise;
}
