import { load } from "cheerio";

export type ParsedPost = {
  id: string;
  title: string;
  url: string;
  board: string;
  contentHtml: string;
};

export type CorpusRow = {
  title: string;
  content: string;
  url?: string;
  source: string;
};

const blockSelector = "p, pre, li, div, h1, h2, h3, h4, h5, h6";
const nonVisibleSelector =
  'script, style, noscript, template, [hidden], [aria-hidden="true"]';

export function postToCorpusRow(post: ParsedPost): CorpusRow {
  const content = htmlToVisibleBlocks(post.contentHtml);
  const row: CorpusRow = {
    title: post.title.trim(),
    content,
    source: `board_${post.board}.jsonl`
  };

  const url = post.url.trim();
  if (url.length > 0) {
    row.url = url;
  }

  return row;
}

function htmlToVisibleBlocks(html: string): string {
  const $ = load(html);
  $(nonVisibleSelector).remove();

  const blocks: string[] = [];

  $(blockSelector).each((_, element) => {
    const node = $(element);

    if (node.find(blockSelector).length > 0) {
      return;
    }

    const text = node.is("pre")
      ? normalizePreformattedText(node.text())
      : normalizeBlockText(node.text());
    if (text.length > 0) {
      blocks.push(text);
    }
  });

  if (blocks.length === 0) {
    const fallback = normalizeBlockText($.root().text());
    return fallback;
  }

  return blocks.join("\n\n");
}

function normalizeBlockText(text: string): string {
  return text.replace(/\s+/g, " ").trim();
}

function normalizePreformattedText(text: string): string {
  const lines = text
    .replace(/\r\n?/g, "\n")
    .split("\n")
    .map((line) => line.replace(/[ \t]+$/g, ""));

  while (lines.length > 0 && lines[0].trim().length === 0) {
    lines.shift();
  }

  while (lines.length > 0 && lines[lines.length - 1].trim().length === 0) {
    lines.pop();
  }

  return lines.join("\n");
}
