# EUD-040-b8b0 Report — LIST reads f.Filetype (nonexistent member) → all files non-settable

## Symptom (live editor E2E)

Panel 전송 (send) button permanently disabled even with a project open. WS `list`
returned every file with `ftype: "?"` and `settable: false`, so the
`canSendSet` gate (`hasSettableTarget`) never opened.

## Root cause

`bridge/ZZZ_10_agent_bridge.lua` LIST handler read `f.Filetype` (lowercase t).
The VB type exposes `FileType` (uppercase T) — confirmed live via the LUA debug
channel (single-property reflection probe: `P:FileType:EFileType`). luanet
raises on the missing member, the surrounding `pcall` swallowed it, and the
`or "?"` fallback emitted `"?"` for every file. The server's `_settable_for`
substring match (CUI/SCA/RAWTEXT) then marked everything non-settable.

Secondary finding folded into the fix: luanet enum `tostring` returns
`"Name: value"` (e.g. `"ClassicTrigger: 6"`), not the bare name the
`bridge_io` docstring expects. The fix normalizes to the bare name.

## Fix

```lua
local okT, ftype = pcall(function()
    return string.match(tostring(f.FileType), "^%s*([%w_]+)")
end)
lines[#lines + 1] = p .. "\t" .. ((okT and ftype) and ftype or "?")
```

- `f.FileType` — real VB property, dot access per rules.md.
- `tostring(...)` + `^%s*([%w_]+)` — bare-name normalization
  (`"CUIEps: 0"` → `"CUIEps"`), feeding `_settable_for` substring matching.
- `(okT and ftype) and ftype or "?"` — covers both the raised-access and
  no-match falsy paths.

## Verification

- RED: both new static tests failed on the pre-fix bridge (Step A artifact).
- GREEN on main after squash-merge: full suite **246 passed, 3 skipped**
  (= prior 244-passed baseline + 2 new EUD-040 tests).
- Worker-reported worktree failures triaged on main: `test_app.py` 12 passed,
  `test_rag.py` 18 passed 1 skipped — both environmental (no `server/.venv`
  in worktrees), not regressions.
- All four live-E2E bridge fixes verified coexisting post-merge: Wpf
  CreationProperties import (EUD-038), ppid accept (EUD-037), `f.FileType`
  (EUD-040), `DispatcherTimer(DispatcherPriority.Normal)` (EUD-039).

## Tests added

`server/tests/test_bridge_list_static.py`:

- `test_list_reads_filetype_uppercase` — region-bound to the LIST branch,
  requires `f\.FileType\b`.
- `test_no_lowercase_filetype_anywhere` — bridge-wide ban on `f\.Filetype\b`
  (revert guard).

## Review

PASS — correctness 9, spec-compliance 9, safety 9, test-quality 9.
Non-blocking notes: theoretical nil-FileType → literal `"nil"` ftype (enum is
never nil for real files); `safestr` intentionally bypassed in LIST (explicit
`"?"` fallback replaces it); stale EUD-011 docstring header in the test file
(cosmetic).

## Follow-ups (not in scope)

- Even with this fix, a project containing only GUI file types (e.g.
  `main` = ClassicTrigger) has no settable target — 전송 stays gated until a
  CUI/SCA/RawText file exists. Superseded by the agentic file-creation design
  (user decision: the agent decides create-vs-modify autonomously).
