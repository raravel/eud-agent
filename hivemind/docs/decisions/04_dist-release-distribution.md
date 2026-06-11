# Decision 04: Built panel output distributed via GitHub Releases; dist never committed

- Date: 2026-06-04
- Status: Accepted; release/updater phase implemented in [[decisions/17_updater-implementation]] (2026-06-11)
- Context: The React panel introduces a build step, conflicting with the original "user machine needs no build" drop-in principle. The distribution model had to be decided before adopting the toolchain.
- Considered:
  - Commit `panel/dist/` to git — Pros: install needs no Node, simplest drop-in. Cons: large generated diffs every build, churn. Recommendation: ★★☆.
  - Build via setup script on the user machine — Pros: repo stays source-only. Cons: requires Node + npm install on every user machine. Recommendation: ★★☆.
  - GitHub Releases carry packaged artifacts; an updater downloads them — Pros: repo stays source-only AND user machines need no build; versioned distribution. Cons: requires release pipeline + updater (new components). Recommendation: chosen by user.
- Chosen: GitHub Releases packaging + updater (user's own design). `panel/dist/` is NEVER committed (gitignored). During development (before any release exists), dev machines build locally (`npm run build`) and the server serves the local `panel/dist/`.
- Rationale: user decision — "release 할때만 필요한 내용들을 압축해서 올리고, git에는 dist를 커밋하지 않음. 업데이터가 release에서 가져다 필요한걸 다운할 것."
- Impact: .gitignore (panel/dist, panel/node_modules), rules.md (dist-never-committed rule), a placeholder story for the release/updater phase (NOT decomposed now), GitHub remote to be created when that phase starts.
