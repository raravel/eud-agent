---
name: release
description: "Publish a numbered release of the map: summarize what changed since the previous release from git, prepend a CHANGELOG.md section, set the versioned output map name and in-game title with release_prepare, build, and give the patch notes in chat."
---

# 배포

Activate when the user selects this skill. A release records what the project already contains; it does not authorize gameplay changes beyond what the user asks for in the same message.

## What a release is

- Version `vMAJOR.MINOR` (`v1.3`). Without a version from the user, `release_prepare` takes one minor version above the previous release (`v1.0` for the first). A version the user names must be newer than the previous release.
- Output map `build/[EUD]{title}{version}.<ext>` (e.g. `build/[EUD]운빨디펜스v1.3.scx`) and in-game map title `{title} {version}`. Only the BUILD OUTPUT carries the version; the source map, its title and the map properties stay untouched.
- `{title}` is the stored release title, else the source map's scenario title. Pass `title` only when the user names a different one; it is stored and reused by later releases.
- The app tags this turn's commit `release/<version>` only when `build_run` succeeded for the current revision. A failed build leaves no tag, and the user is told why.

## Workflow

1. Call `release_history`. It returns `previousVersion`, `nextVersion`, the commits since the previous release (all history when there is none), `sourceTitle`, `releaseTitle`, and the current `outputMap`/`outputTitle`.
2. Read the commits and, where a subject is unclear, the changed files (`fs_read`, `source_search`). Ignore commits that only touch agent documents, memory or the changelog itself.
3. Write the changes as player-facing notes in Korean gameplay terms (new units, balance numbers, rules, UI, bug fixes), grouped as 추가 / 변경 / 수정. Never list file names or commit hashes. Leave out a change you cannot explain from the code instead of guessing.
4. Prepend a section to `CHANGELOG.md` at the project root with `fs_read` then `fs_write`/`fs_edit` (create it with a `# 변경 기록` heading if missing). Newest section first, directly under the heading:

   ```markdown
   ## v1.3 — 2026-10-12

   ### 추가
   - ...
   ```

   Use the version `release_prepare` will resolve (the user's version, else `nextVersion`) and today's date.
5. Call `release_prepare` with `version` only when the user named one and `title` only when the user named one. If it refuses the version, tell the user the previous version and ask for a newer one; do not pick one yourself.
6. Call `build_run`. Repair every reported error and build again. Do not finish while the latest build of the current revision failed: the release would carry no tag.
7. Final answer: the version, output map path, in-game title, and the same patch notes as the changelog section, ready to paste. Say that the release tag is added when this turn's work is recorded.

## Do not

- Do not change gameplay, balance, text or map data unless the user asked for it in this request.
- Do not edit the source map's title or scenario properties to show the version.
- Do not rename `outputMap` or set `OutputTitle` by hand with `settings_set`; `release_prepare` owns both.
- Do not reuse or lower a released version.
