# epScript agent preflight

## Purpose

Give Codex fast, read-only diagnostics for a batch of complete or exact-edit EPS candidates before canonical mutation. Diagnostics are advisory; euddraft remains final build authority.

## Native snapshot

`source_snapshot.rs` and `NativeProject::source_snapshot` provide:

- exact project identity and revision;
- exact MainFile;
- every confined `src/**/*.eps` path/content/hash;
- deterministic ordering.

`EpsPreflight` mirrors this snapshot under LocalAppData for the pinned analyzer adapter. There is no Editor snapshot command, transport roundtrip, flattened filename dump, heartbeat, or compiling inbox delay.

## Tool contract

`eps_check {files[]}` accepts candidate entries with either:

- complete `code`, or
- the exact ordered edit list later passed to `file_edit`.

The runtime validates paths, duplicate/case collisions, content bounds, source revision, and candidate coverage. Mutually dependent files belong in one call.

Result:

- checked files;
- structured path/line/column/severity diagnostics;
- import graph;
- truncation/omission metadata.

A missing analyzer produces an explicit unavailable result and never makes a valid native write look compiled.

## Analyzer process

The pinned Node adapter uses framed JSON over stdio, is lazy-started, mutex-serialized, time-bounded, and restartable after protocol/process failure. Candidate content is data, never command-line text.

## Integration

- file create/write/edit invalidates or refreshes the native snapshot revision;
- preflight reads consume no write registration or journal budget;
- mutations still require normal evidence and write coordination;
- `build_run` is required for final runtime-affecting acceptance.

## Verification

Tests cover nested/Korean paths, empty files, imports, exact edits, stale revisions, malformed frames, process timeout/restart, diagnostics truncation, and zero journal/action-budget changes.
