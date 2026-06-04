---
created: '2026-06-04'
depends_on: []
id: EUD-027-ad4f
parent: EUD-025-865b
priority: low
status: pending
title: Release packaging + updater (GitHub Releases)
type: story
updated: '2026-06-04'
---

## Description
PLACEHOLDER — deliberately NOT decomposed yet (user decision: separate later phase). Distribution model per Decision 04: the panel build output (`panel/dist/`) is never committed; a release pipeline packages the needed artifacts (built panel, bridge lua, vendored DLLs, scripts) into a zip attached to GitHub Releases, and an updater (PowerShell script invoked by/alongside install_dropin.ps1) checks the latest release and downloads/replaces what the install needs, so user machines never build. Prerequisites when this phase starts: create the GitHub remote for this repo, decide versioning scheme, then run /hv:plan to decompose (release packaging script, updater script, version check UX).

## Spec References
- [[decisions/04_dist-release-distribution|04_dist-release-distribution]] `../docs/decisions/04_dist-release-distribution.md`

## Completion Criteria
- [ ] (deferred) decomposed via /hv:plan when the user starts the release phase