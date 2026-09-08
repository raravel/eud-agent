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

Refresh SCRMapDocs, eudplib, eud-book, EUD Editor 3, and the selected eudtools snapshots:

```sh
npm run sync-public
```

The command shallow-clones each upstream, records its exact commit (and project version
where available), emits deterministic JSONL, and writes `ci/corpus/THIRD_PARTY_NOTICES.txt`.
No Naver cookie is read.

Refresh only the two eudtools corpora, preserving the other corpus files and notices:

```sh
npm run sync-public -- --only=eudtools
```

The eudtools allowlist contains exactly nine originals:

- Wiki: `EUD-Tutorial:-Creating-uncreatable-units`,
  `EUD-Tutorial:-How-to-make-units-other-than-spellcasters-cast-spells`,
  `EUD-Tutorial:-Creating-Triggered-Spells`, `EUD-Tutorial:-Extended-Animations`,
  `EUD-Tutorial:-The-Rock-Sprite,-and-removing-unwanted-sprites-&-images`, and `Button-Maker`.
- Repository: `Data/iscriptopcodes.txt`, `Data/iscriptanimations.txt`, and
  `Include/IscriptIDList.txt`.

The repository is pinned to `e9729dc12cc30e575a83940ef380570d4819b5b2`; its wiki is pinned to
`fba67326938424c005f6cbd94e8b9b385ad4e00c`. Bare snapshots and `git show` avoid checking out
wiki filenames containing Windows-invalid colons. Other pages, JavaScript, and EUDDB are excluded.
The outputs are `eudtools_wiki.jsonl` and `eudtools_reference.jsonl`.

Wiki extraction selects technical explanations rather than GUI walkthroughs, records reviewed
image context, and explicitly marks omitted long legacy code as incomplete. Each row retains
attribution, original URL, path, commit, and permission basis. Permission is based on the user's
confirmation of free use; no upstream license, including MIT, is inferred.

Compatibility caveats appear at the start of each body so runtime previews retain them:
these are legacy claims, not game behavior verified by this project, and SCMDraft/Pure EUD or
IceCC examples are not epScript. General eudtools references use tier 2; experimental wiki
material uses tier 1 through its row-level `eudtools_wiki_experimental.jsonl` source label,
not a third corpus file.

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
