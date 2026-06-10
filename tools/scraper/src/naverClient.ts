import { request } from "undici";

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
    const response = await request(url, {
      method: "GET",
      headers: {
        accept:
          "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        "accept-language": "ko-KR,ko;q=0.9,en-US;q=0.7,en;q=0.6",
        cookie: this.cookie,
        "user-agent": this.userAgent
      }
    });

    const location = headerValue(response.headers.location);
    const text = await response.body.text();

    if (isLoginRequiredResponse(response.statusCode, location, text)) {
      throw new CookieExpiredError(
        `Naver login cookie was rejected while fetching ${url}. Cookie=${redactCookie(
          this.cookie
        )}`
      );
    }

    if (response.statusCode >= 300 && response.statusCode < 400) {
      throw new Error(`Unexpected redirect while fetching ${url}: ${location ?? "(none)"}`);
    }

    if (response.statusCode >= 400) {
      throw new Error(`HTTP ${response.statusCode} while fetching ${url}`);
    }

    return text;
  }
}

export function redactCookie(cookie: string): string {
  return cookie.trim().length > 0 ? "***" : "";
}

function isLoginRequiredResponse(
  statusCode: number,
  location: string | undefined,
  body: string
): boolean {
  if (statusCode === 401) {
    return true;
  }

  if (location && /nid\.naver\.com\/nidlogin|nidlogin\.login/i.test(location)) {
    return true;
  }

  return [
    "nidlogin.login",
    "로그인 후 이용",
    "로그인이 필요",
    "CafeLogin",
    "login_required"
  ].some((marker) => body.includes(marker));
}

function headerValue(value: string | string[] | undefined): string | undefined {
  if (Array.isArray(value)) {
    return value.join(", ");
  }

  return value;
}
