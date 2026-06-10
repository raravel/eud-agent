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

type BoardWritePlan = {
  board: BoardConfig;
  outputPath: string;
  rows: CorpusJsonRow[];
  newRows: CorpusJsonRow[];
  skipped: number;
};

export async function scrape(options: ScrapeOptions): Promise<ScrapeSummary[]> {
  const selectedBoards = getBoards(options.boards);
  const plans: BoardWritePlan[] = [];
  const dryRun = options.dryRun ?? false;
  const delayMs = options.delayMs ?? defaultDelayMs;
  const limit = dryRun && options.limit === undefined ? 3 : options.limit;

  for (const board of selectedBoards) {
    const outputPath = join(corpusOutputDir, board.outputFile);
    const existingRows = await readCorpusRows(outputPath);
    const existingIds = dryRun ? new Set<string>() : collectExistingIds(existingRows);
    const articleRefs = await collectArticleRefs(
      options.client,
      board,
      existingIds,
      delayMs,
      limit
    );
    const newRows: CorpusJsonRow[] = [];

    for (const articleRef of articleRefs) {
      await sleep(delayMs);
      const html = await options.client.fetchText(articleRef.url);
      const parsed = parsePostHtml(html, board, articleRef);
      newRows.push(postToCorpusRow(parsed));
    }

    const rows = sortRowsByPostId([...existingRows, ...newRows]);
    plans.push({
      board,
      outputPath,
      rows,
      newRows,
      skipped: existingIds.size
    });
  }

  if (dryRun) {
    for (const plan of plans) {
      for (const row of plan.newRows) {
        console.log(serializeCorpusRow(row));
      }
    }
  } else {
    for (const plan of plans) {
      await writeCorpusJsonlAtomic(plan.outputPath, plan.rows);
    }
  }

  return plans.map((plan) => ({
    board: plan.board.name,
    outputPath: plan.outputPath,
    fetched: plan.newRows.length,
    skipped: plan.skipped,
    totalRows: plan.rows.length
  }));
}

async function collectArticleRefs(
  client: NaverClient,
  board: BoardConfig,
  existingIds: Set<string>,
  delayMs: number,
  limit?: number
): Promise<ArticleRef[]> {
  const refs = new Map<string, ArticleRef>();

  for (let page = 1; page <= board.maxPages; page += 1) {
    if (limit !== undefined && refs.size >= limit) {
      break;
    }

    if (page > 1) {
      await sleep(delayMs);
    }

    const url = renderTemplate(board.listUrlTemplate, { page: String(page) });
    const html = await client.fetchText(url);
    const pageRefs = parseArticleListHtml(html, board);

    if (pageRefs.length === 0) {
      break;
    }

    for (const ref of pageRefs) {
      if (!existingIds.has(ref.id) && !refs.has(ref.id)) {
        refs.set(ref.id, ref);
      }

      if (limit !== undefined && refs.size >= limit) {
        break;
      }
    }
  }

  return [...refs.values()].sort(compareArticleRefs);
}

function parseArticleListHtml(html: string, board: BoardConfig): ArticleRef[] {
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

    const url = normalizeArticleUrl(href, board, id);
    const title = anchor.text().replace(/\s+/g, " ").trim();
    refs.set(id, { id, url, title: title.length > 0 ? title : undefined });
  });

  return [...refs.values()].sort(compareArticleRefs);
}

function parsePostHtml(html: string, board: BoardConfig, ref: ArticleRef): ParsedPost {
  const $ = load(html);
  $("script, style, noscript, template").remove();

  const title =
    textFromMeta($, 'meta[property="og:title"]') ??
    firstText($, [
      ".title_text",
      ".ArticleTitle",
      ".article_title",
      "h1",
      "h2",
      "title"
    ]) ??
    ref.title ??
    ref.id;

  const contentHtml =
    firstHtml($, [
      ".se-main-container",
      ".article_viewer",
      ".ArticleContentBox",
      ".ContentRenderer",
      "#postContent",
      "article"
    ]) ??
    $("body").html() ??
    html;

  return {
    id: ref.id,
    title,
    url: ref.url,
    board: board.name,
    contentHtml
  };
}

function normalizeArticleUrl(href: string, board: BoardConfig, id: string): string {
  if (/^https?:\/\//i.test(href)) {
    return href;
  }

  if (href.startsWith("/")) {
    return new URL(href, "https://cafe.naver.com").toString();
  }

  return renderTemplate(board.articleUrlTemplate, { id });
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

function textFromMeta(
  $: ReturnType<typeof load>,
  selector: string
): string | undefined {
  const value = $(selector).first().attr("content")?.trim();
  return value && value.length > 0 ? value : undefined;
}

function firstText(
  $: ReturnType<typeof load>,
  selectors: string[]
): string | undefined {
  for (const selector of selectors) {
    const text = $(selector).first().text().replace(/\s+/g, " ").trim();
    if (text.length > 0) {
      return text;
    }
  }

  return undefined;
}

function firstHtml(
  $: ReturnType<typeof load>,
  selectors: string[]
): string | undefined {
  for (const selector of selectors) {
    const element = $(selector).first();
    const html = element.html();
    if (html && element.text().trim().length > 0) {
      return html;
    }
  }

  return undefined;
}

function sleep(ms: number): Promise<void> {
  if (ms <= 0) {
    return Promise.resolve();
  }

  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}
