# EUD Naver Cafe Scraper

This is a local-only scraper for refreshing `ci/corpus/*.jsonl` from Naver Cafe sources.
It is never run in CI because it requires a personal Naver login cookie and must respect
Naver's terms and rate limits.

## Install

```sh
npm install
```

Do not commit `node_modules/`, `package-lock.json`, cookies, or generated runtime output
from local experiments.

## Cookie Setup

The scraper reads the Naver login cookie from one of these sources:

```sh
NAVER_COOKIE="NID_AUT=...; NID_SES=..."
```

or:

```sh
NAVER_COOKIE_FILE="C:\path\to\naver-cookie.txt"
```

Never commit the cookie. If the scraper reports that the session is expired, sign in to
Naver again in a browser, refresh the cookie, and rerun the command.

## Dry Run

Dry-run mode fetches a small sample and prints JSONL rows to stdout without writing
`ci/corpus`.

```sh
npm run scrape -- --dry-run --limit 3
```

You can limit the run to one configured board:

```sh
npm run scrape -- --dry-run --limit 3 --board articles
```

Available boards are defined in `src/config.ts`.

## Full Local Refresh

After setting `NAVER_COOKIE` or `NAVER_COOKIE_FILE`, run:

```sh
npm run scrape
```

The scraper writes JSONL atomically by creating `<target>.tmp` and renaming it over the
final file. It reads existing rows first, skips article ids already present in output,
and sorts rows by numeric article id to keep rerun diffs small.

## Polite Scraping

Requests are throttled with a default delay of about 750 ms. Keep sample limits small
when testing, avoid repeated full refreshes, and stop immediately if Naver rejects the
cookie or shows login-required responses.
