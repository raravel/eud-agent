import { beforeEach, describe, expect, it, vi } from "vitest";

const { fetchMock } = vi.hoisted(() => ({ fetchMock: vi.fn() }));

vi.mock("undici", () => ({ fetch: fetchMock }));

import { CookieExpiredError, NaverClient } from "./naverClient.js";
function response(contentType: string, body: string, url = "https://article.cafe.naver.com/") {
  return {
    status: 200,
    url,
    headers: {
      get(name: string) {
        return name.toLowerCase() === "content-type" ? contentType : null;
      }
    },
    text: vi.fn().mockResolvedValue(body)
  };
}

describe("NaverClient", () => {
  beforeEach(() => {
    fetchMock.mockReset();
  });

  it("does not treat login-related text inside an article JSON body as an expired cookie", async () => {
    fetchMock.mockResolvedValue(
      response(
        "application/json;charset=UTF-8",
        JSON.stringify({ result: { article: { contentHtml: "로그인이 필요합니다" } } })
      )
    );

    const client = new NaverClient({ cookie: "NID_AUT=secret" });

    await expect(client.fetchJson("https://example.test/article")).resolves.toEqual({
      result: { article: { contentHtml: "로그인이 필요합니다" } }
    });
  });
  it("rejects a response redirected to Naver login", async () => {
    fetchMock.mockResolvedValue(
      response(
        "text/html;charset=UTF-8",
        "<p>login</p>",
        "https://nid.naver.com/nidlogin.login"
      )
    );

    const client = new NaverClient({ cookie: "NID_AUT=secret" });

    await expect(client.fetchText("https://example.test/login")).rejects.toBeInstanceOf(
      CookieExpiredError
    );
  });
});
