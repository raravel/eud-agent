---
task_id: EUD-138-db9b
completed_at: 2026-06-10T15:02:51+09:00
duration_minutes: 26
coding_retries: 0
verify_retries: 1
review_rounds: 1
verification_required: true
verification_passed: true
blocking_issues: true
providers:
  coder: codex
  reviewer: codex
review_scores: {}
tokens:
  estimated: true
  input: 0
  output: 0
cost_usd: 0.00
profile: mixed
models:
  executor: gpt-5.5
  reviewer: gpt-5.5
codex_usage:
  coder_session_id: 019eb00d-e995-7262-828d-28091ec82544
  coder_tokens:
    input: 2582550
    output: 25775
    total: 2608325
  reviewer_tracked: false
---

## Summary
Built a new LOCAL-ONLY Node.js + TypeScript Naver-Cafe scraper under `tools/scraper/`. It logs into
Naver Cafe with a supplied cookie, scrapes the EUD/eps boards, maps each post to the corpus JSONL row
schema (`{title, content, url?, source}`) that `ci/build_rag_index.rs` parses, and writes
`ci/corpus/*.jsonl` atomically (tmp + rename). The package is self-contained (its own package.json +
tsconfig, TypeScript ~5.9 matching the panel) and runs via `tsx`/`vitest`. It is never invoked by CI
(cookie + ToS).

## Changes
New package `tools/scraper/`:
- `package.json` (deps: undici, cheerio; dev: typescript ~5.9, vitest, tsx, @types/node), `tsconfig.json`
  (NodeNext, strict, noEmit), `vitest.config.ts`, `.gitignore` (node_modules, package-lock.json, dist).
- `src/mapper.ts` — pure `postToCorpusRow(ParsedPost): CorpusRow`; cheerio HTML→text, preserving
  `<pre>` line breaks, collapsing ordinary block whitespace.
- `src/naverClient.ts` — undici client; sends the cookie header; throws `CookieExpiredError` on
  login-redirect/401/markers; redacts the cookie to `***` (never logged).
- `src/scraper.ts` — board iteration, throttle (default 750ms), resumable dedup by post id, stable
  numeric-id sort, dry-run/limit.
- `src/corpusWriter.ts` — atomic JSONL write (tmp + rename), stable key order, partial-file cleanup.
- `src/config.ts` — board list + `ci/corpus` output dir resolved from `import.meta.url` (no hardcoded
  absolute path).
- `src/index.ts` — CLI: cookie from `NAVER_COOKIE`/`NAVER_COOKIE_FILE`, fail-fast with refresh hint
  and no partial write; flags `--dry-run`, `--limit`, `--board`.
- `src/mapper.test.ts` — offline unit tests (row mapping + multi-line `<pre>` preservation).
- `README.md` — install, cookie setup (never commit), dry-run/sample mode, polite-scraping, LOCAL-ONLY.

## Verification
Run by the orchestrator in the worker worktree (`tools/scraper`), after `npm install`:
- `npx tsc --noEmit` — clean (after 1 verify retry; see Incident).
- `npx vitest run` — 2 tests pass (mapper schema + code-block line breaks), no network.
Completion criteria:
- [PASS] self-contained Node/TS package; `npx tsc --noEmit` passes.
- [PASS] cookie from env/file, never committed; missing/expired → fail-fast refresh hint, no partial write.
- [PASS] JSONL `{title, content, url?, source}`; atomic tmp+rename; stable numeric-id ordering.
- [PASS] polite throttle + resumable; dry-run/sample mode documented in README.
- [PASS] offline unit test covers row mapping; cookie redacted (`***`), never logged.

## Review
Codex review (`codex exec review --base main`) raised two blocking findings; both were valid in-scope
defects and were fixed in one review round:
- [P2] `collectExistingIds` (scraper.ts) `continue`d after a nonnumeric prefixed id (cafebook's
  `cbk_126461`) and never derived the numeric id (`126461`) from the URL, so a full refresh would
  treat every existing cafebook row as new and append duplicates. Fixed by also extracting the numeric
  id from `row.url` (removed the early `continue`).
- [P2] `normalizeBlockText` (mapper.ts) collapsed newlines inside `<pre>` code blocks, degrading the
  code-heavy corpus. Fixed with a separate `normalizePreformattedText` path that preserves internal
  line breaks; ordinary blocks keep whitespace collapse. A new unit test asserts multi-line `<pre>`
  retention. Re-verified: tsc clean, vitest green.

## Harness Sync
- tech-stack.md ## Active Dependencies += undici ^7.16.0 (BOUND)
- tech-stack.md ## Active Dependencies += cheerio ^1.1.2 (BOUND)
- features/16_rag-corpus-pipeline.md: `tools/scraper/` (package.json, tsconfig.json, src/*.ts) already
  documented under ## Implementation — idempotent, no per-file binding added.
- Contract-drift guard: diff is purely additive (new package); no removed/renamed spec identifiers.

## Notes
- Provider model override: the `mixed` profile names `gpt-5.2-codex`; per a recorded lesson it returns
  HTTP 400 on the active account, so all codex calls used `-m gpt-5.5`.
- Codex workers could not commit inside the worktree (sandbox denies the parent repo's
  `.git/worktrees/.../index.lock`); the orchestrator committed each step on the worker's behalf and
  ran all `npm install` / `tsc` / `vitest` verification (sandbox blocks them for the worker).
- `package-lock.json` is gitignored by the tool's `.gitignore` (worker's choice for a local-only dev
  tool); only `package.json` is tracked.
- Board URLs/cafe ids in `src/config.ts` are best-effort defaults; the tool cannot be live-tested here
  (no cookie + ToS), so they are left for the user to confirm against their own session. The offline
  unit test and the schema mapping are the verified surface.

## Incident

### What broke
- `npx tsc --noEmit` failed: `src/naverClient.ts` passed `maxRedirections: 0` to undici's `request()`,
  which is not in undici v7's `request` options type (TS2353).

### Why
- undici does not auto-follow redirects by default, so the explicit `maxRedirections` was both
  unnecessary and untyped on that overload.

### What fixed it
- Verify retry 1/2: resumed the same codex session to drop the `maxRedirections: 0` property; the 3xx
  login-detection branch already reads `response.headers.location`, so behavior is unchanged. tsc clean
  + vitest green afterward.
