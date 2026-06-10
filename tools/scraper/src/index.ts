import { readFile } from "node:fs/promises";
import { scrape } from "./scraper.js";
import { CookieExpiredError, NaverClient } from "./naverClient.js";

const refreshHint =
  "Refresh your Naver login cookie and set NAVER_COOKIE or NAVER_COOKIE_FILE.";

type CliOptions = {
  dryRun: boolean;
  limit?: number;
  boards: string[];
};

async function main(): Promise<void> {
  const options = parseArgs(process.argv.slice(2));
  const cookie = await readCookie();

  if (cookie.trim().length === 0) {
    fail(`Missing Naver login cookie. ${refreshHint}`);
    return;
  }

  const client = new NaverClient({ cookie });

  try {
    const summaries = await scrape({
      client,
      boards: options.boards,
      dryRun: options.dryRun,
      limit: options.limit
    });

    for (const summary of summaries) {
      console.error(
        `${summary.board}: fetched=${summary.fetched} skipped=${summary.skipped} total=${summary.totalRows}`
      );
    }
  } catch (error) {
    if (error instanceof CookieExpiredError) {
      fail(`${error.message} ${refreshHint}`);
      return;
    }

    throw error;
  }
}

function parseArgs(args: string[]): CliOptions {
  const options: CliOptions = {
    dryRun: false,
    boards: []
  };

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];

    if (arg === "--dry-run") {
      options.dryRun = true;
      continue;
    }

    if (arg === "--limit") {
      const value = args[index + 1];
      if (!value) {
        throw new Error("--limit requires a positive integer");
      }
      options.limit = parsePositiveInteger(value, "--limit");
      index += 1;
      continue;
    }

    if (arg === "--board") {
      const value = args[index + 1];
      if (!value) {
        throw new Error("--board requires a board name");
      }
      options.boards.push(value);
      index += 1;
      continue;
    }

    throw new Error(`Unknown argument: ${arg}`);
  }

  return options;
}

async function readCookie(): Promise<string> {
  const inlineCookie = process.env.NAVER_COOKIE;
  if (inlineCookie && inlineCookie.trim().length > 0) {
    return inlineCookie;
  }

  const cookieFile = process.env.NAVER_COOKIE_FILE;
  if (!cookieFile || cookieFile.trim().length === 0) {
    return "";
  }

  return readFile(cookieFile, "utf8");
}

function parsePositiveInteger(value: string, flag: string): number {
  const parsed = Number.parseInt(value, 10);
  if (!Number.isFinite(parsed) || parsed <= 0 || String(parsed) !== value) {
    throw new Error(`${flag} requires a positive integer`);
  }

  return parsed;
}

function fail(message: string): void {
  console.error(message);
  process.exitCode = 1;
}

main().catch((error: unknown) => {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
});
