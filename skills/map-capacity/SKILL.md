---
name: map-capacity
description: "Reduce a freeze-protected map's MPQ block-table usage by shrinking compiled epScript payload without changing gameplay or on-screen text, using measured per-pattern costs and per-step build evidence."
---

# 맵 용량 최적화

Activate when the user selects this skill. It does not authorize gameplay changes: every edit must keep game behavior, on-screen text bytes, colors, and row numbers identical. Use the project's normal instructions and conventions (specs, memory, file placement, aliases) for where helpers live and how they are named.

## What the limit is

- StarCraft opens a map only while its MPQ block table has at most 262,144 entries (in-game: 257,169 opened, 263,660 did not). The build warns from 90%.
- freeze overlaps the block table with the compressed `scenario.chk`, so entries ≈ (32 + compressed CHK bytes + hash table bytes) / 16 + file count + 2 (euddraft `mpqprt/mpq/mpqwrite.cpp`). One entry is about 16 compressed CHK bytes.
- Sounds, OGG/WAV and other MPQ files add one entry each regardless of size. Removing or recompressing audio does not help.
- `sectorSize` is ignored under freeze. `shufflePayload` should stay off. Disabling freeze removes the limit but also the protection; never do it unless the user decides so explicitly.
- Every build varies by about ±250 entries (freeze randomness). Treat smaller differences as noise.

## Measured cost per call site

euddraft 0.11.0.1, freeze on, 200 sites × 3 builds, entries added per site (2026-10-03). Re-measure if euddraft changes.

| Pattern | Inline | Shared function |
|---|---|---|
| `printfAt(row, "constant")`, no args | ~5 | ~8 (worse) |
| printf/printfAt with 1 arg | ~27 | – |
| printf/printfAt with 2 args | ~30 | ~10 |
| `"{}.{}%"` with `x / 10, x % 10` | ~41 | ~7 |
| division by a constant divisor used for the first time | ~49 | reuse of a divisor ≈ 0 |
| `foreach(i : py_range(11)) { printfAt(i, " "); }` | ~47 | `while`: ~0 |

`py_range` is compile-time unrolling; a `while` loop costs a fixed ~5–7 entries:
- `a[i] = 0` × 6: py_range ~4, while ~9 (keep py_range)
- `f(i)` × 2: py_range ~2, while ~7 (keep py_range)
- `f(i)` × 6: py_range ~7, while ~4 (while slightly better)

Rule: convert a `py_range` loop only when iterations × body cost clearly exceeds ~7 entries (printf, division by new constants, several calls, nested py_range).

## Never convert

- `py_range` whose index must be a compile-time constant: indexing a Python list/tuple or function list, building location/unit name strings, player constants (`P1 + i`), constants inside conditions.
- Constant-only printf lines.
- Per-frame code where a runtime loop adds executed triggers (HUD bars, camera, per-frame views) — ask first.

## Workflow

1. Baseline: build, then read the output map header block count (u32 LE at MPQ header + 0x1C), the collected object count and `Output scenario.chk` size from the build log. Record them.
2. Rank candidates by expected saving: printf inside py_range → repeated arg-bearing printf shapes (decimal `{}.{}` first) → heavy py_range bodies → repeated divisions by distinct constants.
3. Shared print helpers: a `StringBuffer` cannot be passed as a function argument (it becomes an `EUDVariable`; the build fails with `'EUDVariable' object has no attribute 'printfAt'`), so the helper uses its own module-level buffer and takes row, `Db` label pointers and values. Group only format strings that are byte-identical including color codes. Follow how existing views mark chat rows. Build once right after the first helper to confirm the pattern compiles and renders.
4. Change one category at a time, build, record the block count and delta. If a step's delta is within noise, report it and ask whether to keep it.
5. Finish with baseline → final block count and % of 262,144, the list of changed sites, and how identical on-screen output was confirmed. Record the measured costs and the decisions in the project's conventions document.

## Do not

- Do not change gameplay numbers, text, colors, rows, timing or MSQC lines.
- Do not delete audio, change `sectorSize`, enable `shufflePayload`, or turn off freeze.
- Do not report a saving without a build-measured block count.
