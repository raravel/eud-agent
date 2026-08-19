import { fetch } from "undici";

export class CookieExpiredError extends Error {
  constructor(message = "Naver login cookie is expired or invalid.") {
    super(message);
    this.name = "CookieExpiredError";
  }
}

export type NaverClientOptions = {
  cookie: string;
  userAgent?: string;
};

export class NaverClient {
  private readonly cookie: string;
  private readonly userAgent: string;

  constructor(options: NaverClientOptions) {
    const cookie = options.cookie.trim();
    if (cookie.length === 0) {
      throw new CookieExpiredError("Naver login cookie is empty.");
    }

    this.cookie = cookie;
    this.userAgent =
      options.userAgent ??
      "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 " +
        "(KHTML, like Gecko) Chrome/125.0 Safari/537.36 eud-agent-local-scraper";
  }

  async fetchText(url: string): Promise<string> {
    const response = await fetch(url, {
      method: "GET",
      redirect: "follow",
      headers: {
        accept:
          "application/json,text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        "accept-language": "ko-KR,ko;q=0.9,en-US;q=0.7,en;q=0.6",
        cookie: this.cookie,
        origin: "https://cafe.naver.com",
        referer: "https://cafe.naver.com/",
        "user-agent": this.userAgent
      }
    });

    const location = response.headers.get("location") ?? response.url;
    const text = await response.text();

    if (isLoginRequiredResponse(response.status, location)) {
      throw new CookieExpiredError(
        `Naver login cookie was rejected while fetching ${url}. Cookie=${redactCookie(
          this.cookie
        )}`
      );
    }

    if (response.status >= 300 && response.status < 400) {
      throw new Error(`Unexpected redirect while fetching ${url}: ${location}`);
    }

    if (response.status >= 400) {
      throw new Error(`HTTP ${response.status} while fetching ${url}`);
    }

    return text;
  }

  async fetchJson<T>(url: string): Promise<T> {
    const text = await this.fetchText(url);
    try {
      return JSON.parse(text) as T;
    } catch {
      throw new Error(`Expected JSON while fetching ${url}`);
    }
  }
}

export function redactCookie(cookie: string): string {
  return cookie.trim().length > 0 ? "***" : "";
}

function isLoginRequiredResponse(statusCode: number, location: string | undefined): boolean {
  if (statusCode === 401) {
    return true;
  }

  return Boolean(location && /nid\.naver\.com\/nidlogin|nidlogin\.login/i.test(location));
}
