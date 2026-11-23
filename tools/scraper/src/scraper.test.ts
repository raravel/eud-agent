import { afterEach, describe, expect, it, vi } from "vitest";
import type { NaverClient } from "./naverClient.js";
import { scrape } from "./scraper.js";

const researchMenuId = 33;

function fakeClient(articles: Record<string, unknown>): NaverClient {
  return {
    fetchText: async () => {
      throw new Error("unexpected fetchText");
    },
    fetchJson: async (url: string) => {
      if (url.includes("/menus/")) {
        const page = new URL(url).searchParams.get("page");
        return {
          result: {
            articleList:
              page === "1"
                ? Object.keys(articles).map((id) => ({
                    type: "ARTICLE",
                    item: { articleId: Number(id), menuId: researchMenuId, subject: `t${id}` }
                  }))
                : []
          }
        };
      }
      const id = /articles\/(\d+)/.exec(url)?.[1] ?? "";
      return articles[id];
    }
  } as unknown as NaverClient;
}

describe("scrape", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("skips a private (unreadable) article and keeps the readable ones", async () => {
    vi.spyOn(console, "log").mockImplementation(() => undefined);
    vi.spyOn(console, "error").mockImplementation(() => undefined);

    const client = fakeClient({
      "900000001": {
        result: {
          article: {
            id: 900000001,
            subject: "[네이버] 비공개 게시글 입니다.",
            isReadable: false,
            menu: { id: researchMenuId }
          }
        }
      },
      "900000002": {
        result: {
          article: {
            id: 900000002,
            subject: "readable",
            isReadable: true,
            contentHtml: "<p>body</p>",
            menu: { id: researchMenuId }
          }
        }
      }
    });

    const summaries = await scrape({
      client,
      boards: ["research"],
      dryRun: true,
      delayMs: 0,
      limit: 10
    });

    expect(summaries).toHaveLength(1);
    expect(summaries[0].fetched).toBe(1);
    expect(summaries[0].unreadable).toBe(1);
  });
});
