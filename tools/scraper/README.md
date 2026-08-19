# EUD RAG Corpus Scraper

This local tool refreshes `ci/corpus/*.jsonl` from authenticated Naver Cafe data and
pinned public Git repositories. Naver commands require a personal login cookie and must
respect Naver's terms and rate limits; public-source synchronization requires no secret.

## Install

```sh
npm install
```

Do not commit `node_modules/`, cookies, or generated runtime output from local experiments.

## Cookie Setup

The scraper reads the Naver login cookie from one of these sources:

```sh
NAVER_COOKIE="NID_AUT=...; NID_SES=..."
```

or:

```sh
NAVER_COOKIE_FILE="C:\path\to\naver_cookie.txt"
```

Never commit the cookie. If the scraper reports that the session is expired, sign in to
Naver again in a browser, refresh the cookie, and rerun the command.

## Public Source Sync

Refresh SCRMapDocs, eudplib, eud-book, and EUD Editor 3 snapshots:

```sh
npm run sync-public
```

The command shallow-clones each upstream, records its exact commit (and project version
where available), emits deterministic JSONL, and writes `ci/corpus/THIRD_PARTY_NOTICES.txt`.
No Naver cookie is read.

## Dry Run

Dry-run mode fetches a small sample and prints JSONL rows to stdout without writing
`ci/corpus`.

```sh
npm run scrape -- --dry-run --limit 3
```

You can limit the run to one configured board. `articles` expands to all configured
article menus:

```sh
npm run scrape -- --dry-run --limit 3 --board articles
```

Available boards and their numeric Naver menu ids are defined in `src/config.ts`.

## Full Local Naver Refresh

After setting `NAVER_COOKIE` or `NAVER_COOKIE_FILE`, run:

```sh
npm run scrape
```

The scraper reads Naver's authenticated board-list and article JSON APIs, then writes
JSONL atomically by creating `<target>.tmp` and renaming it over the final file. It reads
existing rows first, skips article ids already present in output, and sorts rows by numeric
article id to keep rerun diffs small.

## Polite Scraping

Requests are throttled with a default delay of about 750 ms. Keep sample limits small
when testing, avoid repeated full refreshes, and stop immediately if Naver rejects the
cookie or shows login-required responses.
