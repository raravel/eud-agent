# Batch DAT authoring

## Decision

The model-facing mutation API is one schema-rich `dat_patch` call. Individual setters are not registered or advertised.

The canonical authoring format is sparse JSON, not SQLite, EDS, generated Python, or E3S.

## Documents

- `dat/standard.json`: numeric standard DAT fields
- `dat/xdat.json`: wireframe, status, and ButtonSet numeric fields
- `dat/tbl.json`: text overrides
- `dat/requirements.json`: requirement copy-string overrides
- `dat/buttons.json`: button CSV overrides

Each override stores both stock `before` and desired `after`. Stock values come from `DatCatalog`; stale catalog/project pairs fail closed.

## Request schema

Each `changes[]` entry is a tagged union:

- `dat {dat, objectId, field, before, after}`
- `xdat {dat, objectId, field, before, after}`
- `tbl {index, before, after}`
- `requirement {dat, objectId, before, after}`
- `button {setId, before, after}`

`const` tags are validated during tool admission. Exactly one union shape must match.

## Model workflow

1. Batch-read targets with `dat_get(items[])` or the family-specific read tool.
2. Prepare all exact before/after values.
3. Submit one `dat_patch(changes[])` whenever the bounded request fits.
4. Run `build_run`; fix structured diagnostics rather than retrying blind.

## Atomicity and rollback

- Maximum 300 changes per call.
- Duplicate, stale, no-op, malformed, or out-of-range input writes nothing.
- A successful patch updates all affected sparse documents as one logical transaction.
- Semantic journal entries support per-item and all-item reject.
- Reopen/restart reads the exact persisted state.

## Benchmark evidence

Frozen benchmark artifacts live under `benchmark-results/dat-authoring/` and decision 19.

Schema-rich batch MCP exact-state results:

| target count | success | median | MCP calls |
|---:|---:|---:|---:|
| 1 | 3/3 | 17.948 s | 2 |
| 50 | 3/3 | 51.987 s | 6 |
| 200 | 3/3 | 38.497 s | 6 |

The individual-call 200-target path completed only 1/3 runs. This evidence is why individual model-facing setters were removed.

## Permanent verification

`batch_patch_persists_exact_state_for_1_50_and_200_changes` applies each batch, reopens the project, and compares every stored target and before/after value.
